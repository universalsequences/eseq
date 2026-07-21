use crate::macro_engine::{Macro, MacroMapping};
use crate::effects::{EffectDescriptor, EffectSlotSnapshot, BUILTIN_SLOT_COUNT};
use crate::plock_variants::PlockVariantRegistry;
use crate::sequencer::{
    BusId, StepCellSnapshot, TrackId, TrackParamsSnapshot, TrackPatternId, MAX_STEPS,
};
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

use super::command::{history_policy, sanitize_pasted_step_snapshot, AppCommand};
use super::history::{
    step_snapshot_bit_exact_eq, ActiveGesture, ApplyMode, BusEffectChainPatch,
    BusEffectChainState, BusEffectValuesPatch, BusMixerPatch, BusMixerSnapshot,
    DeviceId, DeviceValueSnapshot, DeviceValuesPatch, EditPatch, EffectChainPatch,
    EffectChainState, EffectInstanceState, EffectPatternSlots, GestureId, HistoryMove,
    HistoryPolicy, HistoryReplay, InstrumentBindingPatch, MergeKey, MidiFxChainPatch,
    MidiFxChainState, MidiFxInstanceState, PatternGeometryPatch,
    RackContainerSlotState, RackEffectChainPatch, RackEffectChainState, RackSlotStructureEdit,
    RackSlotStructurePatch, StepCellDelta, StepCellsPatch,
    SceneStructurePatch,
    TrackInstrumentSource, TrackInstrumentState,
    TrackCreationPatch, TrackDeletionPatch, TrackParamsBatchPatch, TrackParamsPatch,
    TrackPresentationChange, TrackPresentationPatch, TrackPresentationState,
    TransportAuthoringSnapshot, TransportParamsPatch,
};
use super::App;
use super::fx_chain::{
    rewire_fx_chain, FxChainLocator, FxGraphEditBatch, RetainedEffectSource,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EditError {
    UnsupportedCommand,
    TrackOutOfRange { track: usize },
    MissingStableTrack { track: TrackId },
    MissingStableBus { bus: BusId },
    StepOutOfRange { step: usize },
    InvalidStepRange,
    MissingTrackPattern,
    InvalidTarget(String),
    ReplayFailed(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EditOutcome {
    NoOp,
    Applied(HistoryMove),
    AppliedUnrecorded,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MutationEffects {
    pub publish_scheduler: bool,
}

pub struct StepGestureTransaction {
    target: TrackPatternId,
    label: &'static str,
    before: BTreeMap<usize, StepCellSnapshot>,
    variant_registry_before: PlockVariantRegistry,
}

impl App {
    fn capture_rack_effect_chain_state(
        &mut self,
        track: usize,
        rack_slot: usize,
        retained_ids_by_node: Option<&std::collections::HashMap<u32, crate::sequencer::EffectInstanceId>>,
    ) -> Result<RackEffectChainState, String> {
        let track_id = self.track_registry.id_at(track)
            .ok_or_else(|| format!("Track {} has no stable identity", track + 1))?;
        let rack_slot_id = self.device_registry.rack_slot(track_id, rack_slot);
        let snapshot = self.rack_slot_effect_snapshot(track, rack_slot)?;
        let active_slots = snapshot.effect_slots.iter().enumerate()
            .take_while(|(_, slot)| slot.node_id != 0)
            .map(|(slot, _)| slot)
            .collect::<Vec<_>>();
        if snapshot.effect_slots[active_slots.len()..].iter().any(|slot| slot.node_id != 0) {
            return Err("Rack-slot effect chain contains a sparse logical layout".to_string());
        }
        let mut ids = Vec::with_capacity(active_slots.len());
        for slot in &active_slots {
            let node_id = snapshot.effect_slots[*slot].node_id;
            ids.push(retained_ids_by_node.and_then(|ids| ids.get(&node_id).copied())
                .unwrap_or_else(|| self.device_registry.rack_audio_effect(rack_slot_id, *slot)));
        }
        self.device_registry.bind_rack_audio_effect_chain(rack_slot_id, &ids)?;
        let locator = Self::rack_slot_fx_locator(track, rack_slot);
        let instances = active_slots.iter().zip(ids).map(|(slot, id)| {
            let descriptor = snapshot.effect_descriptors[*slot].clone();
            let name = descriptor.name.trim().to_string();
            let source = self.editor.effect_chain_leases.source(locator, *slot).cloned()
                .map(Ok)
                .unwrap_or_else(|| self.retained_effect_source_for_name(&name))?;
            Ok(EffectInstanceState { id, source, descriptor })
        }).collect::<Result<Vec<_>, String>>()?;
        for (slot, instance) in active_slots.iter().zip(&instances) {
            self.retain_effect_source(locator, *slot, instance.source.clone())?;
        }
        Ok(RackEffectChainState {
            instances,
            patterns: self.state.capture_rack_slot_pattern_state(track, rack_slot)?,
            macros: self.state.capture_rack_macro_pattern_state(track)?,
        })
    }

    pub fn apply_recorded_rack_effect_chain_mutation<T>(
        &mut self,
        track: usize,
        rack_slot: usize,
        label: &'static str,
        mutate: impl FnOnce(&mut App) -> Result<T, String>,
    ) -> Result<T, String> {
        finish_active_gesture(self);
        let track_id = self.track_registry.id_at(track)
            .ok_or_else(|| format!("Track {} has no stable identity", track + 1))?;
        let rack_slot_id = self.device_registry.rack_slot(track_id, rack_slot);
        let before = self.capture_rack_effect_chain_state(track, rack_slot, None)?;
        let retained_ids_by_node = before.instances.iter().enumerate()
            .map(|(slot, instance)| (before.patterns.live.effect_slots[slot].node_id, instance.id))
            .collect::<std::collections::HashMap<_, _>>();
        let result = match mutate(self) {
            Ok(result) => result,
            Err(error) => {
                let rollback = self.capture_rack_effect_chain_state(
                    track,
                    rack_slot,
                    Some(&retained_ids_by_node),
                ).and_then(|partial| {
                    self.restore_rack_effect_chain_state(track, rack_slot, &partial, &before)
                });
                return match rollback {
                    Ok(()) => Err(error),
                    Err(rollback_error) => Err(format!(
                        "Rack-slot effect edit failed ({error}); restoring the original chain also failed ({rollback_error})"
                    )),
                };
            }
        };
        let mut after = self.capture_rack_effect_chain_state(
            track,
            rack_slot,
            Some(&retained_ids_by_node),
        )?;
        if before.instances.len() == after.instances.len() {
            let before_ids = before.instances.iter().map(|instance| instance.id).collect::<Vec<_>>();
            let after_ids = after.instances.iter().map(|instance| instance.id).collect::<Vec<_>>();
            let missing_before = before_ids.iter().filter(|id| !after_ids.contains(id)).copied().collect::<Vec<_>>();
            let new_after = after_ids.iter().filter(|id| !before_ids.contains(id)).copied().collect::<Vec<_>>();
            if missing_before.len() == 1 && new_after.len() == 1 {
                if let Some(instance) = after.instances.iter_mut().find(|instance| instance.id == new_after[0]) {
                    instance.id = missing_before[0];
                }
                let ids = after.instances.iter().map(|instance| instance.id).collect::<Vec<_>>();
                self.device_registry.bind_rack_audio_effect_chain(rack_slot_id, &ids)?;
            }
        }
        let unchanged = before.instances.len() == after.instances.len()
            && before.instances.iter().zip(&after.instances).all(|(left, right)| {
                left.id == right.id && left.source == right.source
            });
        if !unchanged {
            let patch = RackEffectChainPatch {
                track: track_id,
                rack_slot: rack_slot_id,
                before,
                after,
            };
            let retained_bytes = patch.retained_bytes();
            self.history.commit(label, None, EditPatch::RackEffectChain(patch), retained_bytes);
        }
        Ok(result)
    }

    fn restore_rack_effect_chain_state(
        &mut self,
        track: usize,
        rack_slot: usize,
        current: &RackEffectChainState,
        target: &RackEffectChainState,
    ) -> Result<(), String> {
        if target.instances.len() > crate::lisp_host::MAX_CUSTOM_FX {
            return Err("Retained rack-slot effect chain exceeds host capacity".to_string());
        }
        let track_id = self.track_registry.id_at(track)
            .ok_or_else(|| format!("Track {} has no stable identity", track + 1))?;
        let rack_slot_id = self.device_registry.rack_slot(track_id, rack_slot);
        let locator = Self::rack_slot_fx_locator(track, rack_slot);
        let mut occupied = vec![false; crate::lisp_host::MAX_CUSTOM_FX];
        for instance in &current.instances {
            if let Some((owner, slot)) = self.device_registry.rack_audio_effect_location(instance.id) {
                if owner == rack_slot_id && slot < occupied.len() {
                    occupied[slot] = true;
                }
            }
        }
        let mut source_slots = Vec::with_capacity(target.instances.len());
        for desired in &target.instances {
            let current_instance = current.instances.iter().find(|item| item.id == desired.id);
            let existing_slot = self.device_registry.rack_audio_effect_location(desired.id)
                .and_then(|(owner, slot)| (owner == rack_slot_id).then_some(slot));
            let slot = match existing_slot {
                Some(slot) => slot,
                None => {
                    let slot = occupied.iter().position(|occupied| !*occupied)
                        .ok_or_else(|| "No temporary rack-slot effect slot is available for restore".to_string())?;
                    occupied[slot] = true;
                    slot
                }
            };
            if current_instance.map(|item| &item.source) != Some(&desired.source) {
                match &desired.source {
                    RetainedEffectSource::NativeBuiltin { name } => {
                        self.load_builtin_rack_slot_effect_to_slot_sync(
                            track, rack_slot, slot, name,
                        )?;
                    }
                    RetainedEffectSource::Compiled { name, source, asset_base, origin } => {
                        let result = self.editor.dylib_cache.acquire(
                            crate::lisp_host::DGenCompileKind::Effect,
                            *origin,
                            source,
                            self.graph.sample_rate,
                            asset_base.as_deref(),
                        )?;
                        let ir_slots = crate::effects::conv_reverb::StereoIrSlots::from_manifest(&result.manifest);
                        self.apply_compiled_rack_slot_effect_to_slot_sync(
                            track, rack_slot, slot, name, result,
                        )?;
                        self.retain_effect_source(locator, slot, desired.source.clone())?;
                        if let Some(ir_slots) = ir_slots {
                            let node_id = self.rack_slot_effect_snapshot(track, rack_slot)?
                                .effect_slots[slot].node_id as i32;
                            crate::effects::conv_reverb::record_ir_slots(node_id, ir_slots);
                        }
                    }
                }
            }
            source_slots.push(slot);
        }
        let old_host = self.fx_chain_host(locator)?;
        let live = self.rack_slot_effect_snapshot(track, rack_slot)?;
        let desired_snapshots = source_slots.iter()
            .map(|slot| live.effect_slots[*slot].clone())
            .collect::<Vec<_>>();
        let desired_nodes = desired_snapshots.iter().map(|slot| slot.node_id)
            .collect::<std::collections::HashSet<_>>();
        let removed_nodes = old_host.slots.iter()
            .filter(|slot| slot.node_id > 0 && !desired_nodes.contains(&(slot.node_id as u32)))
            .copied()
            .collect::<Vec<_>>();
        let mut restored = target.patterns.clone();
        let rebind = |saved: &mut crate::sequencer::RackSlotSnapshot| -> Result<(), String> {
            for slot in 0..crate::lisp_host::MAX_CUSTOM_FX {
                if let Some((instance, mut runtime)) = target.instances.get(slot)
                    .zip(desired_snapshots.get(slot).cloned())
                {
                    let values = saved.effect_slots[slot].authoring_values();
                    runtime.apply_authoring_values(&values)?;
                    saved.effect_descriptors[slot] = instance.descriptor.clone();
                    saved.effect_slots[slot] = runtime;
                    saved.custom_effect_names[slot] = Some(match &instance.source {
                        RetainedEffectSource::NativeBuiltin { name } => name.clone(),
                        RetainedEffectSource::Compiled { name, .. } => name.clone(),
                    });
                } else {
                    saved.effect_descriptors[slot] = EffectDescriptor::empty_custom_slot();
                    saved.effect_slots[slot] = EffectSlotSnapshot::new_empty();
                    saved.custom_effect_names[slot] = None;
                }
            }
            Ok(())
        };
        rebind(&mut restored.live)?;
        for (_, saved) in &mut restored.patterns {
            rebind(saved)?;
        }
        self.state.restore_rack_slot_effect_pattern_state(track, &restored)?;
        self.state.restore_rack_macro_pattern_state(track, &target.macros)?;
        self.graph_controller().refresh_rack_signature_from_live_state(track);
        let new_host = self.fx_chain_host(locator)?;
        let batch = FxGraphEditBatch::new(self.graph.lg.0);
        rewire_fx_chain(self.graph.lg.0, &old_host, &new_host);
        for slot in removed_nodes {
            unsafe {
                crate::audiograph::delete_node(self.graph.lg.0, slot.node_id);
                if slot.modulator_node_id > 0 {
                    crate::audiograph::delete_node(self.graph.lg.0, slot.modulator_node_id);
                }
            }
            crate::effects::conv_reverb::clear_instance(slot.node_id);
        }
        self.editor.effect_chain_leases.remap_slots(locator, &source_slots, batch.serial)?;
        drop(batch);
        for (slot, values) in restored.live.effect_slots.iter()
            .take(target.instances.len())
            .map(|slot| slot.authoring_values())
            .enumerate()
        {
            if let (Some(reference), Some(ir)) = (&values.ir, &values.prepared_ir) {
                self.restore_prepared_rack_effect_ir(
                    track, rack_slot, slot, reference, ir.clone(),
                )?;
            }
        }
        let ids = target.instances.iter().map(|instance| instance.id).collect::<Vec<_>>();
        self.device_registry.bind_rack_audio_effect_chain(rack_slot_id, &ids)?;
        for slot in 0..target.instances.len() {
            self.push_rack_slot_effect_defaults(track, rack_slot, slot);
        }
        self.push_all_delay_bpm();
        self.state.publish_scheduler_snapshot();
        Ok(())
    }

    fn capture_bus_effect_chain_state(
        &mut self,
        bus_idx: usize,
        retained_ids_by_node: Option<&std::collections::HashMap<u32, crate::sequencer::EffectInstanceId>>,
    ) -> Result<BusEffectChainState, String> {
        let bus_id = self.buses.get(bus_idx)
            .map(|bus| bus.id)
            .ok_or_else(|| format!("Bus {} not found", bus_idx + 1))?;
        let active_slots = self.buses[bus_idx]
            .effect_slots
            .iter()
            .enumerate()
            .take_while(|(_, slot)| slot.node_id != 0)
            .map(|(slot, _)| slot)
            .collect::<Vec<_>>();
        if self.buses[bus_idx].effect_slots[active_slots.len()..]
            .iter()
            .any(|slot| slot.node_id != 0)
        {
            return Err("Bus effect chain contains a sparse logical layout".to_string());
        }
        let mut ids = Vec::with_capacity(active_slots.len());
        for slot in &active_slots {
            let node_id = self.buses[bus_idx].effect_slots[*slot].node_id;
            ids.push(
                retained_ids_by_node
                    .and_then(|ids| ids.get(&node_id).copied())
                    .unwrap_or_else(|| self.device_registry.bus_audio_effect(bus_id, *slot)),
            );
        }
        self.device_registry.bind_bus_audio_effect_chain(bus_id, &ids)?;
        let locator = FxChainLocator::Bus(bus_id);
        let instances = active_slots
            .iter()
            .zip(ids)
            .map(|(slot, id)| {
                let descriptor = self.buses[bus_idx].effect_descriptors[*slot].clone();
                let name = descriptor.name.trim().to_string();
                let source = self.editor.effect_chain_leases.source(locator, *slot).cloned()
                    .map(Ok)
                    .unwrap_or_else(|| self.retained_effect_source_for_name(&name))?;
                Ok(EffectInstanceState { id, source, descriptor })
            })
            .collect::<Result<Vec<_>, String>>()?;
        for (slot, instance) in active_slots.iter().zip(&instances) {
            self.retain_effect_source(locator, *slot, instance.source.clone())?;
        }
        let live_all = self.capture_bus_pattern_snapshot();
        let live = live_all.iter().find(|snapshot| snapshot.id == bus_id)
            .cloned()
            .ok_or_else(|| format!("Bus {} has no live pattern state", bus_idx + 1))?;
        let mut repository = self.state.export_bus_pattern_repository(&live_all);
        let current_scene = self.state.current_scene_index();
        if let Some(scene) = repository.get_mut(current_scene) {
            *scene = live_all.clone();
        }
        let scenes = repository
            .into_iter()
            .map(|scene| {
                scene.into_iter().find(|snapshot| snapshot.id == bus_id)
                    .unwrap_or_else(|| live.clone())
            })
            .collect();
        let live_values = active_slots.iter()
            .map(|slot| self.buses[bus_idx].effect_slots[*slot].authoring_values())
            .collect();
        Ok(BusEffectChainState {
            instances,
            live,
            live_values,
            scenes,
            macro_mappings: self.macro_engine.capture_effect_mappings_for_bus(bus_id),
        })
    }

    pub fn apply_recorded_bus_effect_chain_mutation<T>(
        &mut self,
        bus_idx: usize,
        label: &'static str,
        mutate: impl FnOnce(&mut App) -> Result<T, String>,
    ) -> Result<T, String> {
        finish_active_gesture(self);
        let bus_id = self.buses.get(bus_idx)
            .map(|bus| bus.id)
            .ok_or_else(|| format!("Bus {} not found", bus_idx + 1))?;
        let before = self.capture_bus_effect_chain_state(bus_idx, None)?;
        let retained_ids_by_node = before.instances.iter().enumerate().map(|(slot, instance)| {
            (self.buses[bus_idx].effect_slots[slot].node_id, instance.id)
        }).collect::<std::collections::HashMap<_, _>>();
        let result = match mutate(self) {
            Ok(result) => result,
            Err(error) => {
                let rollback = self.capture_bus_effect_chain_state(
                    bus_idx,
                    Some(&retained_ids_by_node),
                ).and_then(|partial| self.restore_bus_effect_chain_state(bus_idx, &partial, &before));
                return match rollback {
                    Ok(()) => Err(error),
                    Err(rollback_error) => Err(format!(
                        "Bus effect edit failed ({error}); restoring the original chain also failed ({rollback_error})"
                    )),
                };
            }
        };
        let mut after = self.capture_bus_effect_chain_state(bus_idx, Some(&retained_ids_by_node))?;
        if before.instances.len() == after.instances.len() {
            let before_ids = before.instances.iter().map(|instance| instance.id).collect::<Vec<_>>();
            let after_ids = after.instances.iter().map(|instance| instance.id).collect::<Vec<_>>();
            let missing_before = before_ids.iter().filter(|id| !after_ids.contains(id)).copied().collect::<Vec<_>>();
            let new_after = after_ids.iter().filter(|id| !before_ids.contains(id)).copied().collect::<Vec<_>>();
            if missing_before.len() == 1 && new_after.len() == 1 {
                if let Some(instance) = after.instances.iter_mut().find(|instance| instance.id == new_after[0]) {
                    instance.id = missing_before[0];
                }
                let ids = after.instances.iter().map(|instance| instance.id).collect::<Vec<_>>();
                self.device_registry.bind_bus_audio_effect_chain(bus_id, &ids)?;
            }
        }
        let mut old_to_new = vec![None; crate::lisp_host::MAX_CUSTOM_FX];
        for (old_slot, old) in before.instances.iter().enumerate() {
            old_to_new[old_slot] = after.instances.iter()
                .position(|candidate| candidate.id == old.id);
        }
        self.macro_engine.remap_effect_mappings_for_bus(bus_id, &old_to_new);
        self.state.publish_macro_overrides(self.macro_engine.override_snapshot());
        after = self.capture_bus_effect_chain_state(bus_idx, None)?;
        let unchanged = before.instances.len() == after.instances.len()
            && before.instances.iter().zip(&after.instances).all(|(left, right)| {
                left.id == right.id && left.source == right.source
            })
            && before.live_values.len() == after.live_values.len()
            && before.live_values.iter().zip(&after.live_values)
                .all(|(left, right)| left.bit_exact_eq(right));
        if !unchanged {
            let patch = BusEffectChainPatch { bus: bus_id, before, after };
            let retained_bytes = patch.retained_bytes();
            self.history.commit(label, None, EditPatch::BusEffectChain(patch), retained_bytes);
        }
        Ok(result)
    }

    pub fn apply_compiled_bus_effect_to_slot_recorded(
        &mut self,
        bus_idx: usize,
        slot: usize,
        name: &str,
        result: crate::lisp_host::CompileResult,
    ) -> Result<(), String> {
        let source = self.retained_effect_source_for_name(name)?;
        let bus_id = self.buses.get(bus_idx)
            .map(|bus| bus.id)
            .ok_or_else(|| format!("Bus {} not found", bus_idx + 1))?;
        self.apply_recorded_bus_effect_chain_mutation(
            bus_idx,
            "Replace bus effect",
            |app| {
                app.apply_compiled_bus_effect_to_slot_sync(bus_idx, slot, name, result)?;
                app.retain_effect_source(FxChainLocator::Bus(bus_id), slot, source)
            },
        )
    }

    pub fn apply_recorded_bus_effect_value_mutation<T>(
        &mut self,
        bus_idx: usize,
        slot: usize,
        label: &'static str,
        merge_suffix: impl AsRef<str>,
        mutate: impl FnOnce(&mut App) -> Result<T, String>,
    ) -> Result<T, String> {
        let bus_id = self.buses.get(bus_idx)
            .map(|bus| bus.id)
            .ok_or_else(|| format!("Bus {} not found", bus_idx + 1))?;
        let instance = self.device_registry.bus_audio_effect(bus_id, slot);
        let scene = self.state.current_scene_index();
        self.save_current_bus_pattern();
        let current_before = self.buses.get(bus_idx)
            .and_then(|bus| bus.effect_slots.get(slot))
            .map(EffectSlotSnapshot::authoring_values)
            .ok_or_else(|| format!("Bus effect slot {} is out of range", slot + 1))?;
        let merge_key = MergeKey::new(format!(
            "bus-effect:{}:scene:{}:{}",
            instance.0,
            scene,
            merge_suffix.as_ref(),
        ));
        let entry_before = app_bus_effect_gesture_before(self, &merge_key, bus_id, instance, scene)
            .unwrap_or_else(|| current_before.clone());
        let result = match mutate(self) {
            Ok(result) => result,
            Err(error) => {
                if let Some(slot_state) = self.buses.get_mut(bus_idx)
                    .and_then(|bus| bus.effect_slots.get_mut(slot))
                {
                    if let Err(rollback_error) = slot_state.apply_authoring_values(&current_before) {
                        return Err(format!(
                            "Bus effect edit failed ({error}); rollback also failed ({rollback_error})"
                        ));
                    }
                }
                self.push_bus_effect_slot_defaults(bus_idx, slot);
                self.publish_bus_gate_runtime();
                return Err(error);
            }
        };
        let after = self.buses[bus_idx].effect_slots[slot].authoring_values();
        if current_before.bit_exact_eq(&after) {
            return Ok(result);
        }
        let patch = BusEffectValuesPatch {
            bus: bus_id,
            instance,
            scene,
            before: entry_before,
            after,
        };
        if patch.before.bit_exact_eq(&patch.after)
            && self.history.discard_active_gesture_entry(&merge_key)
        {
            return Ok(result);
        }
        ensure_coalescing_gesture(self, &merge_key);
        let retained_bytes = patch.retained_bytes();
        self.history.stage_active_gesture(
            label,
            &merge_key,
            EditPatch::BusEffectValues(patch),
            retained_bytes,
        ).ok_or_else(|| "Could not stage bus-effect history gesture".to_string())?;
        Ok(result)
    }

    fn restore_bus_effect_chain_state(
        &mut self,
        bus_idx: usize,
        current: &BusEffectChainState,
        target: &BusEffectChainState,
    ) -> Result<(), String> {
        if target.instances.len() > crate::lisp_host::MAX_CUSTOM_FX {
            return Err("Retained bus effect chain exceeds host capacity".to_string());
        }
        let bus_id = self.buses.get(bus_idx)
            .map(|bus| bus.id)
            .ok_or_else(|| format!("Bus {} not found", bus_idx + 1))?;
        let locator = FxChainLocator::Bus(bus_id);
        let mut occupied = vec![false; crate::lisp_host::MAX_CUSTOM_FX];
        for instance in &current.instances {
            if let Some((owner, slot)) = self.device_registry.bus_audio_effect_location(instance.id) {
                if owner == bus_id && slot < occupied.len() {
                    occupied[slot] = true;
                }
            }
        }
        let mut source_slots = Vec::with_capacity(target.instances.len());
        for desired in &target.instances {
            let current_instance = current.instances.iter().find(|item| item.id == desired.id);
            let existing_slot = self.device_registry.bus_audio_effect_location(desired.id)
                .and_then(|(owner, slot)| (owner == bus_id).then_some(slot));
            let slot = match existing_slot {
                Some(slot) => slot,
                None => {
                    let slot = occupied.iter().position(|occupied| !*occupied)
                        .ok_or_else(|| "No temporary bus effect slot is available for restore".to_string())?;
                    occupied[slot] = true;
                    slot
                }
            };
            if current_instance.map(|item| &item.source) != Some(&desired.source) {
                match &desired.source {
                    RetainedEffectSource::NativeBuiltin { name } => {
                        self.load_builtin_bus_effect_to_slot_sync(bus_idx, slot, name)?;
                    }
                    RetainedEffectSource::Compiled { name, source, asset_base, origin } => {
                        let result = self.editor.dylib_cache.acquire(
                            crate::lisp_host::DGenCompileKind::Effect,
                            *origin,
                            source,
                            self.graph.sample_rate,
                            asset_base.as_deref(),
                        )?;
                        let ir_slots = crate::effects::conv_reverb::StereoIrSlots::from_manifest(&result.manifest);
                        self.apply_compiled_bus_effect_to_slot_sync(bus_idx, slot, name, result)?;
                        self.retain_effect_source(locator, slot, desired.source.clone())?;
                        if let Some(ir_slots) = ir_slots {
                            let node_id = self.buses[bus_idx].effect_slots[slot].node_id as i32;
                            crate::effects::conv_reverb::record_ir_slots(node_id, ir_slots);
                        }
                    }
                }
            }
            source_slots.push(slot);
        }
        let old_host = self.fx_chain_host(locator)?;
        let desired_snapshots = source_slots.iter()
            .map(|slot| self.buses[bus_idx].effect_slots[*slot].clone())
            .collect::<Vec<_>>();
        let desired_nodes = desired_snapshots.iter().map(|slot| slot.node_id)
            .collect::<std::collections::HashSet<_>>();
        let removed_nodes = old_host.slots.iter()
            .filter(|slot| slot.node_id > 0 && !desired_nodes.contains(&(slot.node_id as u32)))
            .copied()
            .collect::<Vec<_>>();
        for slot in 0..crate::lisp_host::MAX_CUSTOM_FX {
            if let Some((instance, mut snapshot)) = target.instances.get(slot)
                .zip(desired_snapshots.get(slot).cloned())
            {
                snapshot.apply_authoring_values(&target.live_values[slot])?;
                self.buses[bus_idx].effect_descriptors[slot] = instance.descriptor.clone();
                self.buses[bus_idx].effect_slots[slot] = snapshot;
                self.buses[bus_idx].custom_effect_names[slot] = Some(match &instance.source {
                    RetainedEffectSource::NativeBuiltin { name } => name.clone(),
                    RetainedEffectSource::Compiled { name, .. } => name.clone(),
                });
            } else {
                self.buses[bus_idx].effect_descriptors[slot] = EffectDescriptor::empty_custom_slot();
                self.buses[bus_idx].effect_slots[slot] = EffectSlotSnapshot::new_empty();
                self.buses[bus_idx].custom_effect_names[slot] = None;
            }
        }
        self.buses[bus_idx].gate_sequence = target.live.gate_sequence.clone();
        let new_host = self.fx_chain_host(locator)?;
        let batch = FxGraphEditBatch::new(self.graph.lg.0);
        rewire_fx_chain(self.graph.lg.0, &old_host, &new_host);
        for slot in removed_nodes {
            unsafe {
                crate::audiograph::delete_node(self.graph.lg.0, slot.node_id);
                if slot.modulator_node_id > 0 {
                    crate::audiograph::delete_node(self.graph.lg.0, slot.modulator_node_id);
                }
            }
            crate::effects::conv_reverb::clear_instance(slot.node_id);
        }
        self.editor.effect_chain_leases.remap_slots(locator, &source_slots, batch.serial)?;
        drop(batch);
        let live_all = self.capture_bus_pattern_snapshot();
        let mut repository = self.state.export_bus_pattern_repository(&live_all);
        for (scene, saved) in repository.iter_mut().zip(&target.scenes) {
            if let Some(bus) = scene.iter_mut().find(|bus| bus.id == bus_id) {
                *bus = saved.clone();
            } else {
                scene.push(saved.clone());
            }
        }
        self.state.replace_bus_pattern_repository(repository, &live_all);
        for (slot, values) in target.live_values.iter().enumerate() {
            if let (Some(reference), Some(ir)) = (&values.ir, &values.prepared_ir) {
                self.restore_prepared_bus_effect_ir(bus_idx, slot, reference, ir.clone())?;
            }
        }
        let ids = target.instances.iter().map(|instance| instance.id).collect::<Vec<_>>();
        self.device_registry.bind_bus_audio_effect_chain(bus_id, &ids)?;
        self.macro_engine.restore_effect_mappings_for_bus(bus_id, &target.macro_mappings)
            .map_err(|error| format!("{error:?}"))?;
        self.state.publish_macro_overrides(self.macro_engine.override_snapshot());
        self.refresh_effect_sidechain_labels();
        self.publish_bus_gate_runtime();
        for slot in 0..target.instances.len() {
            self.push_bus_effect_slot_defaults(bus_idx, slot);
        }
        self.push_all_delay_bpm();
        self.state.publish_scheduler_snapshot();
        Ok(())
    }

    fn capture_track_midi_fx_chain_state(
        &mut self,
        track: usize,
    ) -> Result<MidiFxChainState, String> {
        let track_id = self
            .track_registry
            .id_at(track)
            .ok_or_else(|| format!("Track {} has no stable identity", track + 1))?;
        let names = self.state.pattern.track_params[track].midi_fx_chain();
        let ids = self.device_registry.midi_effect_chain(track_id, names.len());
        let instances = names
            .into_iter()
            .zip(ids)
            .map(|(name, id)| {
                let descriptor = crate::lisp_host::load_midi_fx_descriptor(&name)
                    .ok_or_else(|| format!("Unknown retained MIDI FX '{name}'"))?;
                Ok(MidiFxInstanceState { id, name, descriptor })
            })
            .collect::<Result<Vec<_>, String>>()?;
        let pattern_slots = self
            .state
            .capture_track_midi_fx_chain_values(track)?
            .into_iter()
            .map(|(pattern, values)| EffectPatternSlots { pattern, values })
            .collect();
        Ok(MidiFxChainState {
            instances,
            pattern_slots,
            macro_mappings: self.macro_engine.capture_midi_fx_mappings_for_track(track),
            process_chains: self.state.capture_track_process_chains(track)?,
        })
    }

    pub fn apply_recorded_track_midi_fx_chain_mutation<T>(
        &mut self,
        track: usize,
        label: &'static str,
        mutate: impl FnOnce(&mut App) -> Result<T, String>,
    ) -> Result<T, String> {
        finish_active_gesture(self);
        let track_id = self
            .track_registry
            .id_at(track)
            .ok_or_else(|| format!("Track {} has no stable identity", track + 1))?;
        let before = self.capture_track_midi_fx_chain_state(track)?;
        let result = match mutate(self) {
            Ok(result) => result,
            Err(error) => {
                return match self.restore_track_midi_fx_chain_state(track, &before) {
                    Ok(()) => Err(error),
                    Err(rollback_error) => Err(format!(
                        "MIDI-FX edit failed ({error}); restoring the original chain also failed ({rollback_error})"
                    )),
                };
            }
        };
        let mut after = self.capture_track_midi_fx_chain_state(track)?;
        let mut old_to_new = vec![None; crate::lisp_host::MAX_MIDI_FX_SLOTS];
        for (old_slot, old) in before.instances.iter().enumerate() {
            old_to_new[old_slot] = after
                .instances
                .iter()
                .position(|candidate| candidate.id == old.id && candidate.name == old.name);
        }
        self.macro_engine
            .remap_midi_fx_mappings_for_track(track, &old_to_new);
        self.state
            .remap_track_midi_fx_references(track, &old_to_new)?;
        after = self.capture_track_midi_fx_chain_state(track)?;
        let unchanged = before.instances.len() == after.instances.len()
            && before.instances.iter().zip(&after.instances).all(|(left, right)| {
                left.id == right.id && left.name == right.name
            })
            && before.pattern_slots.iter().zip(&after.pattern_slots).all(|(left, right)| {
                left.pattern == right.pattern
                    && left.values.len() == right.values.len()
                    && left.values.iter().zip(&right.values).all(|(left, right)| {
                        left.bit_exact_eq(right)
                    })
            });
        if unchanged {
            return Ok(result);
        }
        let patch = MidiFxChainPatch { track: track_id, before, after };
        let retained_bytes = patch.retained_bytes();
        self.history
            .commit(label, None, EditPatch::MidiFxChain(patch), retained_bytes);
        Ok(result)
    }

    fn restore_track_midi_fx_chain_state(
        &mut self,
        track: usize,
        target: &MidiFxChainState,
    ) -> Result<(), String> {
        let names = target.instances.iter().map(|instance| instance.name.clone()).collect::<Vec<_>>();
        let descriptors = target
            .instances
            .iter()
            .map(|instance| instance.descriptor.clone())
            .collect::<Vec<_>>();
        let patterns = target
            .pattern_slots
            .iter()
            .map(|pattern| (pattern.pattern, pattern.values.clone()))
            .collect::<Vec<_>>();
        self.state.restore_track_midi_fx_chain_values(
            track,
            &names,
            &descriptors,
            &patterns,
        )?;
        self.state
            .restore_track_process_chains(track, &target.process_chains)?;
        self.macro_engine
            .restore_midi_fx_mappings_for_track(track, &target.macro_mappings)
            .map_err(|error| format!("{error:?}"))?;
        let track_id = self.track_registry.id_at(track)
            .ok_or_else(|| format!("Track {} has no stable identity", track + 1))?;
        let ids = target.instances.iter().map(|instance| instance.id).collect::<Vec<_>>();
        self.device_registry.bind_midi_effect_chain(track_id, &ids)?;
        self.state.publish_macro_overrides(self.macro_engine.override_snapshot());
        self.sync_scratch_runtime_descriptors();
        self.state.publish_scheduler_snapshot();
        Ok(())
    }
    fn capture_track_effect_chain_state(
        &mut self,
        track: usize,
        retained_ids_by_node: Option<&std::collections::HashMap<u32, crate::sequencer::EffectInstanceId>>,
    ) -> Result<EffectChainState, String> {
        let track_id = self
            .track_registry
            .id_at(track)
            .ok_or_else(|| format!("Track {} has no stable identity", track + 1))?;
        let active_slots = (BUILTIN_SLOT_COUNT..BUILTIN_SLOT_COUNT + crate::lisp_host::MAX_CUSTOM_FX)
            .take_while(|slot| {
                self.state.pattern.effect_chains[track][*slot]
                    .node_id
                    .load(Ordering::Relaxed) != 0
            })
            .collect::<Vec<_>>();
        let mut ids = Vec::with_capacity(active_slots.len());
        for slot in &active_slots {
            let node_id = self.state.pattern.effect_chains[track][*slot]
                .node_id
                .load(Ordering::Relaxed);
            let id = retained_ids_by_node
                .and_then(|ids| ids.get(&node_id).copied())
                .unwrap_or_else(|| self.device_registry.audio_effect(track_id, *slot));
            ids.push(id);
        }
        self.device_registry
            .bind_audio_effect_chain(track_id, BUILTIN_SLOT_COUNT, &ids)?;
        let instances = active_slots
            .iter()
            .zip(ids)
            .map(|(slot, id)| {
                let descriptor = self.graph.effect_descriptors[track][*slot].clone();
                let name = descriptor.name.trim().to_string();
                let source = if let Some(source) = self
                    .editor
                    .effect_chain_leases
                    .source(FxChainLocator::Track(track), *slot)
                    .cloned()
                {
                    source
                } else if EffectDescriptor::builtin_insert(&name).is_some() {
                    RetainedEffectSource::NativeBuiltin { name }
                } else if crate::effects::conv_reverb::is_dgen_builtin(&name) {
                    RetainedEffectSource::Compiled {
                        name,
                        source: crate::effects::conv_reverb::dsp_source().to_string(),
                        asset_base: None,
                        origin: crate::lisp_host::DGenSourceOrigin::BuiltinConvolutionReverb,
                    }
                } else {
                    let path = crate::lisp_host::effect_source_path(&name);
                    let source = std::fs::read_to_string(&path).map_err(|error| {
                        format!("Could not retain effect source '{}': {error}", path.display())
                    })?;
                    RetainedEffectSource::Compiled {
                        name,
                        source,
                        asset_base: path.parent().map(std::path::Path::to_path_buf),
                        origin: crate::lisp_host::DGenSourceOrigin::Custom,
                    }
                };
                Ok(EffectInstanceState { id, source, descriptor })
            })
            .collect::<Result<Vec<_>, String>>()?;
        for (slot, instance) in active_slots.iter().zip(&instances) {
            self.retain_effect_source(
                FxChainLocator::Track(track),
                *slot,
                instance.source.clone(),
            )?;
        }
        let pattern_slots = self
            .state
            .capture_track_effect_chain_values(
                track,
                BUILTIN_SLOT_COUNT,
                crate::lisp_host::MAX_CUSTOM_FX,
            )?
            .into_iter()
            .map(|(pattern, values)| EffectPatternSlots { pattern, values })
            .collect();
        Ok(EffectChainState {
            instances,
            pattern_slots,
            macro_mappings: self.macro_engine.capture_effect_mappings_for_track(track),
            bindings: self.state.capture_track_effect_binding_state(track)?,
        })
    }

    pub fn apply_recorded_track_effect_chain_mutation<T>(
        &mut self,
        track: usize,
        label: &'static str,
        mutate: impl FnOnce(&mut App) -> Result<T, String>,
    ) -> Result<T, String> {
        finish_active_gesture(self);
        let track_id = self
            .track_registry
            .id_at(track)
            .ok_or_else(|| format!("Track {} has no stable identity", track + 1))?;
        let before = self.capture_track_effect_chain_state(track, None)?;
        let retained_ids_by_node = before
            .instances
            .iter()
            .enumerate()
            .map(|(offset, instance)| {
                let node_id = self.state.pattern.effect_chains[track]
                    [BUILTIN_SLOT_COUNT + offset]
                    .node_id
                    .load(Ordering::Relaxed);
                (node_id, instance.id)
            })
            .collect::<std::collections::HashMap<_, _>>();
        let result = match mutate(self) {
            Ok(result) => result,
            Err(error) => {
                let rollback = self
                    .capture_track_effect_chain_state(track, Some(&retained_ids_by_node))
                    .and_then(|partial| {
                        self.restore_track_effect_chain_state(track, &partial, &before)
                    });
                return match rollback {
                    Ok(()) => Err(error),
                    Err(rollback_error) => Err(format!(
                        "Effect edit failed ({error}); restoring the original chain also failed ({rollback_error})"
                    )),
                };
            }
        };
        let mut after = self.capture_track_effect_chain_state(track, Some(&retained_ids_by_node))?;
        if before.instances.len() == after.instances.len() {
            let before_ids = before.instances.iter().map(|instance| instance.id).collect::<Vec<_>>();
            let after_ids = after.instances.iter().map(|instance| instance.id).collect::<Vec<_>>();
            let missing_before = before_ids.iter().filter(|id| !after_ids.contains(id)).copied().collect::<Vec<_>>();
            let new_after = after_ids.iter().filter(|id| !before_ids.contains(id)).copied().collect::<Vec<_>>();
            if missing_before.len() == 1 && new_after.len() == 1 {
                if let Some(instance) = after.instances.iter_mut().find(|instance| instance.id == new_after[0]) {
                    instance.id = missing_before[0];
                }
                let rebound = after.instances.iter().map(|instance| instance.id).collect::<Vec<_>>();
                self.device_registry.bind_audio_effect_chain(
                    track_id,
                    BUILTIN_SLOT_COUNT,
                    &rebound,
                )?;
            }
        }
        let chain_len = self.graph.effect_descriptors[track].len();
        let mut old_to_new = (0..chain_len).map(Some).collect::<Vec<_>>();
        let mut drop_neural_slots = vec![false; chain_len];
        for (old_offset, old) in before.instances.iter().enumerate() {
            let old_slot = BUILTIN_SLOT_COUNT + old_offset;
            let new = after
                .instances
                .iter()
                .enumerate()
                .find(|(_, candidate)| candidate.id == old.id);
            old_to_new[old_slot] = new.map(|(offset, _)| BUILTIN_SLOT_COUNT + offset);
            drop_neural_slots[old_slot] = new
                .map(|(_, candidate)| candidate.source != old.source)
                .unwrap_or(true);
        }
        self.macro_engine
            .remap_effect_mappings_for_track(track, &old_to_new);
        self.state.remap_track_effect_references(
            track,
            &old_to_new,
            &drop_neural_slots,
            &self.graph.effect_descriptors[track],
        )?;
        let state = std::sync::Arc::clone(&self.state);
        let effect_descriptors = &self.graph.effect_descriptors;
        let instrument_descriptors = &self.graph.instrument_descriptors;
        let buses = &self.buses;
        self.macro_engine.revalidate_mappings(|scope, target| {
            super::projects::resolve_live_macro_target(
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
        after = self.capture_track_effect_chain_state(track, None)?;
        let unchanged = before.instances.len() == after.instances.len()
            && before.instances.iter().zip(&after.instances).all(|(left, right)| {
                left.id == right.id && left.source == right.source
            })
            && before.pattern_slots.len() == after.pattern_slots.len()
            && before.pattern_slots.iter().zip(&after.pattern_slots).all(|(left, right)| {
                left.pattern == right.pattern
                    && left.values.len() == right.values.len()
                    && left.values.iter().zip(&right.values).all(|(left, right)| {
                        left.bit_exact_eq(right)
                    })
            });
        if unchanged {
            return Ok(result);
        }
        let patch = EffectChainPatch { track: track_id, before, after };
        let retained_bytes = patch.retained_bytes();
        self.history.commit(label, None, EditPatch::EffectChain(patch), retained_bytes);
        Ok(result)
    }

    pub fn load_saved_effect_to_slot_recorded(
        &mut self,
        track: usize,
        slot: usize,
        name: &str,
    ) -> Result<(), String> {
        self.apply_recorded_track_effect_chain_mutation(
            track,
            "Replace audio effect",
            |app| app.load_saved_effect_to_slot_sync(track, slot, name),
        )
    }

    pub fn apply_compiled_effect_to_slot_recorded(
        &mut self,
        result: crate::lisp_host::CompileResult,
        name: &str,
        slot: usize,
        track: usize,
    ) -> Result<(), String> {
        let source = self.retained_effect_source_for_name(name)?;
        self.apply_recorded_track_effect_chain_mutation(
            track,
            "Replace audio effect",
            |app| {
                app.apply_compiled_effect_to_slot_sync(result, name, slot, track)?;
                app.retain_effect_source(FxChainLocator::Track(track), slot, source)
            },
        )
    }

    pub fn delete_custom_effect_slot_recorded(
        &mut self,
        track: usize,
        slot: usize,
    ) -> Result<(), String> {
        self.apply_recorded_track_effect_chain_mutation(
            track,
            "Delete audio effect",
            |app| app.graph_controller().delete_custom_effect_slot(track, slot),
        )
    }

    fn restore_track_effect_chain_state(
        &mut self,
        track: usize,
        current: &EffectChainState,
        target: &EffectChainState,
    ) -> Result<(), String> {
        if target.instances.len() > crate::lisp_host::MAX_CUSTOM_FX {
            return Err("Retained effect chain exceeds host capacity".to_string());
        }
        let locator = FxChainLocator::Track(track);
        let mut occupied = vec![false; crate::lisp_host::MAX_CUSTOM_FX];
        for instance in &current.instances {
            if let Some((_, slot)) = self.device_registry.audio_effect_location(instance.id) {
                if let Some(offset) = slot.checked_sub(BUILTIN_SLOT_COUNT) {
                    if offset < occupied.len() {
                        occupied[offset] = true;
                    }
                }
            }
        }
        let mut source_slots = Vec::with_capacity(target.instances.len());
        for desired in &target.instances {
            let current_instance = current.instances.iter().find(|item| item.id == desired.id);
            let existing_slot = self
                .device_registry
                .audio_effect_location(desired.id)
                .and_then(|(owner, slot)| {
                    (owner == self.track_registry.id_at(track)?).then_some(slot)
                });
            let slot = match existing_slot {
                Some(slot) => slot,
                None => {
                    let offset = occupied
                        .iter()
                        .position(|occupied| !*occupied)
                        .ok_or_else(|| "No temporary effect slot is available for restore".to_string())?;
                    occupied[offset] = true;
                    BUILTIN_SLOT_COUNT + offset
                }
            };
            if current_instance.map(|item| &item.source) != Some(&desired.source) {
                match &desired.source {
                    RetainedEffectSource::NativeBuiltin { name } => {
                        self.load_builtin_effect_to_slot_sync(track, slot, name)?;
                    }
                    RetainedEffectSource::Compiled {
                        name,
                        source,
                        asset_base,
                        origin,
                    } => {
                        let result = self.editor.dylib_cache.acquire(
                            crate::lisp_host::DGenCompileKind::Effect,
                            *origin,
                            source,
                            self.graph.sample_rate,
                            asset_base.as_deref(),
                        )?;
                        let ir_slots = crate::effects::conv_reverb::StereoIrSlots::from_manifest(
                            &result.manifest,
                        );
                        self.apply_compiled_effect_to_slot_sync(result, name, slot, track)?;
                        self.retain_effect_source(locator, slot, desired.source.clone())?;
                        if let Some(ir_slots) = ir_slots {
                            let node_id = self.state.pattern.effect_chains[track][slot]
                                .node_id
                                .load(Ordering::Relaxed) as i32;
                            crate::effects::conv_reverb::record_ir_slots(node_id, ir_slots);
                        }
                    }
                }
            }
            source_slots.push(slot);
        }

        let old_host = self.fx_chain_host(locator)?;
        let desired_snapshots = source_slots
            .iter()
            .map(|slot| EffectSlotSnapshot::capture(&self.state.pattern.effect_chains[track][*slot]))
            .collect::<Vec<_>>();
        let desired_node_ids = desired_snapshots
            .iter()
            .map(|slot| (slot.node_id, slot.modulator_node_id))
            .collect::<Vec<_>>();
        let desired_nodes = desired_snapshots
            .iter()
            .map(|slot| slot.node_id)
            .collect::<std::collections::HashSet<_>>();
        let removed_nodes = old_host
            .slots
            .iter()
            .skip(BUILTIN_SLOT_COUNT)
            .filter(|slot| slot.node_id > 0 && !desired_nodes.contains(&(slot.node_id as u32)))
            .copied()
            .collect::<Vec<_>>();
        for offset in 0..crate::lisp_host::MAX_CUSTOM_FX {
            let slot = BUILTIN_SLOT_COUNT + offset;
            if let Some((instance, snapshot)) = target.instances.get(offset).zip(desired_snapshots.get(offset)) {
                self.graph.effect_descriptors[track][slot] = instance.descriptor.clone();
                snapshot.restore(&self.state.pattern.effect_chains[track][slot]);
            } else {
                self.graph.effect_descriptors[track][slot] = EffectDescriptor::empty_custom_slot();
                self.state.pattern.effect_chains[track][slot].clear();
            }
        }
        let new_host = self.fx_chain_host(locator)?;
        let batch = FxGraphEditBatch::new(self.graph.lg.0);
        rewire_fx_chain(self.graph.lg.0, &old_host, &new_host);
        for slot in removed_nodes {
            unsafe {
                crate::audiograph::delete_node(self.graph.lg.0, slot.node_id);
                if slot.modulator_node_id > 0 {
                    crate::audiograph::delete_node(self.graph.lg.0, slot.modulator_node_id);
                }
            }
            crate::effects::conv_reverb::clear_instance(slot.node_id);
        }
        self.editor
            .effect_chain_leases
            .remap_slots(locator, &source_slots, batch.serial)?;
        drop(batch);

        let mut descriptors = target
            .instances
            .iter()
            .map(|instance| instance.descriptor.clone())
            .collect::<Vec<_>>();
        descriptors.resize_with(crate::lisp_host::MAX_CUSTOM_FX, EffectDescriptor::empty_custom_slot);
        let mut node_ids = desired_node_ids;
        node_ids.resize(crate::lisp_host::MAX_CUSTOM_FX, (0, 0));
        let patterns = target
            .pattern_slots
            .iter()
            .map(|pattern| (pattern.pattern, pattern.values.clone()))
            .collect::<Vec<_>>();
        self.state.restore_track_effect_chain_values(
            track,
            BUILTIN_SLOT_COUNT,
            &descriptors,
            &node_ids,
            &patterns,
        )?;
        self.state.restore_track_effect_binding_state(
            track,
            &target.bindings,
            &self.graph.effect_descriptors[track],
        )?;
        if let Some(pattern) = self.state.effective_track_pattern_id(track) {
            if let Some(saved) = target.pattern_slots.iter().find(|saved| saved.pattern == pattern) {
                for (offset, values) in saved.values.iter().enumerate() {
                    if let (Some(reference), Some(ir)) = (&values.ir, &values.prepared_ir) {
                        self.restore_prepared_track_effect_ir(
                            track,
                            BUILTIN_SLOT_COUNT + offset,
                            reference,
                            ir.clone(),
                        )?;
                    }
                }
            }
        }
        let ids = target.instances.iter().map(|instance| instance.id).collect::<Vec<_>>();
        let track_id = self.track_registry.id_at(track)
            .ok_or_else(|| format!("Track {} has no stable identity", track + 1))?;
        self.device_registry.bind_audio_effect_chain(track_id, BUILTIN_SLOT_COUNT, &ids)?;
        self.macro_engine
            .restore_effect_mappings_for_track(track, &target.macro_mappings)
            .map_err(|error| format!("{error:?}"))?;
        let state = std::sync::Arc::clone(&self.state);
        let effect_descriptors = &self.graph.effect_descriptors;
        let instrument_descriptors = &self.graph.instrument_descriptors;
        let buses = &self.buses;
        self.macro_engine.revalidate_mappings(|scope, target| {
            super::projects::resolve_live_macro_target(
                &state,
                effect_descriptors,
                instrument_descriptors,
                buses,
                scope,
                target,
            )
        });
        self.state.publish_macro_overrides(self.macro_engine.override_snapshot());
        self.refresh_effect_sidechain_labels();
        self.sync_scratch_runtime_descriptors();
        self.push_all_restored_defaults();
        self.state.publish_scheduler_snapshot();
        Ok(())
    }
    pub fn capture_track_instrument_state(
        &mut self,
        track: usize,
    ) -> Result<TrackInstrumentState, String> {
        let source = match self.graph.track_instrument_types.get(track).copied() {
            Some(crate::sequencer::InstrumentType::Custom) => {
                let engine_id = self
                    .graph
                    .track_engine_ids
                    .get(track)
                    .and_then(|engine_id| *engine_id)
                    .ok_or_else(|| format!("Custom track {} has no engine binding", track + 1))?;
                if self.editor.engine_registry.get(engine_id).is_none() {
                    return Err(format!(
                        "Custom track {} references missing retained engine {}",
                        track + 1,
                        engine_id
                    ));
                }
                TrackInstrumentSource::Custom { engine_id }
            }
            Some(crate::sequencer::InstrumentType::Sampler) => {
                TrackInstrumentSource::Sampler {
                    buffer_id: *self
                        .graph
                        .track_buffer_ids
                        .get(track)
                        .ok_or_else(|| format!("Sampler track {} has no buffer", track + 1))?,
                    sample_rate: *self
                        .graph
                        .track_sample_rates
                        .get(track)
                        .ok_or_else(|| {
                            format!("Sampler track {} has no sample rate", track + 1)
                        })?,
                    path: self.sampler_path_for_track(track),
                }
            }
            Some(crate::sequencer::InstrumentType::Rack) => {
                let track_id = self.track_registry.id_at(track)
                    .ok_or_else(|| format!("Track {} has no stable identity", track + 1))?;
                let slot_count = self.state.pattern.rack_tracks.lock().unwrap()
                    .get(track).and_then(Option::as_ref)
                    .map(|rack| rack.slots.len())
                    .ok_or_else(|| format!("Rack track {} has no rack state", track + 1))?;
                let mut slots = Vec::with_capacity(slot_count);
                for slot_index in 0..slot_count {
                    let id = self.device_registry.rack_slot(track_id, slot_index);
                    let effects = self.capture_rack_effect_chain_state(track, slot_index, None)?;
                    slots.push(RackContainerSlotState { id, effects });
                }
                TrackInstrumentSource::Rack { slots }
            }
            Some(crate::sequencer::InstrumentType::Modulator) => {
                TrackInstrumentSource::Modulator
            }
            None => return Err(format!("Track {} does not exist", track + 1)),
        };
        Ok(TrackInstrumentState {
            source,
            display_name: self.tracks[track].clone(),
            patterns: self.state.capture_track_instrument_pattern_state(track)?,
            macro_mappings: self
                .macro_engine
                .capture_instrument_mappings_for_track(track),
        })
    }

    pub fn commit_created_track(
        &mut self,
        track: usize,
        label: &'static str,
    ) -> Result<(), String> {
        finish_active_gesture(self);
        let track_id = self.track_registry.id_at(track)
            .ok_or_else(|| format!("Track {} has no stable identity", track + 1))?;
        let state = match self.capture_track_instrument_state(track) {
            Ok(state) => state,
            Err(error) => {
                let rollback = self.graph_controller().delete_track(track);
                return match rollback {
                    Ok(_) => Err(format!(
                        "New track was rolled back because history capture failed: {error}"
                    )),
                    Err(rollback_error) => Err(format!(
                        "New-track history capture failed ({error}); rollback also failed ({rollback_error})"
                    )),
                };
            }
        };
        let patch = TrackCreationPatch {
            track: track_id,
            state,
            color: self.track_colors.get(track).copied(),
            collapsed: self.track_collapsed.get(track).copied().unwrap_or(false),
            group: self.groups.iter()
                .find(|group| group.members.contains(&track))
                .map(|group| (group.id, group.bus_id)),
        };
        let retained_bytes = patch.retained_bytes();
        self.history.commit(label, None, EditPatch::TrackCreation(patch), retained_bytes);
        Ok(())
    }

    pub fn apply_recorded_instrument_binding_mutation<T>(
        &mut self,
        track: usize,
        label: &'static str,
        mutate: impl FnOnce(&mut App) -> Result<T, String>,
    ) -> Result<T, String> {
        finish_active_gesture(self);
        let track_id = self
            .track_registry
            .id_at(track)
            .ok_or_else(|| format!("Track {} has no stable identity", track + 1))?;
        let before = self.capture_track_instrument_state(track)?;
        let result = match mutate(self) {
            Ok(result) => result,
            Err(error) => {
                return match self.restore_track_instrument_state(track, &before) {
                    Ok(()) => Err(error),
                    Err(rollback_error) => Err(format!(
                        "Instrument replacement failed ({error}); restoring the original container also failed ({rollback_error})"
                    )),
                };
            }
        };
        let after = match self.capture_track_instrument_state(track) {
            Ok(after) => after,
            Err(capture_error) => {
                return match self.restore_track_instrument_state(track, &before) {
                    Ok(()) => Err(format!(
                        "Instrument replacement was rolled back because its after-state could not be captured: {capture_error}"
                    )),
                    Err(rollback_error) => Err(format!(
                        "Instrument replacement after-state capture failed ({capture_error}); rollback also failed ({rollback_error})"
                    )),
                };
            }
        };
        let patch = InstrumentBindingPatch {
            track: track_id,
            before,
            after,
        };
        let retained_bytes = patch.retained_bytes();
        self.history.commit(
            label,
            None,
            EditPatch::InstrumentBinding(patch),
            retained_bytes,
        );
        Ok(result)
    }

    pub fn apply_recorded_scene_structure_mutation<T>(
        &mut self,
        label: &'static str,
        mutate: impl FnOnce(&mut App) -> Result<T, String>,
    ) -> Result<T, String> {
        finish_active_gesture(self);
        self.save_current_bus_pattern();
        if !self.state.save_current_pattern_snapshot(
            self.tracks.len(),
            &self.graph.track_buffer_ids,
            &self.graph.track_sample_rates,
            &self.tracks,
            &self.graph.track_instrument_types,
        ) {
            return Err("Could not snapshot the current scene before editing scenes".to_string());
        }
        let before = self.state.capture_project_scenes();
        let result = match mutate(self) {
            Ok(result) => result,
            Err(error) => {
                return match self.restore_scene_structure_state(&before) {
                    Ok(()) => Err(error),
                    Err(rollback_error) => Err(format!(
                        "Scene edit failed ({error}); restoring its before-state also failed ({rollback_error})"
                    )),
                };
            }
        };
        let after = self.state.capture_project_scenes();
        let patch = SceneStructurePatch { before, after };
        let retained_bytes = patch.retained_bytes();
        self.history.commit(label, None, EditPatch::SceneStructure(patch), retained_bytes);
        Ok(result)
    }

    pub fn apply_recorded_track_collapsed(
        &mut self,
        collapsed: Vec<bool>,
    ) -> Result<bool, String> {
        finish_active_gesture(self);
        self.normalize_track_colors();
        self.normalize_track_collapsed();
        if collapsed.len() != self.tracks.len() {
            return Err("Collapsed-track state does not match the track topology".to_string());
        }
        let changes = collapsed.iter().enumerate().filter_map(|(track, target)| {
            let before = TrackPresentationState {
                color: self.track_colors[track],
                collapsed: self.track_collapsed[track],
            };
            if before.collapsed == *target {
                return None;
            }
            let after = TrackPresentationState {
                color: before.color,
                collapsed: *target,
            };
            Some(TrackPresentationChange {
                track: self.track_registry.id_at(track)?,
                before,
                after,
            })
        }).collect::<Vec<_>>();
        if changes.is_empty() {
            return Ok(false);
        }
        self.replace_track_collapsed(collapsed);
        let patch = TrackPresentationPatch { changes };
        let retained_bytes = patch.retained_bytes();
        self.history.commit(
            "Toggle track collapse",
            None,
            EditPatch::TrackPresentation(patch),
            retained_bytes,
        );
        Ok(true)
    }

    fn restore_track_presentation(
        &mut self,
        patch: &TrackPresentationPatch,
        mode: ApplyMode,
    ) -> Result<(), String> {
        let mut resolved = Vec::with_capacity(patch.changes.len());
        for change in &patch.changes {
            let track = self.track_registry.index_of(change.track)
                .ok_or_else(|| format!("Track {:?} no longer exists", change.track))?;
            resolved.push((track, match mode {
                ApplyMode::Undo => &change.before,
                ApplyMode::Redo => &change.after,
                ApplyMode::UserEdit | ApplyMode::ProjectLoad => {
                    return Err("track-presentation replay requires undo or redo mode".to_string());
                }
            }));
        }
        for (track, target) in resolved {
            self.track_colors[track] = target.color;
            self.track_collapsed[track] = target.collapsed;
        }
        Ok(())
    }

    fn restore_scene_structure_state(
        &mut self,
        target: &crate::sequencer::ProjectScenes,
    ) -> Result<(), String> {
        let sample_ids = self.state.restore_project_scenes(target)?;
        self.graph_controller().apply_sample_ids(&sample_ids);
        self.graph_controller().sync_current_pattern_mod_routes();
        self.graph_controller().sync_track_instrument_run_modes_from_live_state()?;
        let default_bus_snapshot = self.capture_bus_pattern_snapshot();
        let bus_snapshot = self.state.bus_pattern_snapshot_or_default(
            target.current_scene,
            &default_bus_snapshot,
        );
        self.restore_bus_pattern_snapshot(&bus_snapshot);
        self.push_all_restored_defaults();
        Ok(())
    }

    fn restore_track_instrument_state(
        &mut self,
        track: usize,
        target: &TrackInstrumentState,
    ) -> Result<(), String> {
        if matches!(&target.source, TrackInstrumentSource::Rack { .. }) {
            return self.restore_rack_instrument_container_state(track, target);
        }
        let target_descriptor = match &target.source {
            TrackInstrumentSource::Custom { engine_id } => {
                let retained = self
                    .editor
                    .engine_registry
                    .get(*engine_id)
                    .ok_or_else(|| format!("Retained instrument engine {engine_id} is missing"))?;
                if self.editor.instrument_libs.get(retained.lib_index).is_none() {
                    return Err(format!(
                        "Retained instrument engine {engine_id} references missing library {}",
                        retained.lib_index
                    ));
                }
                crate::lisp_host::instrument_descriptor_from_manifest(
                    &retained.name,
                    &retained.manifest,
                )
            }
            TrackInstrumentSource::Sampler { buffer_id, .. } => {
                if *buffer_id < 0 {
                    return Err("Retained sampler buffer is invalid".to_string());
                }
                crate::effects::EffectDescriptor::builtin_sampler()
            }
            TrackInstrumentSource::Rack { .. } => unreachable!(),
            TrackInstrumentSource::Modulator => crate::track_modulator::descriptor(),
        };
        self.state.validate_track_instrument_pattern_state(
            track,
            &target.patterns,
            &target_descriptor,
        )?;
        for (macro_id, _) in &target.macro_mappings.mappings {
            if self.macro_engine.macro_definition(*macro_id).is_none() {
                return Err(format!(
                    "Instrument history references missing macro {macro_id}"
                ));
            }
        }
        match &target.source {
            TrackInstrumentSource::Custom { engine_id } => {
                let retained = self
                    .editor
                    .engine_registry
                    .get(*engine_id)
                    .cloned()
                    .ok_or_else(|| format!("Retained instrument engine {engine_id} is missing"))?;
                let lib_ptr: *const crate::lisp_host::LoadedDGenLib = self
                    .editor
                    .instrument_libs
                    .get(retained.lib_index)
                    .ok_or_else(|| {
                        format!(
                            "Retained instrument engine {engine_id} references missing library {}",
                            retained.lib_index
                        )
                    })?;
                unsafe {
                    self.graph_controller().replace_track_with_custom_instrument(
                        track,
                        &retained.name,
                        *engine_id,
                        &retained.manifest,
                        &*lib_ptr,
                        target.patterns.live.instrument_run_mode,
                    )?;
                }
                if let Some(path) = self.sampler_paths.get_mut(track) {
                    *path = None;
                }
            }
            TrackInstrumentSource::Sampler {
                buffer_id,
                sample_rate,
                path,
            } => {
                match self.graph.track_instrument_types.get(track).copied() {
                    Some(crate::sequencer::InstrumentType::Rack) => {
                        self.graph_controller().replace_rack_track_with_sampler(
                            track,
                            *buffer_id,
                            *sample_rate,
                            &target.display_name,
                        )?;
                    }
                    Some(crate::sequencer::InstrumentType::Custom) => {
                        self.graph_controller().convert_custom_track_to_sampler(
                            track,
                            *buffer_id,
                            *sample_rate,
                            &target.display_name,
                        )?;
                    }
                    Some(crate::sequencer::InstrumentType::Sampler) => {
                        self.graph_controller().send_sample_to_all_voices(
                            track,
                            *buffer_id,
                            *sample_rate,
                        );
                        self.graph.track_buffer_ids[track] = *buffer_id;
                        self.graph.track_sample_rates[track] = *sample_rate;
                        let nodes = &self.graph.track_node_ids[track];
                        let node_id = nodes
                            .sampler_ids
                            .first()
                            .copied()
                            .and_then(|id| u32::try_from(id).ok())
                            .unwrap_or(0);
                        let modulator_node_id = nodes
                            .sampler_modulator_ids
                            .first()
                            .copied()
                            .and_then(|id| u32::try_from(id).ok())
                            .unwrap_or(0);
                        let descriptor = self.graph.instrument_descriptors[track].clone();
                        self.state.reset_sampler_slot_all_patterns(
                            track,
                            &descriptor,
                            node_id,
                            modulator_node_id,
                            (*buffer_id, target.display_name.clone(), *sample_rate),
                        ).ok_or_else(|| {
                            format!("Sampler track {} could not reset its pattern state", track + 1)
                        })?;
                    }
                    Some(other) => {
                        return Err(format!(
                            "Track {} has instrument type {other:?}, which cannot restore a sampler",
                            track + 1
                        ));
                    }
                    None => return Err(format!("Track {} does not exist", track + 1)),
                }
                if let Some(sampler_path) = self.sampler_paths.get_mut(track) {
                    *sampler_path = path.clone();
                }
                if let Some(path) = path {
                    self.register_loaded_sample_path(
                        &target.display_name,
                        *buffer_id,
                        path.clone(),
                    );
                }
            }
            TrackInstrumentSource::Rack { .. } => unreachable!(),
            TrackInstrumentSource::Modulator => {
                if self.graph.track_instrument_types.get(track)
                    != Some(&crate::sequencer::InstrumentType::Modulator)
                {
                    return Err(format!("Track {} is not a modulator", track + 1));
                }
            }
        }
        self.tracks[track] = target.display_name.clone();
        let descriptor = self.graph.instrument_descriptors[track].clone();
        let (node_id, modulator_node_id) = match self.graph.track_instrument_types[track] {
            crate::sequencer::InstrumentType::Custom => {
                let engine_id = self.graph.track_engine_ids[track]
                    .ok_or_else(|| format!("Custom track {} lost its engine", track + 1))?;
                let engine = self.graph.engine_node_ids[engine_id]
                    .as_ref()
                    .ok_or_else(|| format!("Instrument engine {engine_id} has no runtime"))?;
                (
                    engine.synth_ids.first().copied(),
                    engine.modulator_ids.first().copied(),
                )
            }
            crate::sequencer::InstrumentType::Sampler => {
                let nodes = &self.graph.track_node_ids[track];
                (
                    nodes.sampler_ids.first().copied(),
                    nodes.sampler_modulator_ids.first().copied(),
                )
            }
            crate::sequencer::InstrumentType::Modulator => {
                let nodes = &self.graph.track_node_ids[track];
                (Some(nodes.mod_env_id), None)
            }
            other => {
                return Err(format!(
                    "Track {} restored unexpected instrument type {other:?}",
                    track + 1
                ));
            }
        };
        let node_id = node_id.and_then(|id| u32::try_from(id).ok()).unwrap_or(0);
        let modulator_node_id = modulator_node_id
            .and_then(|id| u32::try_from(id).ok())
            .unwrap_or(0);
        self.state.restore_track_instrument_pattern_state(
            track,
            &target.patterns,
            &descriptor,
            node_id,
            modulator_node_id,
        )?;
        self.macro_engine
            .restore_instrument_mappings_for_track(track, &target.macro_mappings)
            .map_err(|error| format!("{error:?}"))?;
        let state = std::sync::Arc::clone(&self.state);
        let effect_descriptors = &self.graph.effect_descriptors;
        let instrument_descriptors = &self.graph.instrument_descriptors;
        let buses = &self.buses;
        self.macro_engine.revalidate_mappings(|scope, target| {
            super::projects::resolve_live_macro_target(
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
        self.push_instrument_defaults_for_track(track);
        self.state.publish_scheduler_snapshot();
        Ok(())
    }

    fn restore_created_track(&mut self, patch: &TrackCreationPatch) -> Result<(), String> {
        if self.track_registry.index_of(patch.track).is_some() {
            return Err(format!("Track {:?} already exists", patch.track));
        }
        let track = match patch.state.source {
            TrackInstrumentSource::Modulator => self.graph_controller().add_modulator_track()?,
            _ => self.graph_controller().add_blank_sampler_track()?,
        };
        let allocated = self.track_registry.replace_at(track, patch.track)
            .map_err(|error| format!("Failed to restore stable track id: {error:?}"))?;
        self.device_registry.clear_track(allocated);
        if !matches!(patch.state.source, TrackInstrumentSource::Modulator) {
            self.restore_track_instrument_state(track, &patch.state)?;
        } else {
            self.tracks[track] = patch.state.display_name.clone();
            let descriptor = self.graph.instrument_descriptors[track].clone();
            let node_id = self.graph.track_node_ids[track].mod_env_id as u32;
            self.state.restore_track_instrument_pattern_state(
                track,
                &patch.state.patterns,
                &descriptor,
                node_id,
                0,
            )?;
        }
        if let Some(color) = patch.color {
            self.track_colors[track] = color;
        }
        self.track_collapsed[track] = patch.collapsed;
        if let Some((group_id, bus_id)) = patch.group {
            let group = self.groups.iter_mut().find(|group| group.id == group_id)
                .ok_or_else(|| format!("Track history group {group_id} no longer exists"))?;
            group.members.push(track);
            group.members.sort_unstable();
            group.members.dedup();
            self.set_track_output_all_scenes(
                track,
                crate::sequencer::TrackOutput::Bus(crate::sequencer::BusId(bus_id)),
            );
        }
        self.state.publish_scheduler_snapshot();
        Ok(())
    }

    fn remap_groups_after_track_delete(&mut self, deleted: usize) {
        for group in &mut self.groups {
            group.members.retain(|member| *member != deleted);
            for member in &mut group.members {
                if *member > deleted {
                    *member -= 1;
                }
            }
        }
        self.groups.retain(|group| !group.members.is_empty());
    }

    pub fn delete_track_recorded(&mut self, track: usize) -> Result<usize, String> {
        finish_active_gesture(self);
        if track >= self.tracks.len() {
            return Err(format!("Invalid track index {}", track + 1));
        }
        if self.tracks.len() <= 1 {
            return Err("Cannot delete the last remaining track".to_string());
        }
        let names = self.tracks.clone();
        let buffer_ids = self.graph.track_buffer_ids.clone();
        let sample_rates = self.graph.track_sample_rates.clone();
        let instrument_types = self.graph.track_instrument_types.clone();
        if !self.state.save_current_pattern_snapshot(
            self.tracks.len(),
            &buffer_ids,
            &sample_rates,
            &names,
            &instrument_types,
        ) {
            return Err("Could not snapshot the current scene before deleting the track".to_string());
        }
        let track_id = self.track_registry.id_at(track)
            .ok_or_else(|| format!("Track {} has no stable identity", track + 1))?;
        let instrument = self.capture_track_instrument_state(track)?;
        let effects = self.capture_track_effect_chain_state(track, None)?;
        let midi_fx = self.capture_track_midi_fx_chain_state(track)?;
        let patterns = self.state.capture_track_pattern_lane_state(
            track,
            &self.graph.effect_descriptors,
        )?;
        let patch = TrackDeletionPatch {
            track: track_id,
            index: track,
            instrument,
            effects,
            midi_fx,
            patterns,
            color: self.track_colors.get(track).copied(),
            collapsed: self.track_collapsed.get(track).copied().unwrap_or(false),
            rack_selected_slot: self.rack_selected_slots.get(track).copied().unwrap_or(0),
            rack_pad_bank_start: self.rack_pad_bank_starts.get(track).copied()
                .unwrap_or(crate::sequencer::DRUM_RACK_FIRST_PAD_NOTE),
            record_armed: self.graph.record_armed.get(track).copied().unwrap_or(false),
            groups: self.groups.clone(),
            macro_mappings: self.macro_engine.capture_track_topology_mappings(track),
        };
        let retained_bytes = patch.retained_bytes();
        let selected = self.graph_controller().delete_track(track)?;
        self.remap_groups_after_track_delete(track);
        self.macro_engine.remap_after_track_delete(track);
        self.device_registry.clear_track(track_id);
        self.history.commit(
            "Delete track",
            None,
            EditPatch::TrackDeletion(patch),
            retained_bytes,
        );
        Ok(selected)
    }

    fn restore_deleted_track(&mut self, patch: &TrackDeletionPatch) -> Result<(), String> {
        if self.track_registry.index_of(patch.track).is_some() {
            return Err(format!("Deleted track {:?} already exists", patch.track));
        }
        if patch.index > self.tracks.len() {
            return Err("Deleted track insertion point is no longer valid".to_string());
        }
        let appended = match patch.instrument.source {
            TrackInstrumentSource::Modulator => self.graph_controller().add_modulator_track()?,
            _ => self.graph_controller().add_blank_sampler_track()?,
        };
        let allocated = self.track_registry.replace_at(appended, patch.track)
            .map_err(|error| format!("Failed to restore stable track id: {error:?}"))?;
        self.device_registry.clear_track(allocated);
        let empty_effects = self.capture_track_effect_chain_state(appended, None)?;
        self.state.replace_appended_track_pattern_lane(&patch.patterns)?;
        self.restore_track_instrument_state(appended, &patch.instrument)?;
        self.restore_track_effect_chain_state(appended, &empty_effects, &patch.effects)?;
        self.restore_track_midi_fx_chain_state(appended, &patch.midi_fx)?;
        self.track_colors[appended] = patch.color.unwrap_or(self.track_colors[appended]);
        self.track_collapsed[appended] = patch.collapsed;
        self.rack_selected_slots[appended] = patch.rack_selected_slot;
        self.rack_pad_bank_starts[appended] = patch.rack_pad_bank_start;
        self.graph.record_armed[appended] = patch.record_armed;
        self.state.move_appended_track_pattern_lane_to(patch.index, &patch.patterns)?;
        self.graph_controller().move_appended_track_to(patch.index)?;
        self.groups = patch.groups.clone();
        self.macro_engine.restore_track_topology_mappings(&patch.macro_mappings)
            .map_err(|error| format!("Could not restore track macro mappings: {error:?}"))?;
        self.state.publish_macro_overrides(self.macro_engine.override_snapshot());
        self.ui.cursor_track = patch.index;
        self.ui.cursor_step = self.ui.cursor_step.min(
            self.state.pattern.track_params[patch.index].get_num_steps().saturating_sub(1),
        );
        self.push_all_restored_defaults();
        self.state.publish_scheduler_snapshot();
        Ok(())
    }

    fn restore_rack_instrument_container_state(
        &mut self,
        track: usize,
        target: &TrackInstrumentState,
    ) -> Result<(), String> {
        let TrackInstrumentSource::Rack { slots } = &target.source else {
            return Err("Rack-container restore requires rack history state".to_string());
        };
        let target_rack = target.patterns.live.rack_track.as_ref()
            .ok_or_else(|| "Rack history has no live rack state".to_string())?;
        if target_rack.slots.len() != slots.len() {
            return Err("Rack history slot identities do not match its rack state".to_string());
        }
        for (_, pattern) in &target.patterns.patterns {
            if pattern.rack_track.as_ref().is_none_or(|rack| rack.slots.len() != slots.len()) {
                return Err("Rack history topology differs between track patterns".to_string());
            }
        }
        if !matches!(
            self.graph.track_instrument_types.get(track),
            Some(
                crate::sequencer::InstrumentType::Rack
                    | crate::sequencer::InstrumentType::Sampler
                    | crate::sequencer::InstrumentType::Custom
            )
        ) {
            return Err(
                "Rack-container history can replay only onto a replaceable instrument track"
                    .to_string(),
            );
        }

        let mut restored_patterns = target.patterns.clone();
        let clear_effect_runtime = |rack: &mut crate::sequencer::RackTrackSnapshot| {
            for slot in &mut rack.slots {
                for effect in &mut slot.effect_slots {
                    effect.node_id = 0;
                    effect.modulator_node_id = 0;
                }
            }
        };
        clear_effect_runtime(
            restored_patterns.live.rack_track.as_mut()
                .expect("live rack was validated"),
        );
        for (_, pattern) in &mut restored_patterns.patterns {
            clear_effect_runtime(
                pattern.rack_track.as_mut().expect("pattern rack was validated"),
            );
        }
        let rack = restored_patterns.live.rack_track.clone()
            .expect("live rack was validated");
        self.graph_controller()
            .replace_track_instrument_container_with_rack(track, rack, &target.display_name)?;
        let empty_descriptor = EffectDescriptor::empty_custom_slot();
        self.state.restore_track_instrument_pattern_state(
            track,
            &restored_patterns,
            &empty_descriptor,
            0,
            0,
        )?;
        let bindings = (0..slots.len())
            .map(|slot| self.rack_slot_live_binding(track, slot))
            .collect::<Result<Vec<_>, _>>()?;
        if !self.state.sync_rack_slot_instrument_bindings_for_all_patterns(track, &bindings) {
            return Err("Failed to restore rack instrument bindings across patterns".to_string());
        }

        let track_id = self.track_registry.id_at(track)
            .ok_or_else(|| format!("Track {} has no stable identity", track + 1))?;
        self.device_registry.clear_rack_track(track_id);
        for (slot_index, slot) in slots.iter().enumerate() {
            self.device_registry.bind_rack_slot(track_id, slot_index, slot.id)?;
            let current = self.capture_rack_effect_chain_state(track, slot_index, None)?;
            self.restore_rack_effect_chain_state(track, slot_index, &current, &slot.effects)?;
        }
        self.tracks[track] = target.display_name.clone();
        self.macro_engine
            .restore_instrument_mappings_for_track(track, &target.macro_mappings)
            .map_err(|error| format!("{error:?}"))?;
        let state = std::sync::Arc::clone(&self.state);
        let effect_descriptors = &self.graph.effect_descriptors;
        let instrument_descriptors = &self.graph.instrument_descriptors;
        let buses = &self.buses;
        self.macro_engine.revalidate_mappings(|scope, target| {
            super::projects::resolve_live_macro_target(
                &state,
                effect_descriptors,
                instrument_descriptors,
                buses,
                scope,
                target,
            )
        });
        self.state.publish_macro_overrides(self.macro_engine.override_snapshot());
        self.push_all_restored_defaults();
        self.state.publish_scheduler_snapshot();
        Ok(())
    }

    fn rack_slot_live_binding(
        &self,
        track: usize,
        slot_index: usize,
    ) -> Result<(crate::effects::EffectDescriptor, u32, u32), String> {
        let rack = self
            .state
            .pattern
            .rack_tracks
            .lock()
            .unwrap()
            .get(track)
            .and_then(Option::as_ref)
            .and_then(|rack| rack.slots.get(slot_index))
            .cloned()
            .ok_or_else(|| format!("Rack slot {} is missing", slot_index + 1))?;
        let nodes = self
            .graph
            .track_node_ids
            .get(track)
            .and_then(|nodes| nodes.rack_slots.get(slot_index))
            .ok_or_else(|| format!("Rack slot {} has no graph nodes", slot_index + 1))?;
        let (descriptor, node, modulator) = match rack.instrument_type {
            crate::sequencer::InstrumentType::Sampler => (
                crate::effects::EffectDescriptor::builtin_sampler(),
                nodes.sampler_ids.first().copied(),
                nodes.sampler_modulator_ids.first().copied(),
            ),
            crate::sequencer::InstrumentType::Custom => {
                let engine_id = rack
                    .track_sound_state
                    .engine_id
                    .ok_or_else(|| "Rack instrument has no retained engine".to_string())?;
                let descriptor = self
                    .editor
                    .engine_registry
                    .get_instrument_descriptor(engine_id)
                    .cloned()
                    .ok_or_else(|| format!("Rack engine {engine_id} has no descriptor"))?;
                let engine = self
                    .graph
                    .engine_node_ids
                    .get(engine_id)
                    .and_then(Option::as_ref)
                    .ok_or_else(|| format!("Rack engine {engine_id} has no runtime"))?;
                (
                    descriptor,
                    engine.synth_ids.first().copied(),
                    engine.modulator_ids.first().copied(),
                )
            }
            other => {
                return Err(format!(
                    "Rack slot {} has unsupported source type {other:?}",
                    slot_index + 1
                ));
            }
        };
        Ok((
            descriptor,
            node.and_then(|id| u32::try_from(id).ok()).unwrap_or(0),
            modulator
                .and_then(|id| u32::try_from(id).ok())
                .unwrap_or(0),
        ))
    }

    fn materialize_rack_slot_source(
        &mut self,
        track: usize,
        slot_index: usize,
        target: &crate::sequencer::RackSlotPatternStateSnapshot,
        append: bool,
    ) -> Result<(), String> {
        if append {
            self.state
                .validate_rack_slot_append_pattern_state(track, target)?;
        } else {
            self.state.validate_rack_slot_pattern_state(track, target)?;
        }
        let slot = &target.live;
        let materialized_index = match slot.instrument_type {
            crate::sequencer::InstrumentType::Sampler => {
                let (buffer_id, sample_name, sample_rate) = slot
                    .sample_id
                    .as_ref()
                    .ok_or_else(|| "Rack sampler history is missing sample metadata".to_string())?;
                if append {
                    self.graph_controller().add_sampler_slot_to_rack_buffer(
                        track,
                        *buffer_id,
                        *sample_rate,
                        sample_name,
                    )?
                } else {
                    self.graph_controller().replace_rack_slot_with_sampler_buffer(
                        track,
                        slot_index,
                        *buffer_id,
                        *sample_rate,
                        sample_name,
                    )?;
                    slot_index
                }
            }
            crate::sequencer::InstrumentType::Custom => {
                let engine_id = slot
                    .track_sound_state
                    .engine_id
                    .ok_or_else(|| "Rack instrument history is missing its engine".to_string())?;
                let retained = self
                    .editor
                    .engine_registry
                    .get(engine_id)
                    .cloned()
                    .ok_or_else(|| format!("Retained rack engine {engine_id} is missing"))?;
                let lib_ptr: *const crate::lisp_host::LoadedDGenLib = self
                    .editor
                    .instrument_libs
                    .get(retained.lib_index)
                    .ok_or_else(|| {
                        format!("Retained rack engine {engine_id} has no loaded library")
                    })?;
                if append {
                    unsafe {
                        self.graph_controller().add_custom_slot_to_rack(
                            track,
                            &retained.name,
                            engine_id,
                            &retained.manifest,
                            &*lib_ptr,
                            slot.instrument_run_mode,
                        )?
                    }
                } else {
                    self.graph_controller().replace_rack_slot_with_custom(
                        track,
                        slot_index,
                        &retained.name,
                        engine_id,
                        &retained.manifest,
                        slot.instrument_run_mode,
                    )?;
                    slot_index
                }
            }
            other => {
                return Err(format!(
                    "Rack history cannot materialize source type {other:?}"
                ));
            }
        };
        if materialized_index != slot_index {
            return Err(format!(
                "Rack slot replay appended at {}, expected {}",
                materialized_index + 1,
                slot_index + 1
            ));
        }
        let (descriptor, node_id, modulator_node_id) =
            self.rack_slot_live_binding(track, slot_index)?;
        self.state.restore_rack_slot_pattern_state(
            track,
            target,
            &descriptor,
            node_id,
            modulator_node_id,
        )?;
        self.push_all_restored_defaults();
        self.state.publish_scheduler_snapshot();
        Ok(())
    }

    pub fn apply_recorded_rack_slot_add(
        &mut self,
        track: usize,
        label: &'static str,
        mutate: impl FnOnce(&mut App) -> Result<usize, String>,
    ) -> Result<usize, String> {
        finish_active_gesture(self);
        let track_id = self
            .track_registry
            .id_at(track)
            .ok_or_else(|| format!("Track {} has no stable identity", track + 1))?;
        let slot_index = mutate(self)?;
        let slot_id = self.device_registry.rack_slot(track_id, slot_index);
        let after = match self.state.capture_rack_slot_pattern_state(track, slot_index) {
            Ok(after) => after,
            Err(error) => {
                let _ = self.graph_controller().delete_rack_slot(track, slot_index);
                return Err(format!(
                    "Rack slot was rolled back because history capture failed: {error}"
                ));
            }
        };
        let patch = RackSlotStructurePatch {
            track: track_id,
            slot: slot_id,
            slot_index,
            edit: RackSlotStructureEdit::Add { after },
        };
        let retained_bytes = patch.retained_bytes();
        self.history.commit(
            label,
            None,
            EditPatch::RackSlotStructure(patch),
            retained_bytes,
        );
        Ok(slot_index)
    }

    pub fn apply_recorded_rack_slot_source_replacement(
        &mut self,
        track: usize,
        slot_index: usize,
        label: &'static str,
        mutate: impl FnOnce(&mut App) -> Result<(), String>,
    ) -> Result<(), String> {
        finish_active_gesture(self);
        let track_id = self
            .track_registry
            .id_at(track)
            .ok_or_else(|| format!("Track {} has no stable identity", track + 1))?;
        let slot_id = self.device_registry.rack_slot(track_id, slot_index);
        let before = self.state.capture_rack_slot_pattern_state(track, slot_index)?;
        mutate(self)?;
        let after = match self.state.capture_rack_slot_pattern_state(track, slot_index) {
            Ok(after) => after,
            Err(capture_error) => {
                return match self.materialize_rack_slot_source(
                    track,
                    slot_index,
                    &before,
                    false,
                ) {
                    Ok(()) => Err(format!(
                        "Rack replacement was rolled back because history capture failed: {capture_error}"
                    )),
                    Err(rollback_error) => Err(format!(
                        "Rack replacement capture failed ({capture_error}); rollback also failed ({rollback_error})"
                    )),
                };
            }
        };
        let patch = RackSlotStructurePatch {
            track: track_id,
            slot: slot_id,
            slot_index,
            edit: RackSlotStructureEdit::ReplaceSource { before, after },
        };
        let retained_bytes = patch.retained_bytes();
        self.history.commit(
            label,
            None,
            EditPatch::RackSlotStructure(patch),
            retained_bytes,
        );
        Ok(())
    }
}

impl StepGestureTransaction {
    pub fn begin(
        app: &App,
        track: usize,
        steps: &[usize],
        label: &'static str,
    ) -> Result<Self, EditError> {
        let track_id = app
            .track_registry
            .id_at(track)
            .ok_or(EditError::TrackOutOfRange { track })?;
        let pattern_id = app
            .state
            .effective_track_pattern_id(track)
            .ok_or(EditError::MissingTrackPattern)?;
        let steps = normalized_steps(steps);
        if steps.is_empty() {
            return Err(EditError::InvalidStepRange);
        }
        let (cells, variant_registry_before) = app
            .state
            .capture_pattern_step_cells(track, pattern_id, &steps)
            .map_err(EditError::ReplayFailed)?;
        Ok(Self {
            target: TrackPatternId {
                track: track_id,
                pattern: pattern_id,
            },
            label,
            before: steps.into_iter().zip(cells).collect(),
            variant_registry_before,
        })
    }

    pub fn capture_additional_steps(
        &mut self,
        app: &App,
        steps: &[usize],
    ) -> Result<(), EditError> {
        let track = app
            .track_registry
            .index_of(self.target.track)
            .ok_or(EditError::MissingStableTrack {
                track: self.target.track,
            })?;
        if app.state.effective_track_pattern_id(track) != Some(self.target.pattern) {
            return Err(EditError::MissingTrackPattern);
        }
        let missing = normalized_steps(steps)
            .into_iter()
            .filter(|step| !self.before.contains_key(step))
            .collect::<Vec<_>>();
        if missing.is_empty() {
            return Ok(());
        }
        let (cells, _) = app
            .state
            .capture_pattern_step_cells(track, self.target.pattern, &missing)
            .map_err(EditError::ReplayFailed)?;
        self.before.extend(missing.into_iter().zip(cells));
        Ok(())
    }

    pub fn rollback(self, app: &mut App) -> Result<(), EditError> {
        let track = app
            .track_registry
            .index_of(self.target.track)
            .ok_or(EditError::MissingStableTrack {
                track: self.target.track,
            })?;
        let cells = self.before.into_iter().collect::<Vec<_>>();
        let publish = app
            .state
            .restore_pattern_step_cells_no_publish(
                track,
                self.target.pattern,
                &cells,
                &self.variant_registry_before,
            )
            .map_err(EditError::ReplayFailed)?;
        if publish {
            app.state.publish_scheduler_snapshot();
        }
        Ok(())
    }

    pub fn commit(self, app: &mut App) -> Result<EditOutcome, EditError> {
        let track = app
            .track_registry
            .index_of(self.target.track)
            .ok_or(EditError::MissingStableTrack {
                track: self.target.track,
            })?;
        let steps = self.before.keys().copied().collect::<Vec<_>>();
        let (after, _) = match app
            .state
            .capture_pattern_step_cells(track, self.target.pattern, &steps)
        {
            Ok(after) => after,
            Err(error) => {
                return match self.rollback(app) {
                    Ok(()) => Err(EditError::ReplayFailed(error)),
                    Err(rollback_error) => Err(EditError::ReplayFailed(format!(
                        "{error}; rollback also failed: {rollback_error:?}"
                    ))),
                };
            }
        };
        let cells = self
            .before
            .into_iter()
            .zip(after)
            .filter_map(|((step, before), after)| {
                (!step_snapshot_bit_exact_eq(&before, &after)).then_some(StepCellDelta {
                    step,
                    before,
                    after,
                })
            })
            .collect::<Vec<_>>();
        if cells.is_empty() {
            return Ok(EditOutcome::NoOp);
        }
        app.state.reconcile_plock_variant_registry_for_track(track);
        let (_, variant_registry_after) = match app
            .state
            .capture_pattern_step_cells(track, self.target.pattern, &steps)
        {
            Ok(after) => after,
            Err(error) => {
                let before_cells = cells
                    .iter()
                    .map(|cell| (cell.step, cell.before.clone()))
                    .collect::<Vec<_>>();
                return match app.state.restore_pattern_step_cells_no_publish(
                    track,
                    self.target.pattern,
                    &before_cells,
                    &self.variant_registry_before,
                ) {
                    Ok(publish) => {
                        if publish {
                            app.state.publish_scheduler_snapshot();
                        }
                        Err(EditError::ReplayFailed(error))
                    }
                    Err(rollback_error) => Err(EditError::ReplayFailed(format!(
                        "{error}; rollback also failed: {rollback_error}"
                    ))),
                };
            }
        };
        let patch = StepCellsPatch {
            target: self.target,
            cells,
            variant_registry_before: self.variant_registry_before,
            variant_registry_after,
        };
        let after_cells = patch
            .cells
            .iter()
            .map(|cell| (cell.step, cell.after.clone()))
            .collect::<Vec<_>>();
        if let Err(error) = app.state.restore_pattern_step_cells_no_publish(
            track,
            patch.target.pattern,
            &after_cells,
            &patch.variant_registry_after,
        ) {
            let before_cells = patch
                .cells
                .iter()
                .map(|cell| (cell.step, cell.before.clone()))
                .collect::<Vec<_>>();
            return match app.state.restore_pattern_step_cells_no_publish(
                track,
                patch.target.pattern,
                &before_cells,
                &patch.variant_registry_before,
            ) {
                Ok(publish) => {
                    if publish {
                        app.state.publish_scheduler_snapshot();
                    }
                    Err(EditError::ReplayFailed(error))
                }
                Err(rollback_error) => Err(EditError::ReplayFailed(format!(
                    "{error}; rollback also failed: {rollback_error}"
                ))),
            };
        }
        let retained_bytes = patch.retained_bytes();
        finish_active_gesture(app);
        let history_move = app.history.commit(
            self.label,
            None,
            EditPatch::StepCells(patch),
            retained_bytes,
        );
        Ok(EditOutcome::Applied(history_move))
    }
}

#[derive(Clone)]
enum BarrierWitness {
    Bytes(Vec<u8>),
    Steps {
        num_steps: usize,
        cells: Vec<StepCellSnapshot>,
    },
    Macros {
        next_id: u32,
        definitions: Vec<Macro>,
    },
}

impl BarrierWitness {
    fn bit_exact_eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Bytes(left), Self::Bytes(right)) => left == right,
            (
                Self::Steps {
                    num_steps: left_num_steps,
                    cells: left,
                },
                Self::Steps {
                    num_steps: right_num_steps,
                    cells: right,
                },
            ) => {
                left_num_steps == right_num_steps
                    && left.len() == right.len()
                    && left
                        .iter()
                        .zip(right)
                        .all(|(left, right)| step_snapshot_bit_exact_eq(left, right))
            }
            (
                Self::Macros {
                    next_id: left_next_id,
                    definitions: left,
                },
                Self::Macros {
                    next_id: right_next_id,
                    definitions: right,
                },
            ) => {
                left_next_id == right_next_id
                    && left.len() == right.len()
                    && left
                        .iter()
                        .zip(right)
                        .all(|(left, right)| macro_bit_exact_eq(left, right))
            }
            _ => false,
        }
    }
}

#[derive(Default)]
struct WitnessBytes(Vec<u8>);

impl WitnessBytes {
    fn u64(&mut self, value: u64) {
        self.0.extend_from_slice(&value.to_le_bytes());
    }

    fn usize(&mut self, value: usize) {
        self.u64(value as u64);
    }

    fn u32(&mut self, value: u32) {
        self.0.extend_from_slice(&value.to_le_bytes());
    }

    fn bool(&mut self, value: bool) {
        self.0.push(u8::from(value));
    }

    fn f32(&mut self, value: f32) {
        self.u32(value.to_bits());
    }

    fn optional_f32(&mut self, value: Option<f32>) {
        self.bool(value.is_some());
        if let Some(value) = value {
            self.f32(value);
        }
    }

    fn f32_slice(&mut self, values: &[f32]) {
        self.usize(values.len());
        for value in values {
            self.f32(*value);
        }
    }
}

fn macro_mapping_bit_exact_eq(left: &MacroMapping, right: &MacroMapping) -> bool {
    left.scope == right.scope
        && left.target == right.target
        && left.range_min.to_bits() == right.range_min.to_bits()
        && left.range_max.to_bits() == right.range_max.to_bits()
        && left.curve == right.curve
        && left.suspended == right.suspended
}

fn macro_bit_exact_eq(left: &Macro, right: &Macro) -> bool {
    left.id == right.id
        && left.key == right.key
        && left.name == right.name
        && left.value.to_bits() == right.value.to_bits()
        && left.kind == right.kind
        && left.mappings.len() == right.mappings.len()
        && left
            .mappings
            .iter()
            .zip(&right.mappings)
            .all(|(left, right)| macro_mapping_bit_exact_eq(left, right))
}

fn encode_track_params(snapshot: &TrackParamsSnapshot) -> Vec<u8> {
    let mut bytes = WitnessBytes::default();
    bytes.bool(snapshot.gate);
    bytes.f32(snapshot.attack_ms);
    bytes.f32(snapshot.release_ms);
    bytes.f32(snapshot.swing);
    bytes.u32(snapshot.swing_resolution as u32);
    bytes.usize(snapshot.num_steps);
    bytes.f32(snapshot.volume);
    bytes.f32(snapshot.pan);
    bytes.bool(snapshot.mute);
    bytes.bool(snapshot.solo);
    bytes.f32(snapshot.send);
    match snapshot.output {
        crate::sequencer::TrackOutput::Mix => bytes.u32(0),
        crate::sequencer::TrackOutput::Bus(id) => {
            bytes.u32(1);
            bytes.u64(id.0);
        }
        crate::sequencer::TrackOutput::None => bytes.u32(2),
    }
    bytes.usize(snapshot.sends.len());
    for send in &snapshot.sends {
        bytes.u64(send.destination.0);
        bytes.f32(send.amount);
    }
    bytes.bool(snapshot.polyphonic);
    bytes.usize(snapshot.max_polyphony);
    bytes.u32(snapshot.timebase as u32);
    bytes.usize(snapshot.accumulator_idx);
    bytes.bool(snapshot.script_accumulator_name.is_some());
    if let Some(name) = &snapshot.script_accumulator_name {
        bytes.usize(name.len());
        bytes.0.extend_from_slice(name.as_bytes());
    }
    bytes.usize(snapshot.midi_fx_chain.len());
    for name in &snapshot.midi_fx_chain {
        bytes.usize(name.len());
        bytes.0.extend_from_slice(name.as_bytes());
    }
    bytes.u32(snapshot.midi_fx_position as u32);
    bytes.f32(snapshot.accum_limit);
    bytes.u32(snapshot.accum_mode);
    bytes.usize(snapshot.fts_scale);
    bytes.u32(snapshot.mute_group as u32);
    bytes.bool(snapshot.global_transpose);
    bytes.0
}

fn encode_optional_f32_rows(bytes: &mut WitnessBytes, rows: &[Vec<Option<f32>>]) {
    bytes.usize(rows.len());
    for row in rows {
        bytes.usize(row.len());
        for value in row {
            bytes.optional_f32(*value);
        }
    }
}

fn encode_effect_slot_values(bytes: &mut WitnessBytes, snapshot: &EffectSlotSnapshot) {
    bytes.usize(snapshot.num_params as usize);
    bytes.f32_slice(&snapshot.defaults);
    encode_optional_f32_rows(bytes, &snapshot.plocks);
    bytes.usize(snapshot.key_locks.len());
    for (note, values) in &snapshot.key_locks {
        bytes.0.push(*note);
        bytes.usize(values.len());
        for value in values {
            bytes.optional_f32(*value);
        }
    }
    bytes.usize(snapshot.tensor_params.len());
    for tensor in &snapshot.tensor_params {
        bytes.usize(tensor.name.len());
        bytes.0.extend_from_slice(tensor.name.as_bytes());
        bytes.usize(tensor.shape.len());
        for dimension in &tensor.shape {
            bytes.usize(*dimension);
        }
        bytes.usize(tensor.cell_offset);
        bytes.f32_slice(&tensor.default);
        bytes.usize(tensor.plocks.len());
        for values in &tensor.plocks {
            bytes.bool(values.is_some());
            if let Some(values) = values {
                bytes.f32_slice(values);
            }
        }
    }
}

fn capture_step_witness(
    app: &App,
    track: usize,
    steps: Vec<usize>,
    include_num_steps: bool,
) -> Result<BarrierWitness, EditError> {
    let pattern_id = app
        .state
        .effective_track_pattern_id(track)
        .ok_or(EditError::MissingTrackPattern)?;
    let (cells, _) = app
        .state
        .capture_pattern_step_cells(track, pattern_id, &steps)
        .map_err(EditError::InvalidTarget)?;
    let num_steps = if include_num_steps {
        app.state
            .live_track_params_snapshot(track)
            .ok_or(EditError::TrackOutOfRange { track })?
            .num_steps
    } else {
        0
    };
    Ok(BarrierWitness::Steps { num_steps, cells })
}

fn capture_track_params_witness(app: &App, track: usize) -> Result<BarrierWitness, EditError> {
    app.state
        .live_track_params_snapshot(track)
        .map(|snapshot| BarrierWitness::Bytes(encode_track_params(&snapshot)))
        .ok_or(EditError::TrackOutOfRange { track })
}

fn capture_effect_slot_witness(
    slot: Option<&crate::effects::EffectSlotState>,
    description: &str,
) -> Result<BarrierWitness, EditError> {
    let slot = slot.ok_or_else(|| EditError::InvalidTarget(description.to_string()))?;
    let mut bytes = WitnessBytes::default();
    encode_effect_slot_values(&mut bytes, &EffectSlotSnapshot::capture(slot));
    Ok(BarrierWitness::Bytes(bytes.0))
}

fn capture_instrument_slot_witness(app: &App, track: usize) -> Result<BarrierWitness, EditError> {
    let slot = app
        .state
        .pattern
        .instrument_slots
        .get(track)
        .ok_or(EditError::TrackOutOfRange { track })?;
    let dirty = app
        .state
        .pattern
        .track_sound_state
        .lock()
        .unwrap()
        .get(track)
        .map(|state| state.dirty)
        .ok_or(EditError::TrackOutOfRange { track })?;
    let mut bytes = WitnessBytes::default();
    encode_effect_slot_values(&mut bytes, &EffectSlotSnapshot::capture(slot));
    bytes.bool(dirty);
    Ok(BarrierWitness::Bytes(bytes.0))
}

fn capture_rack_slot_witness(
    app: &App,
    track: usize,
    slot_idx: usize,
) -> Result<BarrierWitness, EditError> {
    let rack = app
        .state
        .live_rack_track_snapshot(track)
        .ok_or_else(|| EditError::InvalidTarget("rack track does not exist".to_string()))?;
    let slot = rack
        .slots
        .get(slot_idx)
        .ok_or_else(|| EditError::InvalidTarget("rack slot does not exist".to_string()))?;
    let mut bytes = WitnessBytes::default();
    bytes.f32(slot.instrument_base_note_offset);
    bytes.f32(slot.gain);
    bytes.f32(slot.pan);
    bytes.bool(slot.mute);
    bytes.bool(slot.solo);
    bytes.usize(slot.max_polyphony);
    bytes.bool(slot.choke_group.is_some());
    if let Some(group) = slot.choke_group {
        bytes.0.push(group);
    }
    encode_optional_f32_rows(&mut bytes, &slot.param_plocks.rows);
    encode_effect_slot_values(&mut bytes, &slot.instrument_slot);
    bytes.bool(slot.track_sound_state.dirty);
    Ok(BarrierWitness::Bytes(bytes.0))
}

fn validate_device_command_target(app: &App, cmd: &AppCommand) -> Result<(), EditError> {
    let invalid = |message: &str| EditError::InvalidTarget(message.to_string());
    match cmd {
        AppCommand::SetEffectParam {
            track,
            slot_idx,
            param_idx,
            ..
        }
        | AppCommand::SetEffectPlock {
            track,
            slot_idx,
            param_idx,
            ..
        }
        | AppCommand::SetEffectPlockMulti {
            track,
            slot_idx,
            param_idx,
            ..
        }
        | AppCommand::ClearEffectPlockMulti {
            track,
            slot_idx,
            param_idx,
            ..
        } => {
            let slot = app
                .state
                .pattern
                .effect_chains
                .get(*track)
                .and_then(|chain| chain.get(*slot_idx))
                .ok_or_else(|| invalid("effect slot does not exist"))?;
            if *param_idx >= slot.num_params.load(std::sync::atomic::Ordering::Relaxed) as usize {
                return Err(invalid("effect parameter does not exist"));
            }
        }
        AppCommand::SetEffectTensorCell {
            track, slot_idx, tensor_idx, cell_idx, ..
        }
        | AppCommand::SetEffectTensorPlockCellMulti {
            track, slot_idx, tensor_idx, cell_idx, ..
        } => {
            let slot = app.state.pattern.effect_chains.get(*track)
                .and_then(|chain| chain.get(*slot_idx))
                .ok_or_else(|| invalid("effect slot does not exist"))?;
            if *tensor_idx >= slot.tensor_params.num_params()
                || *cell_idx >= slot.tensor_params.tensor_len(*tensor_idx)
            {
                return Err(invalid("effect tensor cell does not exist"));
            }
        }
        AppCommand::ClearEffectTensorPlockMulti { track, slot_idx, tensor_idx, .. } => {
            let slot = app.state.pattern.effect_chains.get(*track)
                .and_then(|chain| chain.get(*slot_idx))
                .ok_or_else(|| invalid("effect slot does not exist"))?;
            if *tensor_idx >= slot.tensor_params.num_params() {
                return Err(invalid("effect tensor does not exist"));
            }
        }
        AppCommand::SetMidiFxParam { track, slot_idx, param_idx, .. }
        | AppCommand::SetMidiFxPlockMulti { track, slot_idx, param_idx, .. }
        | AppCommand::ClearMidiFxPlockMulti { track, slot_idx, param_idx, .. } => {
            let slot = app.state.pattern.midi_fx_slots.get(*track)
                .and_then(|slots| slots.get(*slot_idx))
                .ok_or_else(|| invalid("MIDI-FX slot does not exist"))?;
            if *param_idx >= slot.num_params.load(Ordering::Relaxed) as usize {
                return Err(invalid("MIDI-FX parameter does not exist"));
            }
        }
        AppCommand::SetMidiFxTensorCell { track, slot_idx, tensor_idx, cell_idx, .. }
        | AppCommand::SetMidiFxTensorPlockCellMulti { track, slot_idx, tensor_idx, cell_idx, .. } => {
            let slot = app.state.pattern.midi_fx_slots.get(*track)
                .and_then(|slots| slots.get(*slot_idx))
                .ok_or_else(|| invalid("MIDI-FX slot does not exist"))?;
            if *tensor_idx >= slot.tensor_params.num_params()
                || *cell_idx >= slot.tensor_params.tensor_len(*tensor_idx)
            {
                return Err(invalid("MIDI-FX tensor cell does not exist"));
            }
        }
        AppCommand::ClearMidiFxTensorPlockMulti { track, slot_idx, tensor_idx, .. } => {
            let slot = app.state.pattern.midi_fx_slots.get(*track)
                .and_then(|slots| slots.get(*slot_idx))
                .ok_or_else(|| invalid("MIDI-FX slot does not exist"))?;
            if *tensor_idx >= slot.tensor_params.num_params() {
                return Err(invalid("MIDI-FX tensor does not exist"));
            }
        }
        AppCommand::SetInstrumentParam {
            track, param_idx, ..
        }
        | AppCommand::SetInstrumentPlock {
            track, param_idx, ..
        }
        | AppCommand::SetInstrumentPlockMulti {
            track, param_idx, ..
        }
        | AppCommand::ClearInstrumentPlockMulti {
            track, param_idx, ..
        }
        | AppCommand::SetInstrumentKeyLock {
            track, param_idx, ..
        }
        | AppCommand::SetInstrumentKeyLockMulti {
            track, param_idx, ..
        }
        | AppCommand::ClearInstrumentKeyLock {
            track, param_idx, ..
        } => {
            let slot = app
                .state
                .pattern
                .instrument_slots
                .get(*track)
                .ok_or(EditError::TrackOutOfRange { track: *track })?;
            if *param_idx >= slot.num_params.load(std::sync::atomic::Ordering::Relaxed) as usize {
                return Err(invalid("instrument parameter does not exist"));
            }
        }
        AppCommand::SetInstrumentTensorCell {
            track,
            tensor_idx,
            cell_idx,
            ..
        }
        | AppCommand::SetInstrumentTensorPlockCellMulti {
            track,
            tensor_idx,
            cell_idx,
            ..
        } => {
            let slot = app
                .state
                .pattern
                .instrument_slots
                .get(*track)
                .ok_or(EditError::TrackOutOfRange { track: *track })?;
            if *tensor_idx >= slot.tensor_params.num_params()
                || *cell_idx >= slot.tensor_params.tensor_len(*tensor_idx)
            {
                return Err(invalid("instrument tensor cell does not exist"));
            }
        }
        AppCommand::ClearInstrumentTensorPlockMulti {
            track, tensor_idx, ..
        } => {
            let slot = app
                .state
                .pattern
                .instrument_slots
                .get(*track)
                .ok_or(EditError::TrackOutOfRange { track: *track })?;
            if *tensor_idx >= slot.tensor_params.num_params() {
                return Err(invalid("instrument tensor does not exist"));
            }
        }
        AppCommand::SetRackSlotInstrumentParam {
            track,
            slot_idx,
            param_idx,
            ..
        }
        | AppCommand::SetRackSlotInstrumentPlock {
            track,
            slot_idx,
            param_idx,
            ..
        }
        | AppCommand::SetRackSlotInstrumentPlockMulti {
            track,
            slot_idx,
            param_idx,
            ..
        } => {
            let rack = app
                .state
                .live_rack_track_snapshot(*track)
                .ok_or_else(|| invalid("rack track does not exist"))?;
            let slot = rack
                .slots
                .get(*slot_idx)
                .ok_or_else(|| invalid("rack slot does not exist"))?;
            if *param_idx >= slot.instrument_slot.num_params as usize {
                return Err(invalid("rack instrument parameter does not exist"));
            }
        }
        AppCommand::SetRackSlotGain { track, slot_idx, .. }
        | AppCommand::SetRackSlotPan { track, slot_idx, .. }
        | AppCommand::SetRackSlotMute { track, slot_idx, .. }
        | AppCommand::SetRackSlotSolo { track, slot_idx, .. }
        | AppCommand::SetRackSlotMaxPolyphony { track, slot_idx, .. }
        | AppCommand::SetRackSlotChokeGroup { track, slot_idx, .. }
        | AppCommand::SetRackSlotBaseNoteOffset { track, slot_idx, .. }
        | AppCommand::SetRackSlotParamPlock { track, slot_idx, .. }
        | AppCommand::SetRackSlotParamPlockMulti { track, slot_idx, .. } => {
            let rack = app
                .state
                .live_rack_track_snapshot(*track)
                .ok_or_else(|| invalid("rack track does not exist"))?;
            if rack.slots.get(*slot_idx).is_none() {
                return Err(invalid("rack slot does not exist"));
            }
        }
        AppCommand::SetRackSlotEffectParam {
            track, rack_slot_idx, effect_slot_idx, param_idx, ..
        }
        | AppCommand::SetRackSlotEffectPlockMulti {
            track, rack_slot_idx, effect_slot_idx, param_idx, ..
        }
        | AppCommand::ClearRackSlotEffectPlockMulti {
            track, rack_slot_idx, effect_slot_idx, param_idx, ..
        } => {
            let rack = app.state.live_rack_track_snapshot(*track)
                .ok_or_else(|| invalid("rack track does not exist"))?;
            let effect = rack.slots.get(*rack_slot_idx)
                .and_then(|slot| slot.effect_slots.get(*effect_slot_idx))
                .ok_or_else(|| invalid("rack effect slot does not exist"))?;
            if *param_idx >= effect.num_params as usize {
                return Err(invalid("rack effect parameter does not exist"));
            }
        }
        AppCommand::SetRackMacroPlockMulti { track, macro_idx, .. }
        | AppCommand::ClearRackMacroPlockMulti { track, macro_idx, .. } => {
            let rack = app.state.live_rack_track_snapshot(*track)
                .ok_or_else(|| invalid("rack track does not exist"))?;
            if *macro_idx >= rack.macros.len() {
                return Err(invalid("rack macro does not exist"));
            }
        }
        _ => {}
    }
    Ok(())
}

fn capture_barrier_witness(app: &App, cmd: &AppCommand) -> Result<BarrierWitness, EditError> {
    validate_device_command_target(app, cmd)?;
    match cmd {
        AppCommand::DuplicateTrackPattern { track }
        | AppCommand::HalveTrackPattern { track } => {
            capture_step_witness(app, *track, (0..MAX_STEPS).collect(), true)
        }
        AppCommand::SetTimebasePlock { track, step, .. }
        | AppCommand::SetTrackSwingPlock { track, step, .. }
        | AppCommand::SetTrackSwingResolutionPlock { track, step, .. }
        | AppCommand::SetEffectPlock { track, step, .. }
        | AppCommand::SetInstrumentPlock { track, step, .. }
        | AppCommand::SetRackSlotParamPlock { track, step, .. }
        | AppCommand::SetRackSlotInstrumentPlock { track, step, .. } => {
            capture_step_witness(app, *track, vec![*step], false)
        }
        AppCommand::SetTimebasePlockMulti { track, steps, .. }
        | AppCommand::ClearTimebasePlockMulti { track, steps }
        | AppCommand::SetTrackSwingPlockMulti { track, steps, .. }
        | AppCommand::ClearTrackSwingPlockMulti { track, steps }
        | AppCommand::SetTrackSwingResolutionPlockMulti { track, steps, .. }
        | AppCommand::ClearTrackSwingResolutionPlockMulti { track, steps }
        | AppCommand::SetEffectPlockMulti { track, steps, .. }
        | AppCommand::SetInstrumentPlockMulti { track, steps, .. }
        | AppCommand::SetInstrumentTensorPlockCellMulti { track, steps, .. }
        | AppCommand::ClearInstrumentTensorPlockMulti { track, steps, .. }
        | AppCommand::SetRackSlotParamPlockMulti { track, steps, .. }
        | AppCommand::SetRackSlotInstrumentPlockMulti { track, steps, .. } => {
            capture_step_witness(app, *track, normalized_steps(steps), false)
        }

        AppCommand::ToggleTrackGate { track }
        | AppCommand::ToggleTrackPolyphonic { track }
        | AppCommand::ToggleTrackMute { track }
        | AppCommand::ToggleTrackSolo { track }
        | AppCommand::AdjustTrackMaxPolyphony { track, .. }
        | AppCommand::SetTrackMaxPolyphony { track, .. }
        | AppCommand::SetTrackAttack { track, .. }
        | AppCommand::AdjustTrackAttack { track, .. }
        | AppCommand::SetTrackRelease { track, .. }
        | AppCommand::AdjustTrackRelease { track, .. }
        | AppCommand::SetTrackSwing { track, .. }
        | AppCommand::AdjustTrackSwing { track, .. }
        | AppCommand::SetTrackSwingResolution { track, .. }
        | AppCommand::NextTrackSwingResolution { track }
        | AppCommand::PrevTrackSwingResolution { track }
        | AppCommand::SetTrackNumSteps { track, .. }
        | AppCommand::AdjustTrackNumSteps { track, .. }
        | AppCommand::SetTrackVolume { track, .. }
        | AppCommand::AdjustTrackVolume { track, .. }
        | AppCommand::SetTrackPan { track, .. }
        | AppCommand::AdjustTrackPan { track, .. }
        | AppCommand::SetTrackSend { track, .. }
        | AppCommand::AdjustTrackSend { track, .. }
        | AppCommand::SetTrackOutput { track, .. }
        | AppCommand::SetTrackSends { track, .. }
        | AppCommand::SetTrackTimebase { track, .. }
        | AppCommand::NextTrackTimebase { track }
        | AppCommand::PrevTrackTimebase { track }
        | AppCommand::SetTrackFtsScale { track, .. }
        | AppCommand::SetTrackAccumIdx { track, .. }
        | AppCommand::SetTrackAccumLimit { track, .. }
        | AppCommand::AdjustTrackAccumLimit { track, .. }
        | AppCommand::SetTrackAccumMode { track, .. }
        | AppCommand::SetTrackMuteGroup { track, .. }
        | AppCommand::SetTrackGlobalTranspose { track, .. } => {
            capture_track_params_witness(app, *track)
        }

        AppCommand::SetEffectParam {
            track, slot_idx, ..
        } => capture_effect_slot_witness(
            app.state
                .pattern
                .effect_chains
                .get(*track)
                .and_then(|chain| chain.get(*slot_idx)),
            "effect slot does not exist",
        ),
        AppCommand::SetInstrumentParam { track, .. }
        | AppCommand::SetInstrumentKeyLock { track, .. }
        | AppCommand::SetInstrumentKeyLockMulti { track, .. }
        | AppCommand::ClearInstrumentKeyLock { track, .. }
        | AppCommand::ClearInstrumentKeyLocksForNote { track, .. }
        | AppCommand::StampInstrumentKeyLockVariant { track, .. }
        | AppCommand::ClearInstrumentKeyLockVariantsForNotes { track, .. }
        | AppCommand::SetInstrumentTensorCell { track, .. } => {
            capture_instrument_slot_witness(app, *track)
        }
        AppCommand::SetInstrumentBaseNoteOffset { track, .. } => {
            let value = app
                .state
                .pattern
                .instrument_base_note_offsets
                .get(*track)
                .ok_or(EditError::TrackOutOfRange { track: *track })?
                .load(std::sync::atomic::Ordering::Relaxed);
            Ok(BarrierWitness::Bytes(value.to_le_bytes().to_vec()))
        }

        AppCommand::SetRackSlotGain { track, slot_idx, .. }
        | AppCommand::SetRackSlotPan { track, slot_idx, .. }
        | AppCommand::SetRackSlotMute { track, slot_idx, .. }
        | AppCommand::SetRackSlotSolo { track, slot_idx, .. }
        | AppCommand::SetRackSlotMaxPolyphony { track, slot_idx, .. }
        | AppCommand::SetRackSlotChokeGroup { track, slot_idx, .. }
        | AppCommand::SetRackSlotBaseNoteOffset { track, slot_idx, .. }
        | AppCommand::SetRackSlotInstrumentParam { track, slot_idx, .. } => {
            capture_rack_slot_witness(app, *track, *slot_idx)
        }

        AppCommand::MacroCreate { .. }
        | AppCommand::MacroCreateScene { .. }
        | AppCommand::MacroSceneConfig { .. }
        | AppCommand::MacroEnsure { .. }
        | AppCommand::MacroDelete { .. }
        | AppCommand::MacroRename { .. }
        | AppCommand::MacroMapParam { .. }
        | AppCommand::MacroSetRange { .. }
        | AppCommand::MacroSetCurve { .. }
        | AppCommand::MacroUnmap { .. } => Ok(BarrierWitness::Macros {
            next_id: app.macro_engine.next_id(),
            definitions: app.macro_engine.macros().to_vec(),
        }),

        AppCommand::SetMasterVolume { .. } | AppCommand::AdjustMasterVolume { .. } => Ok(
            BarrierWitness::Bytes(
                app.state
                    .transport
                    .master_volume
                    .load(std::sync::atomic::Ordering::Relaxed)
                    .to_le_bytes()
                    .to_vec(),
            ),
        ),
        AppCommand::SetBpm { .. } => Ok(BarrierWitness::Bytes(
            app.state
                .transport
                .bpm
                .load(std::sync::atomic::Ordering::Relaxed)
                .to_le_bytes()
                .to_vec(),
        )),
        AppCommand::AdjustRecordQuantizeThresh { .. } => Ok(BarrierWitness::Bytes(
            app.state
                .transport
                .record_quantize_thresh
                .load(std::sync::atomic::Ordering::Relaxed)
                .to_le_bytes()
                .to_vec(),
        )),

        AppCommand::ToggleStep { .. }
        | AppCommand::SetStepActive { .. }
        | AppCommand::SetStepParam { .. }
        | AppCommand::AdjustStepParam { .. }
        | AppCommand::ClearStepPayload { .. }
        | AppCommand::ClearSteps { .. }
        | AppCommand::RotateSteps { .. }
        | AppCommand::PasteSteps { .. }
        | AppCommand::ShiftStepRange { .. }
        | AppCommand::TogglePianoNote { .. }
        | AppCommand::MacroSetValue { .. }
        | AppCommand::MacroRelease { .. }
        | AppCommand::ScenePushBegin { .. }
        | AppCommand::ScenePushSetValue { .. }
        | AppCommand::ScenePushEnd
        | AppCommand::SetBusVolume { .. }
        | AppCommand::ToggleBusMute { .. }
        | AppCommand::ToggleBusSolo { .. }
        | AppCommand::SetMidiFxParam { .. }
        | AppCommand::SetEffectTensorCell { .. }
        | AppCommand::SetEffectTensorPlockCellMulti { .. }
        | AppCommand::ClearEffectTensorPlockMulti { .. }
        | AppCommand::SetMidiFxTensorCell { .. }
        | AppCommand::SetMidiFxTensorPlockCellMulti { .. }
        | AppCommand::ClearMidiFxTensorPlockMulti { .. }
        | AppCommand::SetRackSlotEffectParam { .. }
        | AppCommand::SetMidiFxPlockMulti { .. }
        | AppCommand::ClearMidiFxPlockMulti { .. }
        | AppCommand::ClearEffectPlockMulti { .. }
        | AppCommand::ClearInstrumentPlockMulti { .. }
        | AppCommand::SetRackMacroPlockMulti { .. }
        | AppCommand::ClearRackMacroPlockMulti { .. }
        | AppCommand::SetRackSlotEffectPlockMulti { .. }
        | AppCommand::ClearRackSlotEffectPlockMulti { .. }
        | AppCommand::TogglePlay => Err(EditError::UnsupportedCommand),
    }
}

pub fn commit_history_barrier(app: &mut App) {
    let cleared_entries = app.history.undo_len() + app.history.redo_len();
    app.history.barrier();
    app.device_registry.clear();
    if cleared_entries > 0 {
        app.editor.status_message = Some((
            format!(
                "Undo history cleared by an edit not yet supported ({cleared_entries} entr{})",
                if cleared_entries == 1 { "y" } else { "ies" }
            ),
            Instant::now(),
        ));
    }
}

enum ResolvedStepCommand<'a> {
    Toggle { step: usize },
    SetActive { step: usize, active: bool },
    SetParam {
        step: usize,
        param: crate::sequencer::StepParam,
        value: f32,
    },
    AdjustParam {
        step: usize,
        param: crate::sequencer::StepParam,
        delta: f32,
    },
    Clear { steps: Vec<usize> },
    Rotate { steps: Vec<usize>, direction: isize },
    Paste {
        source_track: usize,
        clipboard: &'a [(usize, StepCellSnapshot)],
        dest_start: usize,
        num_steps: usize,
        affected: Vec<usize>,
    },
    Shift {
        lo: usize,
        hi: usize,
        new_lo: usize,
        affected: Vec<usize>,
    },
    TogglePianoNote { step: usize, semitone: i32 },
    TimebasePlock { steps: Vec<usize>, value: Option<crate::sequencer::Timebase> },
    SwingPlock { steps: Vec<usize>, value: Option<f32> },
    SwingResolutionPlock {
        steps: Vec<usize>,
        value: Option<crate::sequencer::SwingResolution>,
    },
}

impl ResolvedStepCommand<'_> {
    fn affected_steps(&self) -> &[usize] {
        match self {
            Self::Toggle { step }
            | Self::SetActive { step, .. }
            | Self::SetParam { step, .. }
            | Self::AdjustParam { step, .. }
            | Self::TogglePianoNote { step, .. } => std::slice::from_ref(step),
            Self::Clear { steps }
            | Self::Rotate { steps, .. }
            | Self::TimebasePlock { steps, .. }
            | Self::SwingPlock { steps, .. }
            | Self::SwingResolutionPlock { steps, .. } => steps,
            Self::Paste { affected, .. } | Self::Shift { affected, .. } => affected,
        }
    }

    fn label(&self) -> &'static str {
        match self {
            Self::Toggle { .. } => "Toggle step",
            Self::SetActive { .. } => "Set step active",
            Self::SetParam { .. } => "Set step parameter",
            Self::AdjustParam { .. } => "Adjust step parameter",
            Self::Clear { steps } if steps.len() > 1 => "Clear steps",
            Self::Clear { .. } => "Clear step",
            Self::Rotate { .. } => "Rotate steps",
            Self::Paste { .. } => "Paste steps",
            Self::Shift { .. } => "Move steps",
            Self::TogglePianoNote { .. } => "Toggle piano note",
            Self::TimebasePlock { value: Some(_), .. } => "Set timebase p-lock",
            Self::TimebasePlock { value: None, .. } => "Clear timebase p-lock",
            Self::SwingPlock { value: Some(_), .. } => "Set swing p-lock",
            Self::SwingPlock { value: None, .. } => "Clear swing p-lock",
            Self::SwingResolutionPlock { value: Some(_), .. } => "Set swing-resolution p-lock",
            Self::SwingResolutionPlock { value: None, .. } => "Clear swing-resolution p-lock",
        }
    }
}

fn normalized_steps(steps: &[usize]) -> Vec<usize> {
    let mut steps = steps
        .iter()
        .copied()
        .filter(|step| *step < MAX_STEPS)
        .collect::<Vec<_>>();
    steps.sort_unstable();
    steps.dedup();
    steps
}

fn resolve_step_command(cmd: &AppCommand) -> Result<(usize, ResolvedStepCommand<'_>), EditError> {
    let resolved = match cmd {
        AppCommand::ToggleStep { track, step } => (*track, ResolvedStepCommand::Toggle { step: *step }),
        AppCommand::SetStepActive { track, step, active } => (
            *track,
            ResolvedStepCommand::SetActive {
                step: *step,
                active: *active,
            },
        ),
        AppCommand::SetStepParam { track, step, param, value } => (
            *track,
            ResolvedStepCommand::SetParam {
                step: *step,
                param: *param,
                value: *value,
            },
        ),
        AppCommand::AdjustStepParam { track, step, param, delta } => (
            *track,
            ResolvedStepCommand::AdjustParam {
                step: *step,
                param: *param,
                delta: *delta,
            },
        ),
        AppCommand::ClearStepPayload { track, step } => (
            *track,
            ResolvedStepCommand::Clear { steps: vec![*step] },
        ),
        AppCommand::ClearSteps { track, steps } => (
            *track,
            ResolvedStepCommand::Clear {
                steps: normalized_steps(steps),
            },
        ),
        AppCommand::RotateSteps { track, steps, direction } => (
            *track,
            ResolvedStepCommand::Rotate {
                steps: normalized_steps(steps),
                direction: *direction,
            },
        ),
        AppCommand::PasteSteps {
            track,
            source_track,
            clipboard,
            dest_start,
            num_steps,
        } => {
            let candidates = clipboard
                .iter()
                .filter_map(|(offset, _)| dest_start.checked_add(*offset))
                .filter(|step| *step < *num_steps)
                .collect::<Vec<_>>();
            (
                *track,
                ResolvedStepCommand::Paste {
                    source_track: *source_track,
                    clipboard,
                    dest_start: *dest_start,
                    num_steps: *num_steps,
                    affected: normalized_steps(&candidates),
                },
            )
        }
        AppCommand::ShiftStepRange { track, lo, hi, new_lo } => {
            if lo > hi || *hi >= MAX_STEPS {
                return Err(EditError::InvalidStepRange);
            }
            let count = hi - lo + 1;
            let new_hi = new_lo
                .checked_add(count - 1)
                .ok_or(EditError::InvalidStepRange)?;
            if new_hi >= MAX_STEPS {
                return Err(EditError::InvalidStepRange);
            }
            let candidates = (*lo..=*hi)
                .chain(*new_lo..=new_hi)
                .collect::<Vec<_>>();
            (
                *track,
                ResolvedStepCommand::Shift {
                    lo: *lo,
                    hi: *hi,
                    new_lo: *new_lo,
                    affected: normalized_steps(&candidates),
                },
            )
        }
        AppCommand::TogglePianoNote {
            track,
            step,
            semitone,
        } => (
            *track,
            ResolvedStepCommand::TogglePianoNote {
                step: *step,
                semitone: *semitone,
            },
        ),
        AppCommand::SetTimebasePlock { track, step, timebase } => (
            *track,
            ResolvedStepCommand::TimebasePlock {
                steps: vec![*step],
                value: *timebase,
            },
        ),
        AppCommand::SetTimebasePlockMulti { track, steps, timebase } => (
            *track,
            ResolvedStepCommand::TimebasePlock {
                steps: normalized_steps(steps),
                value: Some(*timebase),
            },
        ),
        AppCommand::ClearTimebasePlockMulti { track, steps } => (
            *track,
            ResolvedStepCommand::TimebasePlock {
                steps: normalized_steps(steps),
                value: None,
            },
        ),
        AppCommand::SetTrackSwingPlock { track, step, value } => (
            *track,
            ResolvedStepCommand::SwingPlock {
                steps: vec![*step],
                value: *value,
            },
        ),
        AppCommand::SetTrackSwingPlockMulti { track, steps, value } => (
            *track,
            ResolvedStepCommand::SwingPlock {
                steps: normalized_steps(steps),
                value: Some(*value),
            },
        ),
        AppCommand::ClearTrackSwingPlockMulti { track, steps } => (
            *track,
            ResolvedStepCommand::SwingPlock {
                steps: normalized_steps(steps),
                value: None,
            },
        ),
        AppCommand::SetTrackSwingResolutionPlock { track, step, resolution } => (
            *track,
            ResolvedStepCommand::SwingResolutionPlock {
                steps: vec![*step],
                value: *resolution,
            },
        ),
        AppCommand::SetTrackSwingResolutionPlockMulti { track, steps, resolution } => (
            *track,
            ResolvedStepCommand::SwingResolutionPlock {
                steps: normalized_steps(steps),
                value: Some(*resolution),
            },
        ),
        AppCommand::ClearTrackSwingResolutionPlockMulti { track, steps } => (
            *track,
            ResolvedStepCommand::SwingResolutionPlock {
                steps: normalized_steps(steps),
                value: None,
            },
        ),
        _ => return Err(EditError::UnsupportedCommand),
    };
    if let Some(step) = resolved.1.affected_steps().iter().find(|step| **step >= MAX_STEPS) {
        return Err(EditError::StepOutOfRange { step: *step });
    }
    Ok(resolved)
}

fn execute_step_command_no_publish(app: &mut App, track: usize, cmd: &ResolvedStepCommand<'_>) {
    match cmd {
        ResolvedStepCommand::Toggle { step } => {
            app.clear_step_selection();
            app.state.toggle_step_and_clear_plocks_no_publish(track, *step);
        }
        ResolvedStepCommand::SetActive { step, active } => {
            app.state.pattern.patterns[track].set_step_active(*step, *active);
        }
        ResolvedStepCommand::SetParam { step, param, value } => {
            app.state.set_step_param_inner(track, *step, *param, *value);
        }
        ResolvedStepCommand::AdjustParam { step, param, delta } => {
            let current = app.state.pattern.step_data[track].get(*step, *param);
            app.state
                .set_step_param_inner(track, *step, *param, current + delta);
        }
        ResolvedStepCommand::Clear { steps } => {
            for step in steps {
                app.state.clear_step_payload_inner(track, *step);
            }
        }
        ResolvedStepCommand::Rotate { steps, direction } => {
            app.state.rotate_steps_no_publish(track, steps, *direction);
        }
        ResolvedStepCommand::Paste {
            source_track,
            clipboard,
            dest_start,
            num_steps,
            ..
        } => {
            let preserve_audio_plocks = *source_track == track;
            for (offset, snapshot) in *clipboard {
                let Some(destination) = dest_start.checked_add(*offset) else {
                    continue;
                };
                if destination >= *num_steps || destination >= MAX_STEPS {
                    continue;
                }
                if !snapshot.active && app.state.pattern.patterns[track].is_active(destination) {
                    continue;
                }
                let snapshot = sanitize_pasted_step_snapshot(snapshot, preserve_audio_plocks);
                app.state
                    .restore_step_snapshot_inner(track, destination, &snapshot);
            }
        }
        ResolvedStepCommand::Shift { lo, hi, new_lo, .. } => {
            app.state
                .move_step_range_no_publish(track, *lo, *hi, *new_lo);
        }
        ResolvedStepCommand::TogglePianoNote { step, semitone } => {
            let pattern = &app.state.pattern;
            let is_active = pattern.patterns[track].is_active(*step);
            let chord_count = pattern.chord_data[track].count(*step);
            if !is_active {
                pattern.patterns[track].set_step_active(*step, true);
                app.state.set_step_param_inner(
                    track,
                    *step,
                    crate::sequencer::StepParam::Transpose,
                    *semitone as f32,
                );
            } else if chord_count == 0 {
                let current = pattern.step_data[track]
                    .get(*step, crate::sequencer::StepParam::Transpose)
                    .round() as i32;
                if *semitone == current {
                    pattern.patterns[track].set_step_active(*step, false);
                } else {
                    pattern.chord_data[track].add_note(*step, current as f32);
                    pattern.chord_data[track].add_note(*step, *semitone as f32);
                }
            } else {
                let added = pattern.chord_data[track].toggle_note(*step, *semitone as f32);
                if !added {
                    match pattern.chord_data[track].count(*step) {
                        0 => pattern.patterns[track].set_step_active(*step, false),
                        1 => {
                            let remaining = pattern.chord_data[track].get(*step, 0);
                            pattern.step_data[track].set(
                                *step,
                                crate::sequencer::StepParam::Transpose,
                                remaining,
                            );
                            pattern.chord_data[track].clear_step(*step);
                        }
                        _ => {}
                    }
                }
            }
        }
        ResolvedStepCommand::TimebasePlock { steps, value } => {
            for step in steps {
                match value {
                    Some(value) => app.state.pattern.timebase_plocks[track].set(*step, *value),
                    None => app.state.pattern.timebase_plocks[track].clear(*step),
                }
            }
        }
        ResolvedStepCommand::SwingPlock { steps, value } => {
            for step in steps {
                match value {
                    Some(value) => app.state.pattern.swing_plocks[track].set(*step, *value),
                    None => app.state.pattern.swing_plocks[track].clear(*step),
                }
            }
        }
        ResolvedStepCommand::SwingResolutionPlock { steps, value } => {
            for step in steps {
                match value {
                    Some(value) => app.state.pattern.swing_resolution_plocks[track].set(*step, *value),
                    None => app.state.pattern.swing_resolution_plocks[track].clear(*step),
                }
            }
        }
    }
}

pub fn apply_recorded_step_command(
    app: &mut App,
    cmd: &AppCommand,
) -> Result<EditOutcome, EditError> {
    if history_policy(cmd) != HistoryPolicy::Record {
        return Err(EditError::UnsupportedCommand);
    }
    let (track, resolved) = resolve_step_command(cmd)?;
    let affected = resolved.affected_steps().to_vec();
    let label = resolved.label();
    apply_recorded_step_mutation(app, track, &affected, label, |app| {
        execute_step_command_no_publish(app, track, &resolved);
        Ok(())
    })
}

fn is_pattern_geometry_command(cmd: &AppCommand) -> bool {
    matches!(
        cmd,
        AppCommand::DuplicateTrackPattern { .. }
            | AppCommand::HalveTrackPattern { .. }
            | AppCommand::SetTrackNumSteps { .. }
            | AppCommand::AdjustTrackNumSteps { .. }
    )
}

fn device_plock_command_target(cmd: &AppCommand) -> Option<(usize, Vec<usize>)> {
    match cmd {
        AppCommand::SetEffectPlock { track, step, .. }
        | AppCommand::SetInstrumentPlock { track, step, .. }
        | AppCommand::SetRackSlotParamPlock { track, step, .. }
        | AppCommand::SetRackSlotInstrumentPlock { track, step, .. } => {
            Some((*track, vec![*step]))
        }
        AppCommand::SetEffectPlockMulti { track, steps, .. }
        | AppCommand::ClearEffectPlockMulti { track, steps, .. }
        | AppCommand::SetEffectTensorPlockCellMulti { track, steps, .. }
        | AppCommand::ClearEffectTensorPlockMulti { track, steps, .. }
        | AppCommand::SetMidiFxPlockMulti { track, steps, .. }
        | AppCommand::ClearMidiFxPlockMulti { track, steps, .. }
        | AppCommand::SetMidiFxTensorPlockCellMulti { track, steps, .. }
        | AppCommand::ClearMidiFxTensorPlockMulti { track, steps, .. }
        | AppCommand::SetInstrumentPlockMulti { track, steps, .. }
        | AppCommand::ClearInstrumentPlockMulti { track, steps, .. }
        | AppCommand::SetInstrumentTensorPlockCellMulti { track, steps, .. }
        | AppCommand::ClearInstrumentTensorPlockMulti { track, steps, .. }
        | AppCommand::SetRackSlotParamPlockMulti { track, steps, .. }
        | AppCommand::SetRackSlotInstrumentPlockMulti { track, steps, .. }
        | AppCommand::SetRackMacroPlockMulti { track, steps, .. }
        | AppCommand::ClearRackMacroPlockMulti { track, steps, .. }
        | AppCommand::SetRackSlotEffectPlockMulti { track, steps, .. }
        | AppCommand::ClearRackSlotEffectPlockMulti { track, steps, .. } => {
            Some((*track, normalized_steps(steps)))
        }
        _ => None,
    }
}

fn device_plock_label(cmd: &AppCommand) -> &'static str {
    match cmd {
        AppCommand::SetEffectPlock { .. }
        | AppCommand::SetEffectPlockMulti { .. }
        | AppCommand::ClearEffectPlockMulti { .. } => {
            "Set effect p-lock"
        }
        AppCommand::SetEffectTensorPlockCellMulti { .. }
        | AppCommand::ClearEffectTensorPlockMulti { .. } => "Set effect tensor p-lock",
        AppCommand::SetMidiFxPlockMulti { .. } | AppCommand::ClearMidiFxPlockMulti { .. } => {
            "Set MIDI-FX p-lock"
        }
        AppCommand::SetMidiFxTensorPlockCellMulti { .. }
        | AppCommand::ClearMidiFxTensorPlockMulti { .. } => "Set MIDI-FX tensor p-lock",
        AppCommand::SetInstrumentPlock { .. }
        | AppCommand::SetInstrumentPlockMulti { .. }
        | AppCommand::ClearInstrumentPlockMulti { .. } => "Set instrument p-lock",
        AppCommand::SetInstrumentTensorPlockCellMulti { .. }
        | AppCommand::ClearInstrumentTensorPlockMulti { .. } => "Set instrument tensor p-lock",
        AppCommand::SetRackSlotParamPlock { .. }
        | AppCommand::SetRackSlotParamPlockMulti { .. } => "Set rack strip p-lock",
        AppCommand::SetRackSlotInstrumentPlock { .. }
        | AppCommand::SetRackSlotInstrumentPlockMulti { .. } => {
            "Set rack instrument p-lock"
        }
        AppCommand::SetRackMacroPlockMulti { .. } => "Set rack macro p-lock",
        AppCommand::ClearRackMacroPlockMulti { .. } => "Clear rack macro p-lock",
        AppCommand::SetRackSlotEffectPlockMulti { .. } => "Set rack effect p-lock",
        AppCommand::ClearRackSlotEffectPlockMulti { .. } => "Clear rack effect p-lock",
        _ => "Set device p-lock",
    }
}

fn apply_recorded_device_plock_command(
    app: &mut App,
    cmd: &AppCommand,
) -> Result<EditOutcome, EditError> {
    let (track, steps) =
        device_plock_command_target(cmd).ok_or(EditError::UnsupportedCommand)?;
    let label = device_plock_label(cmd);
    apply_recorded_step_mutation(app, track, &steps, label, |app| {
        super::command::execute_command(app, cmd.clone());
        Ok(())
    })
}

fn device_plock_merge_suffix(cmd: &AppCommand) -> Option<String> {
    match cmd {
        AppCommand::SetEffectPlock { slot_idx, param_idx, .. }
        | AppCommand::SetEffectPlockMulti { slot_idx, param_idx, .. } => {
            Some(format!("effect:{slot_idx}:param:{param_idx}"))
        }
        AppCommand::SetEffectTensorPlockCellMulti {
            slot_idx, tensor_idx, cell_idx, ..
        } => Some(format!("effect:{slot_idx}:tensor:{tensor_idx}:cell:{cell_idx}")),
        AppCommand::SetMidiFxPlockMulti { slot_idx, param_idx, .. } => {
            Some(format!("midi-fx:{slot_idx}:param:{param_idx}"))
        }
        AppCommand::SetMidiFxTensorPlockCellMulti {
            slot_idx, tensor_idx, cell_idx, ..
        } => Some(format!("midi-fx:{slot_idx}:tensor:{tensor_idx}:cell:{cell_idx}")),
        AppCommand::SetInstrumentPlock { param_idx, .. }
        | AppCommand::SetInstrumentPlockMulti { param_idx, .. } => {
            Some(format!("instrument:param:{param_idx}"))
        }
        AppCommand::SetInstrumentTensorPlockCellMulti {
            tensor_idx, cell_idx, ..
        } => Some(format!("instrument:tensor:{tensor_idx}:cell:{cell_idx}")),
        AppCommand::SetRackSlotParamPlock { slot_idx, param, .. }
        | AppCommand::SetRackSlotParamPlockMulti { slot_idx, param, .. } => {
            Some(format!("rack:{slot_idx}:strip:{param:?}"))
        }
        AppCommand::SetRackSlotInstrumentPlock { slot_idx, param_idx, .. }
        | AppCommand::SetRackSlotInstrumentPlockMulti { slot_idx, param_idx, .. } => {
            Some(format!("rack:{slot_idx}:instrument:param:{param_idx}"))
        }
        AppCommand::SetRackMacroPlockMulti { macro_idx, .. } => {
            Some(format!("rack-macro:{macro_idx}"))
        }
        AppCommand::SetRackSlotEffectPlockMulti {
            rack_slot_idx, effect_slot_idx, param_idx, ..
        } => Some(format!(
            "rack:{rack_slot_idx}:effect:{effect_slot_idx}:param:{param_idx}"
        )),
        _ => None,
    }
}

fn apply_coalesced_device_plock_commands(
    app: &mut App,
    commands: &[AppCommand],
    merge_suffix: &str,
    label: &str,
) -> Result<EditOutcome, EditError> {
    let first = commands.first().ok_or(EditError::UnsupportedCommand)?;
    let (track, affected) =
        device_plock_command_target(first).ok_or(EditError::UnsupportedCommand)?;
    if affected.is_empty() {
        return Ok(EditOutcome::NoOp);
    }
    for command in commands.iter().skip(1) {
        if device_plock_command_target(command) != Some((track, affected.clone())) {
            return Err(EditError::InvalidTarget(
                "device p-lock batch spans multiple tracks or step selections".to_string(),
            ));
        }
    }
    let track_id = app
        .track_registry
        .id_at(track)
        .ok_or(EditError::TrackOutOfRange { track })?;
    let pattern_id = app
        .state
        .effective_track_pattern_id(track)
        .ok_or(EditError::MissingTrackPattern)?;
    let target = TrackPatternId {
        track: track_id,
        pattern: pattern_id,
    };
    let merge_key = MergeKey::new(format!(
        "device-plock:{target:?}:{merge_suffix}:steps:{affected:?}",
    ));
    if app.history.active_gesture().map(|gesture| &gesture.merge_key) != Some(&merge_key) {
        finish_active_gesture(app);
    }
    let (current_before, current_registry_before) = app
        .state
        .capture_pattern_step_cells(track, pattern_id, &affected)
        .map_err(EditError::ReplayFailed)?;
    let original = app
        .history
        .active_gesture_patch(&merge_key)
        .and_then(|patch| match patch {
            EditPatch::StepCells(patch) if patch.target == target => Some(patch.clone()),
            _ => None,
        });

    for command in commands {
        super::command::execute_command(app, command.clone());
    }
    app.state.reconcile_plock_variant_registry_for_track(track);
    let (after, registry_after) = match app
        .state
        .capture_pattern_step_cells(track, pattern_id, &affected)
    {
        Ok(after) => after,
        Err(error) => {
            let rollback = affected
                .iter()
                .copied()
                .zip(current_before.iter().cloned())
                .collect::<Vec<_>>();
            return match app.state.restore_pattern_step_cells_no_publish(
                track,
                pattern_id,
                &rollback,
                &current_registry_before,
            ) {
                Ok(publish) => {
                    if publish {
                        app.state.publish_scheduler_snapshot();
                    }
                    Err(EditError::ReplayFailed(error))
                }
                Err(rollback_error) => Err(EditError::ReplayFailed(format!(
                    "{error}; rollback also failed: {rollback_error}"
                ))),
            };
        }
    };
    let changed = current_before
        .iter()
        .zip(&after)
        .any(|(before, after)| !step_snapshot_bit_exact_eq(before, after))
        || current_registry_before != registry_after;
    if !changed {
        return Ok(EditOutcome::NoOp);
    }

    let synchronized = affected
        .iter()
        .copied()
        .zip(after.iter().cloned())
        .collect::<Vec<_>>();
    let publish = match app.state.restore_pattern_step_cells_no_publish(
        track,
        pattern_id,
        &synchronized,
        &registry_after,
    ) {
        Ok(publish) => publish,
        Err(error) => {
            let rollback = affected
                .iter()
                .copied()
                .zip(current_before.iter().cloned())
                .collect::<Vec<_>>();
            return match app.state.restore_pattern_step_cells_no_publish(
                track,
                pattern_id,
                &rollback,
                &current_registry_before,
            ) {
                Ok(publish) => {
                    if publish {
                        app.state.publish_scheduler_snapshot();
                    }
                    Err(EditError::ReplayFailed(error))
                }
                Err(rollback_error) => Err(EditError::ReplayFailed(format!(
                    "{error}; rollback also failed: {rollback_error}"
                ))),
            };
        }
    };
    if publish {
        app.state.publish_scheduler_snapshot();
    }

    let original_before = |step: usize, current: &StepCellSnapshot| {
        original
            .as_ref()
            .and_then(|patch| patch.cells.iter().find(|cell| cell.step == step))
            .map(|cell| cell.before.clone())
            .unwrap_or_else(|| current.clone())
    };
    let cells = affected
        .iter()
        .copied()
        .zip(current_before.iter())
        .zip(after)
        .filter_map(|((step, current), after)| {
            let before = original_before(step, current);
            (!step_snapshot_bit_exact_eq(&before, &after)).then_some(StepCellDelta {
                step,
                before,
                after,
            })
        })
        .collect::<Vec<_>>();
    let variant_registry_before = original
        .as_ref()
        .map(|patch| patch.variant_registry_before.clone())
        .unwrap_or(current_registry_before);
    if cells.is_empty() && variant_registry_before == registry_after {
        app.history.discard_active_gesture_entry(&merge_key);
        return Ok(EditOutcome::NoOp);
    }
    let patch = StepCellsPatch {
        target,
        cells,
        variant_registry_before,
        variant_registry_after: registry_after,
    };
    let retained_bytes = patch.retained_bytes();
    ensure_coalescing_gesture(app, &merge_key);
    let history_move = app
        .history
        .stage_active_gesture(
            label,
            &merge_key,
            EditPatch::StepCells(patch),
            retained_bytes,
        )
        .ok_or(EditError::UnsupportedCommand)?;
    Ok(EditOutcome::Applied(history_move))
}

fn apply_coalesced_device_plock_command(
    app: &mut App,
    cmd: &AppCommand,
) -> Result<EditOutcome, EditError> {
    apply_coalesced_device_plock_commands(
        app,
        std::slice::from_ref(cmd),
        &device_plock_merge_suffix(cmd).ok_or(EditError::UnsupportedCommand)?,
        device_plock_label(cmd),
    )
}

pub fn apply_coalesced_device_plock_batch(
    app: &mut App,
    commands: &[AppCommand],
    gesture: &str,
    label: &str,
) -> Result<EditOutcome, EditError> {
    for command in commands {
        validate_device_command_target(app, command)?;
        if !matches!(history_policy(command), HistoryPolicy::Coalesce(_))
            || device_plock_command_target(command).is_none()
        {
            return Err(EditError::UnsupportedCommand);
        }
    }
    let mut parameter_targets = commands
        .iter()
        .map(|command| device_plock_merge_suffix(command).ok_or(EditError::UnsupportedCommand))
        .collect::<Result<Vec<_>, _>>()?;
    parameter_targets.sort();
    parameter_targets.dedup();
    apply_coalesced_device_plock_commands(
        app,
        commands,
        &format!("batch:{gesture}:{parameter_targets:?}"),
        label,
    )
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ResolvedDeviceTarget {
    id: DeviceId,
    track: usize,
    pattern: crate::sequencer::PatternId,
    slot_idx: Option<usize>,
}

fn device_value_command_track(cmd: &AppCommand) -> Option<usize> {
    match cmd {
        AppCommand::SetEffectParam { track, .. }
        | AppCommand::SetEffectTensorCell { track, .. }
        | AppCommand::SetMidiFxParam { track, .. }
        | AppCommand::SetMidiFxTensorCell { track, .. }
        | AppCommand::SetInstrumentParam { track, .. }
        | AppCommand::SetInstrumentKeyLock { track, .. }
        | AppCommand::SetInstrumentKeyLockMulti { track, .. }
        | AppCommand::ClearInstrumentKeyLock { track, .. }
        | AppCommand::ClearInstrumentKeyLocksForNote { track, .. }
        | AppCommand::StampInstrumentKeyLockVariant { track, .. }
        | AppCommand::ClearInstrumentKeyLockVariantsForNotes { track, .. }
        | AppCommand::SetInstrumentTensorCell { track, .. }
        | AppCommand::SetRackSlotGain { track, .. }
        | AppCommand::SetRackSlotPan { track, .. }
        | AppCommand::SetRackSlotMute { track, .. }
        | AppCommand::SetRackSlotSolo { track, .. }
        | AppCommand::SetRackSlotMaxPolyphony { track, .. }
        | AppCommand::SetRackSlotChokeGroup { track, .. }
        | AppCommand::SetRackSlotBaseNoteOffset { track, .. }
        | AppCommand::SetRackSlotInstrumentParam { track, .. }
        | AppCommand::SetRackSlotEffectParam { track, .. } => Some(*track),
        _ => None,
    }
}

fn resolve_device_value_target(
    app: &mut App,
    cmd: &AppCommand,
) -> Result<ResolvedDeviceTarget, EditError> {
    let track = device_value_command_track(cmd).ok_or(EditError::UnsupportedCommand)?;
    let track_id = app
        .track_registry
        .id_at(track)
        .ok_or(EditError::TrackOutOfRange { track })?;
    let pattern = app
        .state
        .effective_track_pattern_id(track)
        .ok_or(EditError::MissingTrackPattern)?;
    let (id, slot_idx) = match cmd {
        AppCommand::SetEffectParam { slot_idx, .. }
        | AppCommand::SetEffectTensorCell { slot_idx, .. } => (
            DeviceId::AudioEffect(app.device_registry.audio_effect(track_id, *slot_idx)),
            Some(*slot_idx),
        ),
        AppCommand::SetMidiFxParam { slot_idx, .. }
        | AppCommand::SetMidiFxTensorCell { slot_idx, .. } => (
            DeviceId::MidiEffect(app.device_registry.midi_effect(track_id, *slot_idx)),
            Some(*slot_idx),
        ),
        AppCommand::SetRackSlotGain { slot_idx, .. }
        | AppCommand::SetRackSlotPan { slot_idx, .. }
        | AppCommand::SetRackSlotMute { slot_idx, .. }
        | AppCommand::SetRackSlotSolo { slot_idx, .. }
        | AppCommand::SetRackSlotMaxPolyphony { slot_idx, .. }
        | AppCommand::SetRackSlotChokeGroup { slot_idx, .. }
        | AppCommand::SetRackSlotBaseNoteOffset { slot_idx, .. } => (
            DeviceId::RackSlot(app.device_registry.rack_slot(track_id, *slot_idx)),
            Some(*slot_idx),
        ),
        AppCommand::SetRackSlotInstrumentParam { slot_idx, .. } => (
            DeviceId::RackInstrument(app.device_registry.rack_slot(track_id, *slot_idx)),
            Some(*slot_idx),
        ),
        AppCommand::SetRackSlotEffectParam { rack_slot_idx, .. } => (
            DeviceId::RackSlot(app.device_registry.rack_slot(track_id, *rack_slot_idx)),
            Some(*rack_slot_idx),
        ),
        _ => (DeviceId::TrackInstrument(track_id), None),
    };
    let target = ResolvedDeviceTarget {
        id,
        track,
        pattern,
        slot_idx,
    };
    capture_device_value_snapshot(app, target)?;
    Ok(target)
}

fn device_value_label(cmd: &AppCommand) -> &'static str {
    match cmd {
        AppCommand::SetEffectParam { .. } => "Set effect parameter",
        AppCommand::SetEffectTensorCell { .. } => "Set effect tensor cell",
        AppCommand::SetMidiFxParam { .. } => "Set MIDI-FX parameter",
        AppCommand::SetMidiFxTensorCell { .. } => "Set MIDI-FX tensor cell",
        AppCommand::SetInstrumentParam { .. } => "Set instrument parameter",
        AppCommand::SetInstrumentTensorCell { .. } => "Set instrument tensor cell",
        AppCommand::SetInstrumentKeyLock { .. }
        | AppCommand::SetInstrumentKeyLockMulti { .. } => "Set instrument key lock",
        AppCommand::ClearInstrumentKeyLock { .. }
        | AppCommand::ClearInstrumentKeyLocksForNote { .. } => "Clear instrument key lock",
        AppCommand::StampInstrumentKeyLockVariant { .. } => "Stamp key-lock variant",
        AppCommand::ClearInstrumentKeyLockVariantsForNotes { .. } => "Clear key-lock variant",
        AppCommand::SetRackSlotGain { .. } => "Set rack slot gain",
        AppCommand::SetRackSlotPan { .. } => "Set rack slot pan",
        AppCommand::SetRackSlotMute { .. } => "Set rack slot mute",
        AppCommand::SetRackSlotSolo { .. } => "Set rack slot solo",
        AppCommand::SetRackSlotMaxPolyphony { .. } => "Set rack slot max polyphony",
        AppCommand::SetRackSlotChokeGroup { .. } => "Set rack slot choke group",
        AppCommand::SetRackSlotBaseNoteOffset { .. } => "Set rack slot base note",
        AppCommand::SetRackSlotInstrumentParam { .. } => "Set rack instrument parameter",
        AppCommand::SetRackSlotEffectParam { .. } => "Set rack effect parameter",
        _ => "Edit device values",
    }
}

fn device_value_merge_suffix(cmd: &AppCommand) -> String {
    match cmd {
        AppCommand::SetEffectParam { param_idx, .. }
        | AppCommand::SetMidiFxParam { param_idx, .. }
        | AppCommand::SetInstrumentParam { param_idx, .. }
        | AppCommand::SetRackSlotInstrumentParam { param_idx, .. } => {
            format!("param:{param_idx}")
        }
        AppCommand::SetRackSlotEffectParam {
            effect_slot_idx,
            param_idx,
            ..
        } => format!("effect:{effect_slot_idx}:param:{param_idx}"),
        AppCommand::SetInstrumentTensorCell {
            tensor_idx, cell_idx, ..
        } => format!("tensor:{tensor_idx}:cell:{cell_idx}"),
        AppCommand::SetEffectTensorCell {
            tensor_idx, cell_idx, ..
        }
        | AppCommand::SetMidiFxTensorCell {
            tensor_idx, cell_idx, ..
        } => format!("tensor:{tensor_idx}:cell:{cell_idx}"),
        _ => device_value_label(cmd).to_string(),
    }
}

fn capture_device_value_snapshot(
    app: &App,
    target: ResolvedDeviceTarget,
) -> Result<DeviceValueSnapshot, EditError> {
    match target.id {
        DeviceId::TrackInstrument(_) => app
            .state
            .capture_pattern_instrument_device_values(target.track, target.pattern)
            .map(DeviceValueSnapshot::Instrument)
            .map_err(EditError::ReplayFailed),
        DeviceId::AudioEffect(_) => app
            .state
            .capture_pattern_effect_device_values(
                target.track,
                target.pattern,
                target.slot_idx.ok_or(EditError::UnsupportedCommand)?,
            )
            .map(DeviceValueSnapshot::Slot)
            .map_err(EditError::ReplayFailed),
        DeviceId::MidiEffect(_) => app
            .state
            .capture_pattern_midi_fx_device_values(
                target.track,
                target.pattern,
                target.slot_idx.ok_or(EditError::UnsupportedCommand)?,
            )
            .map(DeviceValueSnapshot::Slot)
            .map_err(EditError::ReplayFailed),
        DeviceId::RackSlot(_) | DeviceId::RackInstrument(_) => app
            .state
            .capture_pattern_rack_slot_values(
                target.track,
                target.pattern,
                target.slot_idx.ok_or(EditError::UnsupportedCommand)?,
            )
            .map(DeviceValueSnapshot::RackSlot)
            .map_err(EditError::ReplayFailed),
    }
}

fn restore_device_value_snapshot(
    app: &mut App,
    target: ResolvedDeviceTarget,
    snapshot: &DeviceValueSnapshot,
) -> Result<bool, EditError> {
    match (target.id, snapshot) {
        (DeviceId::TrackInstrument(_), DeviceValueSnapshot::Instrument(snapshot)) => app
            .state
            .restore_pattern_instrument_device_values_no_publish(
                target.track,
                target.pattern,
                snapshot,
            )
            .map_err(EditError::ReplayFailed),
        (DeviceId::AudioEffect(_), DeviceValueSnapshot::Slot(snapshot)) => app
            .state
            .restore_pattern_effect_device_values_no_publish(
                target.track,
                target.pattern,
                target.slot_idx.ok_or(EditError::UnsupportedCommand)?,
                snapshot,
            )
            .map_err(EditError::ReplayFailed),
        (DeviceId::MidiEffect(_), DeviceValueSnapshot::Slot(snapshot)) => app
            .state
            .restore_pattern_midi_fx_device_values_no_publish(
                target.track,
                target.pattern,
                target.slot_idx.ok_or(EditError::UnsupportedCommand)?,
                snapshot,
            )
            .map_err(EditError::ReplayFailed),
        (
            DeviceId::RackSlot(_) | DeviceId::RackInstrument(_),
            DeviceValueSnapshot::RackSlot(snapshot),
        ) => app
            .state
            .restore_pattern_rack_slot_values_no_publish(
                target.track,
                target.pattern,
                target.slot_idx.ok_or(EditError::UnsupportedCommand)?,
                snapshot,
            )
            .map_err(EditError::ReplayFailed),
        _ => Err(EditError::ReplayFailed(
            "device history snapshot did not match its target".to_string(),
        )),
    }
}

fn device_command_changes_key_locks(cmd: &AppCommand) -> bool {
    matches!(
        cmd,
        AppCommand::SetInstrumentKeyLock { .. }
            | AppCommand::SetInstrumentKeyLockMulti { .. }
            | AppCommand::ClearInstrumentKeyLock { .. }
            | AppCommand::ClearInstrumentKeyLocksForNote { .. }
            | AppCommand::StampInstrumentKeyLockVariant { .. }
            | AppCommand::ClearInstrumentKeyLockVariantsForNotes { .. }
    )
}

fn apply_recorded_device_value_commands(
    app: &mut App,
    commands: &[AppCommand],
    merge_key: Option<MergeKey>,
    merge_suffix: &str,
    label: &str,
) -> Result<EditOutcome, EditError> {
    let first = commands.first().ok_or(EditError::UnsupportedCommand)?;
    let target = resolve_device_value_target(app, first)?;
    for command in commands.iter().skip(1) {
        if resolve_device_value_target(app, command)? != target {
            return Err(EditError::InvalidTarget(
                "device-value batch spans multiple devices or patterns".to_string(),
            ));
        }
    }
    let current_before = capture_device_value_snapshot(app, target)?;
    let merge_key = merge_key.map(|_| {
        MergeKey::new(format!(
            "device:{:?}:pattern:{}:{}",
            target.id,
            target.pattern.0,
            merge_suffix,
        ))
    });
    if let Some(key) = merge_key.as_ref() {
        if app.history.active_gesture().map(|gesture| &gesture.merge_key) != Some(key) {
            finish_active_gesture(app);
        }
    }
    let entry_before = merge_key
        .as_ref()
        .and_then(|key| app.history.active_gesture_patch(key))
        .and_then(|patch| match patch {
            EditPatch::DeviceValues(patch)
                if patch.target == target.id && patch.pattern == target.pattern =>
            {
                Some(patch.before.clone())
            }
            _ => None,
        })
        .unwrap_or_else(|| current_before.clone());

    for command in commands {
        super::command::execute_command(app, command.clone());
    }
    if commands.iter().any(device_command_changes_key_locks) {
        app.state
            .reconcile_key_lock_variant_registry_for_track(target.track);
    }
    let after = capture_device_value_snapshot(app, target)?;
    if current_before.bit_exact_eq(&after) {
        return Ok(EditOutcome::NoOp);
    }
    if let Err(error) = restore_device_value_snapshot(app, target, &after) {
        let _ = restore_device_value_snapshot(app, target, &current_before);
        return Err(error);
    }

    let patch = DeviceValuesPatch {
        target: target.id,
        pattern: target.pattern,
        before: entry_before,
        after,
    };
    let retained_bytes = patch.retained_bytes();
    if let Some(key) = merge_key {
        if patch.before.bit_exact_eq(&patch.after)
            && app.history.discard_active_gesture_entry(&key)
        {
            return Ok(EditOutcome::NoOp);
        }
        ensure_coalescing_gesture(app, &key);
        let history_move = app
            .history
            .stage_active_gesture(
                label,
                &key,
                EditPatch::DeviceValues(patch),
                retained_bytes,
            )
            .ok_or(EditError::UnsupportedCommand)?;
        return Ok(EditOutcome::Applied(history_move));
    }
    finish_active_gesture(app);
    app.state.publish_scheduler_snapshot();
    let history_move = app.history.commit(
        label,
        None,
        EditPatch::DeviceValues(patch),
        retained_bytes,
    );
    Ok(EditOutcome::Applied(history_move))
}

fn apply_recorded_device_value_command(
    app: &mut App,
    cmd: &AppCommand,
    merge_key: Option<MergeKey>,
) -> Result<EditOutcome, EditError> {
    apply_recorded_device_value_commands(
        app,
        std::slice::from_ref(cmd),
        merge_key,
        &device_value_merge_suffix(cmd),
        device_value_label(cmd),
    )
}

pub fn apply_coalesced_device_value_batch(
    app: &mut App,
    commands: &[AppCommand],
    gesture: &str,
    label: &str,
) -> Result<EditOutcome, EditError> {
    for command in commands {
        validate_device_command_target(app, command)?;
        if !matches!(history_policy(command), HistoryPolicy::Coalesce(_))
            || device_value_command_track(command).is_none()
        {
            return Err(EditError::UnsupportedCommand);
        }
    }
    let mut parameter_targets = commands
        .iter()
        .map(device_value_merge_suffix)
        .collect::<Vec<_>>();
    parameter_targets.sort();
    parameter_targets.dedup();
    apply_recorded_device_value_commands(
        app,
        commands,
        Some(MergeKey::new(gesture)),
        &format!("batch:{gesture}:{parameter_targets:?}"),
        label,
    )
}

pub fn apply_recorded_instrument_values_mutation(
    app: &mut App,
    track: usize,
    label: impl Into<String>,
    mutate: impl FnOnce(&mut App) -> Result<(), String>,
) -> Result<EditOutcome, EditError> {
    let label = label.into();
    let track_id = app
        .track_registry
        .id_at(track)
        .ok_or(EditError::TrackOutOfRange { track })?;
    let pattern = app
        .state
        .effective_track_pattern_id(track)
        .ok_or(EditError::MissingTrackPattern)?;
    let target = ResolvedDeviceTarget {
        id: DeviceId::TrackInstrument(track_id),
        track,
        pattern,
        slot_idx: None,
    };
    let before = capture_device_value_snapshot(app, target)?;
    finish_active_gesture(app);
    if let Err(error) = mutate(app) {
        let _ = restore_device_value_snapshot(app, target, &before);
        let _ = push_live_device_values(app, target, Some(&before));
        return Err(EditError::ReplayFailed(error));
    }
    app.state.reconcile_key_lock_variant_registry_for_track(track);
    let after = capture_device_value_snapshot(app, target)?;
    if before.bit_exact_eq(&after) {
        return Ok(EditOutcome::NoOp);
    }
    if let Err(error) = restore_device_value_snapshot(app, target, &after) {
        let _ = restore_device_value_snapshot(app, target, &before);
        let _ = push_live_device_values(app, target, Some(&before));
        return Err(error);
    }
    app.state.publish_scheduler_snapshot();
    let patch = DeviceValuesPatch {
        target: target.id,
        pattern,
        before,
        after,
    };
    let retained_bytes = patch.retained_bytes();
    let history_move = app.history.commit(
        label,
        None,
        EditPatch::DeviceValues(patch),
        retained_bytes,
    );
    Ok(EditOutcome::Applied(history_move))
}

pub fn apply_recorded_track_effect_ir_mutation(
    app: &mut App,
    track: usize,
    slot_idx: usize,
    source_path: &std::path::Path,
    reference: &str,
) -> Result<EditOutcome, EditError> {
    let track_id = app
        .track_registry
        .id_at(track)
        .ok_or(EditError::TrackOutOfRange { track })?;
    let pattern = app
        .state
        .effective_track_pattern_id(track)
        .ok_or(EditError::MissingTrackPattern)?;
    let target = ResolvedDeviceTarget {
        id: DeviceId::AudioEffect(app.device_registry.audio_effect(track_id, slot_idx)),
        track,
        pattern,
        slot_idx: Some(slot_idx),
    };
    let before = capture_device_value_snapshot(app, target)?;
    let prepared = std::sync::Arc::new(
        crate::effects::conv_reverb::prepare_ir(source_path, app.graph.sample_rate)
            .map_err(EditError::InvalidTarget)?,
    );
    finish_active_gesture(app);
    let node_id = app
        .state
        .pattern
        .effect_chains
        .get(track)
        .and_then(|chain| chain.get(slot_idx))
        .map(|slot| slot.node_id.load(Ordering::Relaxed) as i32)
        .ok_or_else(|| EditError::InvalidTarget("Track effect slot not found".to_string()))?;
    app.apply_prepared_conv_reverb_ir_to_node(
        node_id,
        prepared,
        reference,
        source_path,
    )
    .map_err(EditError::InvalidTarget)?;
    let after = capture_device_value_snapshot(app, target)?;
    if before.bit_exact_eq(&after) {
        return Ok(EditOutcome::NoOp);
    }
    restore_device_value_snapshot(app, target, &after)?;
    app.state.publish_scheduler_snapshot();
    let patch = DeviceValuesPatch {
        target: target.id,
        pattern,
        before,
        after,
    };
    let retained_bytes = patch.retained_bytes();
    let history_move = app.history.commit(
        format!("Load effect IR '{reference}'"),
        None,
        EditPatch::DeviceValues(patch),
        retained_bytes,
    );
    Ok(EditOutcome::Applied(history_move))
}

fn track_params_command_track(cmd: &AppCommand) -> Option<usize> {
    match cmd {
        AppCommand::ToggleTrackGate { track }
        | AppCommand::ToggleTrackPolyphonic { track }
        | AppCommand::ToggleTrackMute { track }
        | AppCommand::ToggleTrackSolo { track }
        | AppCommand::AdjustTrackMaxPolyphony { track, .. }
        | AppCommand::SetTrackMaxPolyphony { track, .. }
        | AppCommand::SetTrackAttack { track, .. }
        | AppCommand::AdjustTrackAttack { track, .. }
        | AppCommand::SetTrackRelease { track, .. }
        | AppCommand::AdjustTrackRelease { track, .. }
        | AppCommand::SetTrackSwing { track, .. }
        | AppCommand::AdjustTrackSwing { track, .. }
        | AppCommand::SetTrackSwingResolution { track, .. }
        | AppCommand::NextTrackSwingResolution { track }
        | AppCommand::PrevTrackSwingResolution { track }
        | AppCommand::SetTrackVolume { track, .. }
        | AppCommand::AdjustTrackVolume { track, .. }
        | AppCommand::SetTrackPan { track, .. }
        | AppCommand::AdjustTrackPan { track, .. }
        | AppCommand::SetTrackSend { track, .. }
        | AppCommand::AdjustTrackSend { track, .. }
        | AppCommand::SetTrackOutput { track, .. }
        | AppCommand::SetTrackSends { track, .. }
        | AppCommand::SetTrackTimebase { track, .. }
        | AppCommand::NextTrackTimebase { track }
        | AppCommand::PrevTrackTimebase { track }
        | AppCommand::SetTrackFtsScale { track, .. }
        | AppCommand::SetTrackAccumIdx { track, .. }
        | AppCommand::SetTrackAccumLimit { track, .. }
        | AppCommand::AdjustTrackAccumLimit { track, .. }
        | AppCommand::SetTrackAccumMode { track, .. }
        | AppCommand::SetTrackMuteGroup { track, .. }
        | AppCommand::SetTrackGlobalTranspose { track, .. }
        | AppCommand::SetInstrumentBaseNoteOffset { track, .. } => Some(*track),
        _ => None,
    }
}

fn track_params_label(cmd: &AppCommand) -> &'static str {
    match cmd {
        AppCommand::ToggleTrackGate { .. } => "Toggle track gate",
        AppCommand::ToggleTrackPolyphonic { .. } => "Toggle track polyphony",
        AppCommand::ToggleTrackMute { .. } => "Toggle track mute",
        AppCommand::ToggleTrackSolo { .. } => "Toggle track solo",
        AppCommand::AdjustTrackMaxPolyphony { .. }
        | AppCommand::SetTrackMaxPolyphony { .. } => "Set track max polyphony",
        AppCommand::SetTrackAttack { .. } | AppCommand::AdjustTrackAttack { .. } => {
            "Set track attack"
        }
        AppCommand::SetTrackRelease { .. } | AppCommand::AdjustTrackRelease { .. } => {
            "Set track release"
        }
        AppCommand::SetTrackSwing { .. } | AppCommand::AdjustTrackSwing { .. } => {
            "Set track swing"
        }
        AppCommand::SetTrackSwingResolution { .. }
        | AppCommand::NextTrackSwingResolution { .. }
        | AppCommand::PrevTrackSwingResolution { .. } => "Set track swing resolution",
        AppCommand::SetTrackVolume { .. } | AppCommand::AdjustTrackVolume { .. } => {
            "Set track volume"
        }
        AppCommand::SetTrackPan { .. } | AppCommand::AdjustTrackPan { .. } => "Set track pan",
        AppCommand::SetTrackSend { .. } | AppCommand::AdjustTrackSend { .. } => "Set track send",
        AppCommand::SetTrackOutput { .. } => "Set track output",
        AppCommand::SetTrackSends { .. } => "Set track sends",
        AppCommand::SetTrackTimebase { .. }
        | AppCommand::NextTrackTimebase { .. }
        | AppCommand::PrevTrackTimebase { .. } => "Set track timebase",
        AppCommand::SetTrackFtsScale { .. } => "Set track FTS scale",
        AppCommand::SetTrackAccumIdx { .. } => "Set track accumulator",
        AppCommand::SetTrackAccumLimit { .. } | AppCommand::AdjustTrackAccumLimit { .. } => {
            "Set accumulator limit"
        }
        AppCommand::SetTrackAccumMode { .. } => "Set accumulator mode",
        AppCommand::SetTrackMuteGroup { .. } => "Set track mute group",
        AppCommand::SetTrackGlobalTranspose { .. } => "Set global transpose",
        AppCommand::SetInstrumentBaseNoteOffset { .. } => "Set instrument base note",
        _ => "Set track parameters",
    }
}

fn track_params_bit_exact_eq(left: &TrackParamsSnapshot, right: &TrackParamsSnapshot) -> bool {
    encode_track_params(left) == encode_track_params(right)
}

fn scheduler_track_params_changed(
    before: &TrackParamsSnapshot,
    after: &TrackParamsSnapshot,
) -> bool {
    let mut normalized = before.clone();
    normalized.volume = after.volume;
    normalized.pan = after.pan;
    normalized.mute = after.mute;
    normalized.solo = after.solo;
    normalized.send = after.send;
    normalized.sends = after.sends.clone();
    !track_params_bit_exact_eq(&normalized, after)
}

fn apply_live_track_param_effects(
    app: &mut App,
    track: usize,
    before: &TrackParamsSnapshot,
    after: &TrackParamsSnapshot,
    base_note_before: u32,
    base_note_after: u32,
) -> bool {
    if before.volume.to_bits() != after.volume.to_bits() {
        app.push_track_volume(track);
    }
    if before.pan.to_bits() != after.pan.to_bits() {
        app.push_track_pan(track);
    }
    if before.send.to_bits() != after.send.to_bits() {
        app.push_send_gain(track);
    }
    if before.mute != after.mute {
        app.push_track_mute(track);
    }
    if before.solo != after.solo {
        app.push_track_solo_mutes();
    }
    if before.output != after.output {
        app.graph_controller().apply_track_output_routing(track);
    }
    let sends_changed = before.sends.len() != after.sends.len()
        || before.sends.iter().zip(&after.sends).any(|(left, right)| {
            left.destination != right.destination || left.amount.to_bits() != right.amount.to_bits()
        });
    if sends_changed {
        app.graph_controller().apply_track_bus_sends(track);
    }
    if before.accumulator_idx != after.accumulator_idx
        || before.script_accumulator_name != after.script_accumulator_name
    {
        app.state.request_accumulator_reset(track);
    }
    scheduler_track_params_changed(before, after) || base_note_before != base_note_after
}

fn capture_transport_authoring(app: &App) -> TransportAuthoringSnapshot {
    TransportAuthoringSnapshot {
        bpm: app.state.transport.bpm.load(Ordering::Relaxed),
        master_volume_bits: app.state.transport.master_volume.load(Ordering::Relaxed),
    }
}

fn bus_mixer_command_bus(cmd: &AppCommand) -> Option<BusId> {
    match cmd {
        AppCommand::SetBusVolume { bus, .. }
        | AppCommand::ToggleBusMute { bus }
        | AppCommand::ToggleBusSolo { bus } => Some(*bus),
        _ => None,
    }
}

fn bus_mixer_label(cmd: &AppCommand) -> &'static str {
    match cmd {
        AppCommand::SetBusVolume { .. } => "Set bus volume",
        AppCommand::ToggleBusMute { .. } => "Toggle bus mute",
        AppCommand::ToggleBusSolo { .. } => "Toggle bus solo",
        _ => "Edit bus mixer",
    }
}

fn capture_bus_mixer(app: &App, bus: BusId) -> Result<BusMixerSnapshot, EditError> {
    let channel = app
        .buses
        .iter()
        .find(|channel| channel.id == bus)
        .ok_or(EditError::MissingStableBus { bus })?;
    Ok(BusMixerSnapshot {
        volume_bits: channel.volume.to_bits(),
        mute: channel.mute,
        solo: channel.solo,
    })
}

fn apply_recorded_bus_mixer_command(
    app: &mut App,
    cmd: &AppCommand,
    merge_key: Option<MergeKey>,
) -> Result<EditOutcome, EditError> {
    let bus = bus_mixer_command_bus(cmd).ok_or(EditError::UnsupportedCommand)?;
    let merge_key = merge_key.map(|_| {
        MergeKey::new(format!("bus:{}:{}", bus.0, bus_mixer_label(cmd)))
    });
    if let Some(key) = merge_key.as_ref() {
        if app.history.active_gesture().map(|gesture| &gesture.merge_key) != Some(key) {
            finish_active_gesture(app);
        }
    }
    let current_before = capture_bus_mixer(app, bus)?;
    let entry_before = merge_key
        .as_ref()
        .and_then(|key| app.history.active_gesture_patch(key))
        .and_then(|patch| match patch {
            EditPatch::BusMixer(patch) if patch.target == bus => Some(patch.before),
            _ => None,
        })
        .unwrap_or(current_before);

    super::command::execute_command(app, cmd.clone());
    let after = capture_bus_mixer(app, bus)?;
    if current_before == after {
        return Ok(EditOutcome::NoOp);
    }

    let patch = BusMixerPatch {
        target: bus,
        before: entry_before,
        after,
    };
    let retained_bytes = patch.retained_bytes();
    if let Some(key) = merge_key {
        if patch.before == patch.after && app.history.discard_active_gesture_entry(&key) {
            return Ok(EditOutcome::NoOp);
        }
        ensure_coalescing_gesture(app, &key);
        let history_move = app
            .history
            .stage_active_gesture(
                bus_mixer_label(cmd),
                &key,
                EditPatch::BusMixer(patch),
                retained_bytes,
            )
            .ok_or(EditError::UnsupportedCommand)?;
        return Ok(EditOutcome::Applied(history_move));
    }
    finish_active_gesture(app);
    let history_move = app.history.commit(
        bus_mixer_label(cmd),
        None,
        EditPatch::BusMixer(patch),
        retained_bytes,
    );
    Ok(EditOutcome::Applied(history_move))
}

fn app_bus_effect_gesture_before(
    app: &App,
    key: &MergeKey,
    bus: BusId,
    instance: crate::sequencer::EffectInstanceId,
    scene: usize,
) -> Option<crate::effects::EffectSlotValuesSnapshot> {
    app.history.active_gesture_patch(key).and_then(|patch| match patch {
        EditPatch::BusEffectValues(patch)
            if patch.bus == bus && patch.instance == instance && patch.scene == scene =>
        {
            Some(patch.before.clone())
        }
        _ => None,
    })
}

static NEXT_HISTORY_GESTURE_ID: AtomicU64 = AtomicU64::new(1);

fn ensure_coalescing_gesture(app: &mut App, merge_key: &MergeKey) {
    if app.history.active_gesture().map(|gesture| &gesture.merge_key) == Some(merge_key) {
        return;
    }
    finish_active_gesture(app);
    let _ = app.history.begin_gesture(ActiveGesture {
        id: GestureId(NEXT_HISTORY_GESTURE_ID.fetch_add(1, Ordering::Relaxed)),
        merge_key: merge_key.clone(),
    });
}

fn apply_recorded_track_params_command(
    app: &mut App,
    cmd: &AppCommand,
    merge_key: Option<MergeKey>,
) -> Result<EditOutcome, EditError> {
    let track = track_params_command_track(cmd).ok_or(EditError::UnsupportedCommand)?;
    let track_id = app
        .track_registry
        .id_at(track)
        .ok_or(EditError::TrackOutOfRange { track })?;
    let pattern_id = app
        .state
        .effective_track_pattern_id(track)
        .ok_or(EditError::MissingTrackPattern)?;
    let target = TrackPatternId { track: track_id, pattern: pattern_id };
    let merge_key = merge_key.map(|_| {
        MergeKey::new(format!(
            "track-pattern:{}:{}:{}",
            target.track.0,
            target.pattern.0,
            track_params_label(cmd),
        ))
    });
    if let Some(key) = merge_key.as_ref() {
        if app.history.active_gesture().map(|gesture| &gesture.merge_key) != Some(key) {
            finish_active_gesture(app);
        }
    }
    let current_before = app
        .state
        .capture_pattern_track_params(track, pattern_id)
        .map_err(EditError::ReplayFailed)?;
    let current_base_before = app
        .state
        .capture_pattern_instrument_base_note_offset(track, pattern_id)
        .map_err(EditError::ReplayFailed)?
        .to_bits();
    let (entry_before, entry_base_before) = merge_key
        .as_ref()
        .and_then(|key| app.history.active_gesture_patch(key))
        .and_then(|patch| match patch {
            EditPatch::TrackParams(patch) if patch.target == target => Some((
                patch.before.clone(),
                patch.instrument_base_note_offset_before,
            )),
            _ => None,
        })
        .unwrap_or_else(|| (current_before.clone(), current_base_before));

    super::command::execute_command(app, cmd.clone());
    let after = app
        .state
        .capture_pattern_track_params(track, pattern_id)
        .map_err(EditError::ReplayFailed)?;
    let base_after = app
        .state
        .capture_pattern_instrument_base_note_offset(track, pattern_id)
        .map_err(EditError::ReplayFailed)?
        .to_bits();
    if track_params_bit_exact_eq(&current_before, &after) && current_base_before == base_after {
        return Ok(EditOutcome::NoOp);
    }

    app.state
        .restore_pattern_track_params_no_publish(track, pattern_id, &after)
        .map_err(EditError::ReplayFailed)?;
    app.state
        .restore_pattern_instrument_base_note_offset_no_publish(
            track,
            pattern_id,
            f32::from_bits(base_after),
        )
        .map_err(EditError::ReplayFailed)?;
    if merge_key.is_none()
        && (scheduler_track_params_changed(&current_before, &after)
            || current_base_before != base_after)
    {
        app.state.publish_scheduler_snapshot();
    }

    let patch = TrackParamsPatch {
        target,
        before: entry_before,
        after,
        instrument_base_note_offset_before: entry_base_before,
        instrument_base_note_offset_after: base_after,
    };
    let retained_bytes = patch.retained_bytes();
    if let Some(key) = merge_key {
        if track_params_bit_exact_eq(&patch.before, &patch.after)
            && patch.instrument_base_note_offset_before
                == patch.instrument_base_note_offset_after
            && app.history.discard_active_gesture_entry(&key)
        {
            return Ok(EditOutcome::NoOp);
        }
        ensure_coalescing_gesture(app, &key);
        let move_result = app.history.stage_active_gesture(
            track_params_label(cmd),
            &key,
            EditPatch::TrackParams(patch),
            retained_bytes,
        ).ok_or(EditError::UnsupportedCommand)?;
        return Ok(EditOutcome::Applied(move_result));
    }
    finish_active_gesture(app);
    let move_result = app.history.commit(
        track_params_label(cmd),
        None,
        EditPatch::TrackParams(patch),
        retained_bytes,
    );
    Ok(EditOutcome::Applied(move_result))
}

pub fn apply_recorded_track_params_batch(
    app: &mut App,
    commands: &[AppCommand],
) -> Result<EditOutcome, EditError> {
    if commands.is_empty() {
        return Ok(EditOutcome::NoOp);
    }
    let label = track_params_label(&commands[0]);
    if commands.iter().any(|command| {
        track_params_command_track(command).is_none() || track_params_label(command) != label
    }) {
        return Err(EditError::UnsupportedCommand);
    }

    let mut resolved = Vec::with_capacity(commands.len());
    for command in commands {
        let track = track_params_command_track(command).ok_or(EditError::UnsupportedCommand)?;
        let track_id = app
            .track_registry
            .id_at(track)
            .ok_or(EditError::TrackOutOfRange { track })?;
        let pattern_id = app
            .state
            .effective_track_pattern_id(track)
            .ok_or(EditError::MissingTrackPattern)?;
        let target = TrackPatternId { track: track_id, pattern: pattern_id };
        let before = app
            .state
            .capture_pattern_track_params(track, pattern_id)
            .map_err(EditError::ReplayFailed)?;
        let base_before = app
            .state
            .capture_pattern_instrument_base_note_offset(track, pattern_id)
            .map_err(EditError::ReplayFailed)?
            .to_bits();
        resolved.push((track, target, before, base_before));
    }
    resolved.sort_by_key(|(_, target, _, _)| target.track);
    resolved.dedup_by_key(|(_, target, _, _)| target.track);
    if resolved.len() != commands.len() {
        return Err(EditError::InvalidTarget(
            "track-parameter batch contains duplicate tracks".to_string(),
        ));
    }
    let merge_key = MergeKey::new(format!(
        "track-batch:{label}:{:?}",
        resolved
            .iter()
            .map(|(_, target, _, _)| *target)
            .collect::<Vec<_>>()
    ));
    if app.history.active_gesture().map(|gesture| &gesture.merge_key) != Some(&merge_key) {
        finish_active_gesture(app);
    }
    let original = app
        .history
        .active_gesture_patch(&merge_key)
        .and_then(|patch| match patch {
            EditPatch::TrackParamsBatch(patch) => Some(patch.tracks.clone()),
            _ => None,
        });

    for command in commands {
        super::command::execute_command(app, command.clone());
    }

    let mut patches = Vec::with_capacity(resolved.len());
    let mut changed = false;
    for (track, target, current_before, current_base_before) in resolved {
        let after = app
            .state
            .capture_pattern_track_params(track, target.pattern)
            .map_err(EditError::ReplayFailed)?;
        let base_after = app
            .state
            .capture_pattern_instrument_base_note_offset(track, target.pattern)
            .map_err(EditError::ReplayFailed)?
            .to_bits();
        changed |= !track_params_bit_exact_eq(&current_before, &after)
            || current_base_before != base_after;
        app.state
            .restore_pattern_track_params_no_publish(track, target.pattern, &after)
            .map_err(EditError::ReplayFailed)?;
        app.state
            .restore_pattern_instrument_base_note_offset_no_publish(
                track,
                target.pattern,
                f32::from_bits(base_after),
            )
            .map_err(EditError::ReplayFailed)?;
        let entry_before = original
            .as_ref()
            .and_then(|patches| patches.iter().find(|patch| patch.target == target));
        patches.push(TrackParamsPatch {
            target,
            before: entry_before
                .map(|patch| patch.before.clone())
                .unwrap_or(current_before),
            after,
            instrument_base_note_offset_before: entry_before
                .map(|patch| patch.instrument_base_note_offset_before)
                .unwrap_or(current_base_before),
            instrument_base_note_offset_after: base_after,
        });
    }
    if !changed {
        return Ok(EditOutcome::NoOp);
    }
    let patch = TrackParamsBatchPatch { tracks: patches };
    let net_no_op = patch.tracks.iter().all(|patch| {
        track_params_bit_exact_eq(&patch.before, &patch.after)
            && patch.instrument_base_note_offset_before == patch.instrument_base_note_offset_after
    });
    if net_no_op && app.history.discard_active_gesture_entry(&merge_key) {
        return Ok(EditOutcome::NoOp);
    }
    let retained_bytes = patch.retained_bytes();
    ensure_coalescing_gesture(app, &merge_key);
    let history_move = app.history.stage_active_gesture(
        label,
        &merge_key,
        EditPatch::TrackParamsBatch(patch),
        retained_bytes,
    ).ok_or(EditError::UnsupportedCommand)?;
    Ok(EditOutcome::Applied(history_move))
}

fn apply_recorded_transport_command(
    app: &mut App,
    cmd: &AppCommand,
    merge_key: Option<MergeKey>,
) -> Result<EditOutcome, EditError> {
    if let Some(key) = merge_key.as_ref() {
        if app.history.active_gesture().map(|gesture| &gesture.merge_key) != Some(key) {
            finish_active_gesture(app);
        }
    }
    let current_before = capture_transport_authoring(app);
    let entry_before = merge_key
        .as_ref()
        .and_then(|key| app.history.active_gesture_patch(key))
        .and_then(|patch| match patch {
            EditPatch::TransportParams(patch) => Some(patch.before),
            _ => None,
        })
        .unwrap_or(current_before);
    super::command::execute_command(app, cmd.clone());
    let after = capture_transport_authoring(app);
    if current_before == after {
        return Ok(EditOutcome::NoOp);
    }
    if merge_key.is_none() && current_before.bpm != after.bpm {
        app.state.publish_scheduler_snapshot();
    }
    let label = match cmd {
        AppCommand::SetBpm { .. } => "Set BPM",
        _ => "Set master volume",
    };
    let patch = TransportParamsPatch { before: entry_before, after };
    let retained_bytes = patch.retained_bytes();
    if let Some(key) = merge_key {
        if patch.before == patch.after && app.history.discard_active_gesture_entry(&key) {
            return Ok(EditOutcome::NoOp);
        }
        ensure_coalescing_gesture(app, &key);
        let move_result = app.history.stage_active_gesture(
            label,
            &key,
            EditPatch::TransportParams(patch),
            retained_bytes,
        ).ok_or(EditError::UnsupportedCommand)?;
        return Ok(EditOutcome::Applied(move_result));
    }
    finish_active_gesture(app);
    let move_result = app.history.commit(
        label,
        None,
        EditPatch::TransportParams(patch),
        retained_bytes,
    );
    Ok(EditOutcome::Applied(move_result))
}

fn pattern_geometry_track(cmd: &AppCommand) -> Option<usize> {
    match cmd {
        AppCommand::DuplicateTrackPattern { track }
        | AppCommand::HalveTrackPattern { track }
        | AppCommand::SetTrackNumSteps { track, .. }
        | AppCommand::AdjustTrackNumSteps { track, .. } => Some(*track),
        _ => None,
    }
}

fn pattern_geometry_label(cmd: &AppCommand) -> &'static str {
    match cmd {
        AppCommand::DuplicateTrackPattern { .. } => "Duplicate track pattern",
        AppCommand::HalveTrackPattern { .. } => "Halve track pattern",
        AppCommand::SetTrackNumSteps { .. } | AppCommand::AdjustTrackNumSteps { .. } => {
            "Set track pattern length"
        }
        _ => "Edit track pattern geometry",
    }
}

fn apply_recorded_pattern_geometry_command(
    app: &mut App,
    cmd: &AppCommand,
) -> Result<EditOutcome, EditError> {
    let track = pattern_geometry_track(cmd).ok_or(EditError::UnsupportedCommand)?;
    let track_id = app
        .track_registry
        .id_at(track)
        .ok_or(EditError::TrackOutOfRange { track })?;
    let pattern_id = app
        .state
        .effective_track_pattern_id(track)
        .ok_or(EditError::MissingTrackPattern)?;
    let target = TrackPatternId {
        track: track_id,
        pattern: pattern_id,
    };
    let steps = (0..MAX_STEPS).collect::<Vec<_>>();
    let (before, registry_before) = app
        .state
        .capture_pattern_step_cells(track, pattern_id, &steps)
        .map_err(EditError::ReplayFailed)?;
    let num_steps_before = app
        .state
        .capture_pattern_num_steps(track, pattern_id)
        .map_err(EditError::ReplayFailed)?;

    super::command::execute_command(app, cmd.clone());
    app.state.reconcile_plock_variant_registry_for_track(track);

    let (after, registry_after) = app
        .state
        .capture_pattern_step_cells(track, pattern_id, &steps)
        .map_err(EditError::ReplayFailed)?;
    let num_steps_after = app
        .state
        .capture_pattern_num_steps(track, pattern_id)
        .map_err(EditError::ReplayFailed)?;
    let cells = steps
        .into_iter()
        .zip(before)
        .zip(after)
        .filter_map(|((step, before), after)| {
            (!step_snapshot_bit_exact_eq(&before, &after)).then_some(StepCellDelta {
                step,
                before,
                after,
            })
        })
        .collect::<Vec<_>>();
    if cells.is_empty() && num_steps_before == num_steps_after {
        return Ok(EditOutcome::NoOp);
    }
    let patch = PatternGeometryPatch {
        target,
        num_steps_before,
        num_steps_after,
        cells: StepCellsPatch {
            target,
            cells,
            variant_registry_before: registry_before,
            variant_registry_after: registry_after,
        },
    };
    if let Err(error) = replay_pattern_geometry_patch(app, &patch, ApplyMode::Redo) {
        return match replay_pattern_geometry_patch(app, &patch, ApplyMode::Undo) {
            Ok(_) => Err(error),
            Err(rollback_error) => Err(EditError::ReplayFailed(format!(
                "{error:?}; rollback also failed: {rollback_error:?}"
            ))),
        };
    }
    let retained_bytes = patch.retained_bytes();
    finish_active_gesture(app);
    let history_move = app.history.commit(
        pattern_geometry_label(cmd),
        None,
        EditPatch::PatternGeometry(patch),
        retained_bytes,
    );
    Ok(EditOutcome::Applied(history_move))
}

pub fn apply_recorded_step_mutation(
    app: &mut App,
    track: usize,
    steps: &[usize],
    label: &'static str,
    mutate: impl FnOnce(&mut App) -> Result<(), EditError>,
) -> Result<EditOutcome, EditError> {
    let affected = normalized_steps(steps);
    if affected.is_empty() {
        return Ok(EditOutcome::NoOp);
    }
    let track_id = app
        .track_registry
        .id_at(track)
        .ok_or(EditError::TrackOutOfRange { track })?;
    let pattern_id = app
        .state
        .effective_track_pattern_id(track)
        .ok_or(EditError::MissingTrackPattern)?;
    let target = TrackPatternId {
        track: track_id,
        pattern: pattern_id,
    };
    let (before, registry_before) = app
        .state
        .capture_pattern_step_cells(track, pattern_id, &affected)
        .map_err(EditError::ReplayFailed)?;

    if let Err(error) = mutate(app) {
        let rollback = affected
            .iter()
            .copied()
            .zip(before.iter().cloned())
            .collect::<Vec<_>>();
        return match app.state.restore_pattern_step_cells_no_publish(
            track,
            pattern_id,
            &rollback,
            &registry_before,
        ) {
            Ok(_) => Err(error),
            Err(rollback_error) => Err(EditError::ReplayFailed(format!(
                "{error:?}; rollback also failed: {rollback_error}"
            ))),
        };
    }
    let (after, _) = match app
        .state
        .capture_pattern_step_cells(track, pattern_id, &affected)
    {
        Ok(after) => after,
        Err(error) => {
            let rollback = affected
                .iter()
                .copied()
                .zip(before.iter().cloned())
                .collect::<Vec<_>>();
            return match app.state.restore_pattern_step_cells_no_publish(
                track,
                pattern_id,
                &rollback,
                &registry_before,
            ) {
                Ok(_) => Err(EditError::ReplayFailed(error)),
                Err(rollback_error) => Err(EditError::ReplayFailed(format!(
                    "{error}; rollback also failed: {rollback_error}"
                ))),
            };
        }
    };
    let cells = affected
        .iter()
        .copied()
        .zip(before)
        .zip(after)
        .filter_map(|((step, before), after)| {
            (!step_snapshot_bit_exact_eq(&before, &after)).then_some(StepCellDelta {
                step,
                before,
                after,
            })
        })
        .collect::<Vec<_>>();
    if cells.is_empty() {
        return Ok(EditOutcome::NoOp);
    }
    app.state.reconcile_plock_variant_registry_for_track(track);
    let (_, registry_after) = match app
        .state
        .capture_pattern_step_cells(track, pattern_id, &affected)
    {
        Ok(after) => after,
        Err(error) => {
            let rollback = cells
                .iter()
                .map(|cell| (cell.step, cell.before.clone()))
                .collect::<Vec<_>>();
            return match app.state.restore_pattern_step_cells_no_publish(
                track,
                pattern_id,
                &rollback,
                &registry_before,
            ) {
                Ok(_) => Err(EditError::ReplayFailed(error)),
                Err(rollback_error) => Err(EditError::ReplayFailed(format!(
                    "{error}; rollback also failed: {rollback_error}"
                ))),
            };
        }
    };

    let patch = StepCellsPatch {
        target,
        cells,
        variant_registry_before: registry_before,
        variant_registry_after: registry_after,
    };
    if let Err(error) = replay_step_patch(app, &patch, ApplyMode::Redo) {
        return match replay_step_patch(app, &patch, ApplyMode::Undo) {
            Ok(_) => Err(error),
            Err(rollback_error) => Err(EditError::ReplayFailed(format!(
                "{error:?}; rollback also failed: {rollback_error:?}"
            ))),
        };
    }
    let retained_bytes = patch.retained_bytes();
    finish_active_gesture(app);
    let history_move = app.history.commit(
        label,
        None,
        EditPatch::StepCells(patch),
        retained_bytes,
    );
    Ok(EditOutcome::Applied(history_move))
}

pub fn try_apply_command(app: &mut App, cmd: AppCommand) -> Result<EditOutcome, EditError> {
    validate_device_command_target(app, &cmd)?;
    let policy = history_policy(&cmd);
    if matches!(policy, HistoryPolicy::Record) {
        finish_active_gesture(app);
    }
    match policy {
        HistoryPolicy::Record if is_pattern_geometry_command(&cmd) => {
            apply_recorded_pattern_geometry_command(app, &cmd)
        }
        HistoryPolicy::Record if device_plock_command_target(&cmd).is_some() => {
            apply_recorded_device_plock_command(app, &cmd)
        }
        HistoryPolicy::Record if track_params_command_track(&cmd).is_some() => {
            apply_recorded_track_params_command(app, &cmd, None)
        }
        HistoryPolicy::Record if bus_mixer_command_bus(&cmd).is_some() => {
            apply_recorded_bus_mixer_command(app, &cmd, None)
        }
        HistoryPolicy::Record if device_value_command_track(&cmd).is_some() => {
            apply_recorded_device_value_command(app, &cmd, None)
        }
        HistoryPolicy::Record
            if matches!(
                cmd,
                AppCommand::SetMasterVolume { .. }
                    | AppCommand::AdjustMasterVolume { .. }
                    | AppCommand::SetBpm { .. }
            ) =>
        {
            apply_recorded_transport_command(app, &cmd, None)
        }
        HistoryPolicy::Record => apply_recorded_step_command(app, &cmd),
        HistoryPolicy::Ignore => {
            let publish = super::command::command_mutates_sequencer_state(&cmd);
            super::command::execute_command(app, cmd);
            if publish {
                app.state.publish_scheduler_snapshot();
            }
            Ok(EditOutcome::AppliedUnrecorded)
        }
        HistoryPolicy::Barrier => {
            let before = capture_barrier_witness(app, &cmd)?;
            let publish = super::command::command_mutates_sequencer_state(&cmd);
            super::command::execute_command(app, cmd.clone());
            let after = match capture_barrier_witness(app, &cmd) {
                Ok(after) => after,
                Err(error) => {
                    if publish {
                        app.state.publish_scheduler_snapshot();
                    }
                    commit_history_barrier(app);
                    return Err(error);
                }
            };
            if before.bit_exact_eq(&after) {
                return Ok(EditOutcome::NoOp);
            }
            if publish {
                app.state.publish_scheduler_snapshot();
            }
            commit_history_barrier(app);
            Ok(EditOutcome::AppliedUnrecorded)
        }
        HistoryPolicy::Coalesce(key) if track_params_command_track(&cmd).is_some() => {
            apply_recorded_track_params_command(app, &cmd, Some(key))
        }
        HistoryPolicy::Coalesce(key) if bus_mixer_command_bus(&cmd).is_some() => {
            apply_recorded_bus_mixer_command(app, &cmd, Some(key))
        }
        HistoryPolicy::Coalesce(_) if device_plock_command_target(&cmd).is_some() => {
            apply_coalesced_device_plock_command(app, &cmd)
        }
        HistoryPolicy::Coalesce(key) if device_value_command_track(&cmd).is_some() => {
            apply_recorded_device_value_command(app, &cmd, Some(key))
        }
        HistoryPolicy::Coalesce(key)
            if matches!(
                cmd,
                AppCommand::SetMasterVolume { .. }
                    | AppCommand::AdjustMasterVolume { .. }
                    | AppCommand::SetBpm { .. }
            ) =>
        {
            apply_recorded_transport_command(app, &cmd, Some(key))
        }
        HistoryPolicy::Coalesce(_) => Err(EditError::UnsupportedCommand),
        HistoryPolicy::Reset => Err(EditError::UnsupportedCommand),
    }
}

fn replay_step_patch(
    app: &mut App,
    patch: &StepCellsPatch,
    mode: ApplyMode,
) -> Result<MutationEffects, EditError> {
    let track = app
        .track_registry
        .index_of(patch.target.track)
        .ok_or(EditError::MissingStableTrack {
            track: patch.target.track,
        })?;
    let (registry, cells): (&PlockVariantRegistry, Vec<(usize, StepCellSnapshot)>) = match mode {
        ApplyMode::Undo => (
            &patch.variant_registry_before,
            patch
                .cells
                .iter()
                .map(|cell| (cell.step, cell.before.clone()))
                .collect(),
        ),
        ApplyMode::Redo => (
            &patch.variant_registry_after,
            patch
                .cells
                .iter()
                .map(|cell| (cell.step, cell.after.clone()))
                .collect(),
        ),
        ApplyMode::UserEdit | ApplyMode::ProjectLoad => {
            return Err(EditError::ReplayFailed(
                "step patch replay requires undo or redo mode".to_string(),
            ));
        }
    };
    let publish_scheduler = app
        .state
        .restore_pattern_step_cells_no_publish(track, patch.target.pattern, &cells, registry)
        .map_err(EditError::ReplayFailed)?;
    if publish_scheduler {
        app.state.publish_scheduler_snapshot();
    }
    Ok(MutationEffects { publish_scheduler })
}

fn replay_pattern_geometry_patch(
    app: &mut App,
    patch: &PatternGeometryPatch,
    mode: ApplyMode,
) -> Result<MutationEffects, EditError> {
    let track = app
        .track_registry
        .index_of(patch.target.track)
        .ok_or(EditError::MissingStableTrack {
            track: patch.target.track,
        })?;
    let num_steps = match mode {
        ApplyMode::Undo => patch.num_steps_before,
        ApplyMode::Redo => patch.num_steps_after,
        ApplyMode::UserEdit | ApplyMode::ProjectLoad => {
            return Err(EditError::ReplayFailed(
                "pattern geometry replay requires undo or redo mode".to_string(),
            ));
        }
    };
    let step_effects = replay_step_patch(app, &patch.cells, mode)?;
    let geometry_publish = app
        .state
        .restore_pattern_num_steps_no_publish(track, patch.target.pattern, num_steps)
        .map_err(EditError::ReplayFailed)?;
    if geometry_publish && !step_effects.publish_scheduler {
        app.state.publish_scheduler_snapshot();
    }
    Ok(MutationEffects {
        publish_scheduler: step_effects.publish_scheduler || geometry_publish,
    })
}

fn replay_track_params_patch(
    app: &mut App,
    patch: &TrackParamsPatch,
    mode: ApplyMode,
    publish: bool,
) -> Result<MutationEffects, EditError> {
    let track = app
        .track_registry
        .index_of(patch.target.track)
        .ok_or(EditError::MissingStableTrack { track: patch.target.track })?;
    let (snapshot, base_note_bits) = match mode {
        ApplyMode::Undo => (&patch.before, patch.instrument_base_note_offset_before),
        ApplyMode::Redo => (&patch.after, patch.instrument_base_note_offset_after),
        ApplyMode::UserEdit | ApplyMode::ProjectLoad => {
            return Err(EditError::ReplayFailed(
                "track-parameter replay requires undo or redo mode".to_string(),
            ));
        }
    };
    let before = app
        .state
        .capture_pattern_track_params(track, patch.target.pattern)
        .map_err(EditError::ReplayFailed)?;
    let base_note_before = app
        .state
        .capture_pattern_instrument_base_note_offset(track, patch.target.pattern)
        .map_err(EditError::ReplayFailed)?
        .to_bits();
    let is_effective = app
        .state
        .restore_pattern_track_params_no_publish(track, patch.target.pattern, snapshot)
        .map_err(EditError::ReplayFailed)?;
    app.state
        .restore_pattern_instrument_base_note_offset_no_publish(
            track,
            patch.target.pattern,
            f32::from_bits(base_note_bits),
        )
        .map_err(EditError::ReplayFailed)?;
    let publish_scheduler = is_effective
        && (scheduler_track_params_changed(&before, snapshot)
            || base_note_before != base_note_bits);
    if is_effective {
        let needs_publish = apply_live_track_param_effects(
            app,
            track,
            &before,
            snapshot,
            base_note_before,
            base_note_bits,
        );
        if needs_publish && publish {
            app.state.publish_scheduler_snapshot();
        }
    }
    Ok(MutationEffects { publish_scheduler })
}

fn replay_track_params_batch_patch(
    app: &mut App,
    patch: &TrackParamsBatchPatch,
    mode: ApplyMode,
    publish: bool,
) -> Result<MutationEffects, EditError> {
    for track_patch in &patch.tracks {
        let track = app
            .track_registry
            .index_of(track_patch.target.track)
            .ok_or(EditError::MissingStableTrack {
                track: track_patch.target.track,
            })?;
        app.state
            .capture_pattern_track_params(track, track_patch.target.pattern)
            .map_err(EditError::ReplayFailed)?;
        app.state
            .capture_pattern_instrument_base_note_offset(track, track_patch.target.pattern)
            .map_err(EditError::ReplayFailed)?;
    }
    let mut publish_scheduler = false;
    for track_patch in &patch.tracks {
        publish_scheduler |= replay_track_params_patch(app, track_patch, mode, false)?
            .publish_scheduler;
    }
    if publish_scheduler && publish {
        app.state.publish_scheduler_snapshot();
    }
    Ok(MutationEffects { publish_scheduler })
}

fn replay_bus_mixer_patch(
    app: &mut App,
    patch: &BusMixerPatch,
    mode: ApplyMode,
) -> Result<(), EditError> {
    let target = match mode {
        ApplyMode::Undo => patch.before,
        ApplyMode::Redo => patch.after,
        ApplyMode::UserEdit | ApplyMode::ProjectLoad => {
            return Err(EditError::ReplayFailed(
                "bus-mixer replay requires undo or redo mode".to_string(),
            ));
        }
    };
    let channel = app
        .buses
        .iter_mut()
        .find(|channel| channel.id == patch.target)
        .ok_or(EditError::MissingStableBus { bus: patch.target })?;
    let before = BusMixerSnapshot {
        volume_bits: channel.volume.to_bits(),
        mute: channel.mute,
        solo: channel.solo,
    };
    channel.volume = f32::from_bits(target.volume_bits);
    channel.mute = target.mute;
    channel.solo = target.solo;
    if before.volume_bits != target.volume_bits {
        app.push_bus_volume(patch.target);
    }
    if before.mute != target.mute {
        app.push_bus_mute(patch.target);
    }
    if before.solo != target.solo {
        app.push_bus_solo_mutes();
    }
    Ok(())
}

fn replay_bus_effect_values_patch(
    app: &mut App,
    patch: &BusEffectValuesPatch,
    mode: ApplyMode,
) -> Result<(), EditError> {
    let values = match mode {
        ApplyMode::Undo => &patch.before,
        ApplyMode::Redo => &patch.after,
        ApplyMode::UserEdit | ApplyMode::ProjectLoad => {
            return Err(EditError::ReplayFailed(
                "bus-effect value replay requires undo or redo mode".to_string(),
            ));
        }
    };
    let bus_idx = app.buses.iter().position(|bus| bus.id == patch.bus)
        .ok_or(EditError::MissingStableBus { bus: patch.bus })?;
    let (owner, slot) = app.device_registry.bus_audio_effect_location(patch.instance)
        .ok_or_else(|| EditError::ReplayFailed(
            "bus-effect instance no longer exists".to_string(),
        ))?;
    if owner != patch.bus {
        return Err(EditError::ReplayFailed(
            "bus-effect instance moved to another bus".to_string(),
        ));
    }
    let live_all = app.capture_bus_pattern_snapshot();
    let mut repository = app.state.export_bus_pattern_repository(&live_all);
    let scene = repository.get_mut(patch.scene)
        .ok_or_else(|| EditError::ReplayFailed(
            "bus-effect scene no longer exists".to_string(),
        ))?;
    let scene_bus = scene.iter_mut().find(|bus| bus.id == patch.bus)
        .ok_or_else(|| EditError::ReplayFailed(
            "bus-effect scene state no longer exists".to_string(),
        ))?;
    scene_bus.effect_defaults.resize_with(slot + 1, Vec::new);
    scene_bus.effect_plocks.resize_with(slot + 1, Vec::new);
    scene_bus.effect_defaults[slot] = values.defaults.clone();
    scene_bus.effect_plocks[slot] = values.plocks.clone();
    app.state.replace_bus_pattern_repository(repository, &live_all);

    if app.state.current_scene_index() == patch.scene {
        let live_slot = app.buses.get_mut(bus_idx)
            .and_then(|bus| bus.effect_slots.get_mut(slot))
            .ok_or_else(|| EditError::ReplayFailed(
                "bus-effect slot no longer exists".to_string(),
            ))?;
        live_slot.apply_authoring_values(values).map_err(EditError::ReplayFailed)?;
        if let (Some(reference), Some(ir)) = (&values.ir, &values.prepared_ir) {
            app.restore_prepared_bus_effect_ir(bus_idx, slot, reference, ir.clone())
                .map_err(EditError::ReplayFailed)?;
        }
        app.push_bus_effect_slot_defaults(bus_idx, slot);
        app.publish_bus_gate_runtime();
    }
    app.state.publish_scheduler_snapshot();
    Ok(())
}

fn resolve_stored_device_target(
    app: &App,
    patch: &DeviceValuesPatch,
) -> Result<ResolvedDeviceTarget, EditError> {
    let (track_id, slot_idx) = match patch.target {
        DeviceId::TrackInstrument(track) => (track, None),
        DeviceId::AudioEffect(id) => app
            .device_registry
            .audio_effect_location(id)
            .map(|(track, slot)| (track, Some(slot)))
            .ok_or_else(|| {
                EditError::ReplayFailed("audio-effect instance no longer exists".to_string())
            })?,
        DeviceId::MidiEffect(id) => app
            .device_registry
            .midi_effect_location(id)
            .map(|(track, slot)| (track, Some(slot)))
            .ok_or_else(|| {
                EditError::ReplayFailed("MIDI-FX instance no longer exists".to_string())
            })?,
        DeviceId::RackSlot(id) | DeviceId::RackInstrument(id) => app
            .device_registry
            .rack_slot_location(id)
            .map(|(track, slot)| (track, Some(slot)))
            .ok_or_else(|| {
                EditError::ReplayFailed("rack-slot instance no longer exists".to_string())
            })?,
    };
    let track = app
        .track_registry
        .index_of(track_id)
        .ok_or(EditError::MissingStableTrack { track: track_id })?;
    Ok(ResolvedDeviceTarget {
        id: patch.target,
        track,
        pattern: patch.pattern,
        slot_idx,
    })
}

fn push_live_device_values(
    app: &mut App,
    target: ResolvedDeviceTarget,
    snapshot: Option<&DeviceValueSnapshot>,
) -> Result<(), EditError> {
    match target.id {
        DeviceId::TrackInstrument(_) => {
            app.push_instrument_defaults_for_track(target.track);
            if let Some(slot) = app.state.pattern.instrument_slots.get(target.track) {
                let tensors = slot.tensor_params.capture();
                for (tensor_idx, tensor) in tensors.iter().enumerate() {
                    app.send_instrument_tensor_param(target.track, tensor_idx, &tensor.default);
                }
            }
        }
        DeviceId::AudioEffect(_) => {
            let Some(slot_idx) = target.slot_idx else { return Ok(()) };
            let count = app
                .state
                .pattern
                .effect_chains
                .get(target.track)
                .and_then(|chain| chain.get(slot_idx))
                .map(|slot| slot.num_params.load(Ordering::Relaxed) as usize)
                .unwrap_or(0);
            for param_idx in 0..count {
                app.send_effective_slot_param(target.track, slot_idx, param_idx);
            }
            if let Some(DeviceValueSnapshot::Slot(values)) = snapshot {
                for (tensor_idx, tensor) in values.tensor_params.iter().enumerate() {
                    app.send_effect_tensor_param(
                        target.track,
                        slot_idx,
                        tensor_idx,
                        &tensor.default,
                    );
                }
                if let (Some(reference), Some(ir)) = (&values.ir, &values.prepared_ir) {
                    app.restore_prepared_track_effect_ir(
                        target.track,
                        slot_idx,
                        reference,
                        ir.clone(),
                    )
                    .map_err(EditError::ReplayFailed)?;
                }
            }
        }
        DeviceId::MidiEffect(_) => {}
        DeviceId::RackSlot(_) | DeviceId::RackInstrument(_) => {
            let Some(slot_idx) = target.slot_idx else { return Ok(()) };
            let values = app
                .state
                .capture_pattern_rack_slot_values(target.track, target.pattern, slot_idx)
                .ok();
            if let Some(values) = values {
                app.set_rack_slot_gain(
                    target.track,
                    slot_idx,
                    f32::from_bits(values.gain_bits),
                );
                app.set_rack_slot_pan(
                    target.track,
                    slot_idx,
                    f32::from_bits(values.pan_bits),
                );
                app.set_rack_slot_mute(target.track, slot_idx, values.mute);
                app.push_rack_slot_solo_mutes(target.track);
                app.push_rack_slot_instrument_defaults_for_track(target.track);
                for (effect_slot, effect) in values.effect_slots.iter().enumerate() {
                    app.push_rack_slot_effect_defaults(target.track, slot_idx, effect_slot);
                    if let (Some(reference), Some(ir)) = (&effect.ir, &effect.prepared_ir) {
                        app.restore_prepared_rack_effect_ir(
                            target.track,
                            slot_idx,
                            effect_slot,
                            reference,
                            ir.clone(),
                        )
                        .map_err(EditError::ReplayFailed)?;
                    }
                }
            }
        }
    }
    Ok(())
}

fn replay_device_values_patch(
    app: &mut App,
    patch: &DeviceValuesPatch,
    mode: ApplyMode,
    publish: bool,
) -> Result<MutationEffects, EditError> {
    let target = resolve_stored_device_target(app, patch)?;
    let snapshot = match mode {
        ApplyMode::Undo => &patch.before,
        ApplyMode::Redo => &patch.after,
        ApplyMode::UserEdit | ApplyMode::ProjectLoad => {
            return Err(EditError::ReplayFailed(
                "device-value replay requires undo or redo mode".to_string(),
            ));
        }
    };
    let current = capture_device_value_snapshot(app, target)?;
    let is_effective = restore_device_value_snapshot(app, target, snapshot)?;
    if is_effective {
        if let Err(error) = push_live_device_values(app, target, Some(snapshot)) {
            let _ = restore_device_value_snapshot(app, target, &current);
            let _ = push_live_device_values(app, target, Some(&current));
            return Err(error);
        }
        if publish {
            app.state.publish_scheduler_snapshot();
        }
    }
    Ok(MutationEffects {
        publish_scheduler: is_effective,
    })
}

fn replay_transport_params_patch(
    app: &mut App,
    patch: &TransportParamsPatch,
    mode: ApplyMode,
    publish: bool,
) -> Result<MutationEffects, EditError> {
    let target = match mode {
        ApplyMode::Undo => patch.before,
        ApplyMode::Redo => patch.after,
        ApplyMode::UserEdit | ApplyMode::ProjectLoad => {
            return Err(EditError::ReplayFailed(
                "transport-parameter replay requires undo or redo mode".to_string(),
            ));
        }
    };
    let before = capture_transport_authoring(app);
    app.state.transport.bpm.store(target.bpm, Ordering::Relaxed);
    app.state
        .transport
        .master_volume
        .store(target.master_volume_bits, Ordering::Relaxed);
    if before.master_volume_bits != target.master_volume_bits {
        app.push_master_volume();
    }
    let publish_scheduler = before.bpm != target.bpm;
    if publish_scheduler {
        app.push_all_delay_bpm();
        if publish {
            app.state.publish_scheduler_snapshot();
        }
    }
    Ok(MutationEffects { publish_scheduler })
}

fn replay_patch(app: &mut App, patch: &EditPatch, mode: ApplyMode) -> Result<(), EditError> {
    match patch {
        EditPatch::StepCells(patch) => replay_step_patch(app, patch, mode).map(|_| ()),
        EditPatch::PatternGeometry(patch) => {
            replay_pattern_geometry_patch(app, patch, mode).map(|_| ())
        }
        EditPatch::TrackParams(patch) => {
            replay_track_params_patch(app, patch, mode, true).map(|_| ())
        }
        EditPatch::TrackParamsBatch(patch) => {
            replay_track_params_batch_patch(app, patch, mode, true).map(|_| ())
        }
        EditPatch::BusMixer(patch) => replay_bus_mixer_patch(app, patch, mode),
        EditPatch::BusEffectValues(patch) => replay_bus_effect_values_patch(app, patch, mode),
        EditPatch::DeviceValues(patch) => {
            replay_device_values_patch(app, patch, mode, true).map(|_| ())
        }
        EditPatch::InstrumentBinding(patch) => {
            let track = app
                .track_registry
                .index_of(patch.track)
                .ok_or(EditError::MissingStableTrack { track: patch.track })?;
            let target = match mode {
                ApplyMode::Undo => &patch.before,
                ApplyMode::Redo => &patch.after,
                ApplyMode::UserEdit | ApplyMode::ProjectLoad => {
                    return Err(EditError::ReplayFailed(
                        "instrument-binding replay requires undo or redo mode".to_string(),
                    ));
                }
            };
            app.restore_track_instrument_state(track, target)
                .map_err(EditError::ReplayFailed)
        }
        EditPatch::EffectChain(patch) => {
            let track = app
                .track_registry
                .index_of(patch.track)
                .ok_or(EditError::MissingStableTrack { track: patch.track })?;
            let (current, target) = match mode {
                ApplyMode::Undo => (&patch.after, &patch.before),
                ApplyMode::Redo => (&patch.before, &patch.after),
                ApplyMode::UserEdit | ApplyMode::ProjectLoad => {
                    return Err(EditError::ReplayFailed(
                        "effect-chain replay requires undo or redo mode".to_string(),
                    ));
                }
            };
            app.restore_track_effect_chain_state(track, current, target)
                .map_err(EditError::ReplayFailed)
        }
        EditPatch::BusEffectChain(patch) => {
            let bus_idx = app.buses.iter().position(|bus| bus.id == patch.bus)
                .ok_or(EditError::MissingStableBus { bus: patch.bus })?;
            let (current, target) = match mode {
                ApplyMode::Undo => (&patch.after, &patch.before),
                ApplyMode::Redo => (&patch.before, &patch.after),
                ApplyMode::UserEdit | ApplyMode::ProjectLoad => {
                    return Err(EditError::ReplayFailed(
                        "bus-effect-chain replay requires undo or redo mode".to_string(),
                    ));
                }
            };
            app.restore_bus_effect_chain_state(bus_idx, current, target)
                .map_err(EditError::ReplayFailed)
        }
        EditPatch::RackEffectChain(patch) => {
            let track = app.track_registry.index_of(patch.track)
                .ok_or(EditError::MissingStableTrack { track: patch.track })?;
            let (slot_track, rack_slot) = app.device_registry.rack_slot_location(patch.rack_slot)
                .ok_or_else(|| EditError::ReplayFailed(
                    "rack-slot instance no longer exists".to_string(),
                ))?;
            if slot_track != patch.track {
                return Err(EditError::ReplayFailed(
                    "rack-slot instance moved to another track".to_string(),
                ));
            }
            let (current, target) = match mode {
                ApplyMode::Undo => (&patch.after, &patch.before),
                ApplyMode::Redo => (&patch.before, &patch.after),
                ApplyMode::UserEdit | ApplyMode::ProjectLoad => {
                    return Err(EditError::ReplayFailed(
                        "rack-effect-chain replay requires undo or redo mode".to_string(),
                    ));
                }
            };
            app.restore_rack_effect_chain_state(track, rack_slot, current, target)
                .map_err(EditError::ReplayFailed)
        }
        EditPatch::MidiFxChain(patch) => {
            let track = app
                .track_registry
                .index_of(patch.track)
                .ok_or(EditError::MissingStableTrack { track: patch.track })?;
            let target = match mode {
                ApplyMode::Undo => &patch.before,
                ApplyMode::Redo => &patch.after,
                ApplyMode::UserEdit | ApplyMode::ProjectLoad => {
                    return Err(EditError::ReplayFailed(
                        "MIDI-FX chain replay requires undo or redo mode".to_string(),
                    ));
                }
            };
            app.restore_track_midi_fx_chain_state(track, target)
                .map_err(EditError::ReplayFailed)
        }
        EditPatch::RackSlotStructure(patch) => {
            let track = app
                .track_registry
                .index_of(patch.track)
                .ok_or(EditError::MissingStableTrack { track: patch.track })?;
            if let Some((stable_track, stable_slot)) =
                app.device_registry.rack_slot_location(patch.slot)
            {
                if stable_track != patch.track || stable_slot != patch.slot_index {
                    return Err(EditError::ReplayFailed(
                        "rack slot stable identity moved before history replay".to_string(),
                    ));
                }
            }
            match (&patch.edit, mode) {
                (RackSlotStructureEdit::Add { .. }, ApplyMode::Undo) => app
                    .graph_controller()
                    .delete_rack_slot(track, patch.slot_index)
                    .map_err(EditError::ReplayFailed),
                (RackSlotStructureEdit::Add { after }, ApplyMode::Redo) => app
                    .materialize_rack_slot_source(track, patch.slot_index, after, true)
                    .map_err(EditError::ReplayFailed),
                (
                    RackSlotStructureEdit::ReplaceSource { before, .. },
                    ApplyMode::Undo,
                ) => app
                    .materialize_rack_slot_source(track, patch.slot_index, before, false)
                    .map_err(EditError::ReplayFailed),
                (
                    RackSlotStructureEdit::ReplaceSource { after, .. },
                    ApplyMode::Redo,
                ) => app
                    .materialize_rack_slot_source(track, patch.slot_index, after, false)
                    .map_err(EditError::ReplayFailed),
                (_, ApplyMode::UserEdit | ApplyMode::ProjectLoad) => Err(
                    EditError::ReplayFailed(
                        "rack-slot replay requires undo or redo mode".to_string(),
                    ),
                ),
            }
        }
        EditPatch::TrackCreation(patch) => match mode {
            ApplyMode::Undo => {
                let track = app.track_registry.index_of(patch.track)
                    .ok_or(EditError::MissingStableTrack { track: patch.track })?;
                if let Some((group_id, _)) = patch.group {
                    if let Some(group) = app.groups.iter_mut().find(|group| group.id == group_id) {
                        group.members.retain(|member| *member != track);
                    }
                }
                app.device_registry.clear_track(patch.track);
                app.graph_controller().delete_track(track)
                    .map(|_| ())
                    .map_err(EditError::ReplayFailed)
            }
            ApplyMode::Redo => app.restore_created_track(patch)
                .map_err(EditError::ReplayFailed),
            ApplyMode::UserEdit | ApplyMode::ProjectLoad => Err(EditError::ReplayFailed(
                "track-creation replay requires undo or redo mode".to_string(),
            )),
        },
        EditPatch::TrackDeletion(patch) => match mode {
            ApplyMode::Undo => app.restore_deleted_track(patch)
                .map_err(EditError::ReplayFailed),
            ApplyMode::Redo => {
                let track = app.track_registry.index_of(patch.track)
                    .ok_or(EditError::MissingStableTrack { track: patch.track })?;
                app.graph_controller().delete_track(track)
                    .map_err(EditError::ReplayFailed)?;
                app.remap_groups_after_track_delete(track);
                app.macro_engine.remap_after_track_delete(track);
                app.device_registry.clear_track(patch.track);
                Ok(())
            }
            ApplyMode::UserEdit | ApplyMode::ProjectLoad => Err(EditError::ReplayFailed(
                "track-deletion replay requires undo or redo mode".to_string(),
            )),
        },
        EditPatch::TrackPresentation(patch) => app
            .restore_track_presentation(patch, mode)
            .map_err(EditError::ReplayFailed),
        EditPatch::SceneStructure(patch) => {
            let target = match mode {
                ApplyMode::Undo => &patch.before,
                ApplyMode::Redo => &patch.after,
                ApplyMode::UserEdit | ApplyMode::ProjectLoad => {
                    return Err(EditError::ReplayFailed(
                        "scene-structure replay requires undo or redo mode".to_string(),
                    ));
                }
            };
            app.restore_scene_structure_state(target)
                .map_err(EditError::ReplayFailed)
        }
        EditPatch::TransportParams(patch) => {
            replay_transport_params_patch(app, patch, mode, true).map(|_| ())
        }
    }
}

fn pending_gesture_publishes_scheduler(patch: &EditPatch) -> bool {
    match patch {
        EditPatch::TrackParams(patch) => {
            scheduler_track_params_changed(&patch.before, &patch.after)
                || patch.instrument_base_note_offset_before
                    != patch.instrument_base_note_offset_after
        }
        EditPatch::TrackParamsBatch(patch) => patch.tracks.iter().any(|patch| {
            scheduler_track_params_changed(&patch.before, &patch.after)
                || patch.instrument_base_note_offset_before
                    != patch.instrument_base_note_offset_after
        }),
        EditPatch::TransportParams(patch) => patch.before.bpm != patch.after.bpm,
        EditPatch::DeviceValues(_) => true,
        EditPatch::InstrumentBinding(_) => true,
        EditPatch::RackSlotStructure(_) => true,
        EditPatch::TrackCreation(_) => true,
        EditPatch::TrackDeletion(_) => true,
        EditPatch::TrackPresentation(_) => false,
        EditPatch::SceneStructure(_) => true,
        EditPatch::EffectChain(_) => true,
        EditPatch::BusEffectChain(_) => true,
        EditPatch::BusEffectValues(_) => true,
        EditPatch::RackEffectChain(_) => true,
        EditPatch::MidiFxChain(_) => true,
        EditPatch::StepCells(_) | EditPatch::PatternGeometry(_) | EditPatch::BusMixer(_) => false,
    }
}

pub fn finish_active_gesture(app: &mut App) -> bool {
    let publish_scheduler = app
        .history
        .active_gesture()
        .and_then(|gesture| app.history.active_gesture_patch(&gesture.merge_key))
        .is_some_and(pending_gesture_publishes_scheduler);
    let finished = app.history.finish_active_gesture().is_some();
    if finished && publish_scheduler {
        app.state.publish_scheduler_snapshot();
    }
    finished
}

pub fn finish_active_gesture_if_idle(app: &mut App) -> bool {
    if !app
        .history
        .active_gesture_is_idle(super::history::FALLBACK_GESTURE_IDLE_TIMEOUT)
    {
        return false;
    }
    finish_active_gesture(app)
}

pub fn undo(app: &mut App) -> HistoryReplay<EditError> {
    finish_active_gesture(app);
    let topology_request = if app.history.next_undo_patch().is_some_and(structural_track_patch) {
        match wait_for_track_topology_boundary(app) {
            Ok(request) => request,
            Err(error) => return HistoryReplay::Failed(error),
        }
    } else { None };
    let mut history = std::mem::take(&mut app.history);
    let result = history.undo(|patch| replay_patch(app, patch, ApplyMode::Undo));
    app.history = history;
    if let Some(request) = topology_request {
        app.state.complete_topology_edit(request);
        app.state.publish_scheduler_snapshot();
    }
    result
}

pub fn redo(app: &mut App) -> HistoryReplay<EditError> {
    finish_active_gesture(app);
    let topology_request = if app.history.next_redo_patch().is_some_and(structural_track_patch) {
        match wait_for_track_topology_boundary(app) {
            Ok(request) => request,
            Err(error) => return HistoryReplay::Failed(error),
        }
    } else { None };
    let mut history = std::mem::take(&mut app.history);
    let result = history.redo(|patch| replay_patch(app, patch, ApplyMode::Redo));
    app.history = history;
    if let Some(request) = topology_request {
        app.state.complete_topology_edit(request);
        app.state.publish_scheduler_snapshot();
    }
    result
}

fn structural_track_patch(patch: &EditPatch) -> bool {
    matches!(patch, EditPatch::TrackCreation(_) | EditPatch::TrackDeletion(_))
}

fn wait_for_track_topology_boundary(app: &mut App) -> Result<Option<u64>, EditError> {
    if !app.state.is_playing() {
        return Ok(None);
    }
    let track = app.ui.cursor_track.min(app.tracks.len().saturating_sub(1));
    let request = app.state.request_track_delete_boundary(track);
    let deadline = Instant::now() + std::time::Duration::from_millis(250);
    while !app.state.topology_edit_ready(request) && Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    if !app.state.topology_edit_ready(request) {
        app.state.complete_topology_edit(request);
        app.state.publish_scheduler_snapshot();
        return Err(EditError::ReplayFailed(
            "Timed out waiting for a playback boundary".to_string(),
        ));
    }
    Ok(Some(request))
}

pub fn cancel_active_gesture(app: &mut App) -> Result<bool, EditError> {
    let Some(gesture) = app.history.active_gesture().cloned() else {
        return Ok(false);
    };
    let Some(patch) = app.history.active_gesture_patch(&gesture.merge_key).cloned() else {
        finish_active_gesture(app);
        return Ok(false);
    };
    match &patch {
        EditPatch::TrackParams(patch) => {
            replay_track_params_patch(app, patch, ApplyMode::Undo, false)?;
        }
        EditPatch::TrackParamsBatch(patch) => {
            replay_track_params_batch_patch(app, patch, ApplyMode::Undo, false)?;
        }
        EditPatch::TransportParams(patch) => {
            replay_transport_params_patch(app, patch, ApplyMode::Undo, false)?;
        }
        EditPatch::BusMixer(patch) => replay_bus_mixer_patch(app, patch, ApplyMode::Undo)?,
        EditPatch::BusEffectValues(patch) => {
            replay_bus_effect_values_patch(app, patch, ApplyMode::Undo)?;
        }
        EditPatch::DeviceValues(patch) => {
            replay_device_values_patch(app, patch, ApplyMode::Undo, false)?;
        }
        EditPatch::InstrumentBinding(_) => {
            replay_patch(app, &patch, ApplyMode::Undo)?;
        }
        EditPatch::RackSlotStructure(_) => {
            replay_patch(app, &patch, ApplyMode::Undo)?;
        }
        EditPatch::TrackCreation(_) => {
            replay_patch(app, &patch, ApplyMode::Undo)?;
        }
        EditPatch::TrackDeletion(_) => {
            replay_patch(app, &patch, ApplyMode::Undo)?;
        }
        EditPatch::TrackPresentation(_) => {
            replay_patch(app, &patch, ApplyMode::Undo)?;
        }
        EditPatch::SceneStructure(_) => {
            replay_patch(app, &patch, ApplyMode::Undo)?;
        }
        EditPatch::EffectChain(_) => {
            replay_patch(app, &patch, ApplyMode::Undo)?;
        }
        EditPatch::BusEffectChain(_) => {
            replay_patch(app, &patch, ApplyMode::Undo)?;
        }
        EditPatch::RackEffectChain(_) => {
            replay_patch(app, &patch, ApplyMode::Undo)?;
        }
        EditPatch::MidiFxChain(_) => {
            replay_patch(app, &patch, ApplyMode::Undo)?;
        }
        EditPatch::StepCells(_) | EditPatch::PatternGeometry(_) => {
            replay_patch(app, &patch, ApplyMode::Undo)?;
        }
    }
    if !app.history.discard_active_gesture_entry(&gesture.merge_key) {
        return Err(EditError::ReplayFailed(
            "active gesture changed while cancellation was applied".to_string(),
        ));
    }
    Ok(true)
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use super::*;
    use crate::audiograph::LiveGraphPtr;
    use crate::recorder::MasterRecorder;
    use crate::sequencer::{
        default_empty_effect_chain, InstrumentType, PatternSnapshot, SequencerState,
        SwingResolution, Timebase,
    };
    use crate::tui::AudioBuses;

    fn test_app(state: SequencerState) -> App {
        let (keyboard_tx, _keyboard_rx) = std::sync::mpsc::channel();
        let mut app = App::new(
            Arc::new(state),
            LiveGraphPtr(std::ptr::null_mut()),
            44_100,
            AudioBuses {
                bus_l_id: 0,
                bus_r_id: 0,
                default_bus_nodes: Vec::new(),
                bus_gate_runtime: Arc::new(Mutex::new(Vec::new())),
                bus_gate_playheads: Arc::new(Mutex::new(Vec::new())),
                reverb_bus_id: 0,
                reverb_node_id: 0,
            },
            Arc::new(MasterRecorder::new(44_100, 2)),
            keyboard_tx,
        );
        app.tracks = vec!["Track 1".to_string()];
        app.track_registry = crate::sequencer::TrackRegistry::for_legacy_track_count(1).unwrap();
        app
    }

    fn assert_command_round_trip(app: &mut App, cmd: AppCommand, steps: &[usize]) {
        let before = steps
            .iter()
            .map(|step| app.state.capture_step_snapshot(0, *step))
            .collect::<Vec<_>>();
        assert!(matches!(
            apply_recorded_step_command(app, &cmd),
            Ok(EditOutcome::Applied(_))
        ));
        let after = steps
            .iter()
            .map(|step| app.state.capture_step_snapshot(0, *step))
            .collect::<Vec<_>>();
        assert!(matches!(undo(app), HistoryReplay::Applied(_)));
        for (step, expected) in steps.iter().zip(&before) {
            assert!(step_snapshot_bit_exact_eq(
                &app.state.capture_step_snapshot(0, *step),
                expected
            ));
        }
        assert!(matches!(redo(app), HistoryReplay::Applied(_)));
        for (step, expected) in steps.iter().zip(&after) {
            assert!(step_snapshot_bit_exact_eq(
                &app.state.capture_step_snapshot(0, *step),
                expected
            ));
        }
    }

    #[test]
    fn recorded_track_collapse_round_trips_by_stable_track_identity() {
        let mut app = test_app(SequencerState::new(
            1,
            vec![default_empty_effect_chain()],
        ));
        app.normalize_track_colors();
        app.normalize_track_collapsed();

        assert_eq!(app.apply_recorded_track_collapsed(vec![true]), Ok(true));
        assert!(app.track_collapsed[0]);
        assert!(matches!(undo(&mut app), HistoryReplay::Applied(_)));
        assert!(!app.track_collapsed[0]);
        assert!(matches!(redo(&mut app), HistoryReplay::Applied(_)));
        assert!(app.track_collapsed[0]);
    }

    #[test]
    fn recorded_toggle_round_trips_and_no_op_preserves_redo() {
        let mut app = test_app(SequencerState::new(
            1,
            vec![default_empty_effect_chain()],
        ));
        let step = 4;
        app.state.pattern.patterns[0].set_step_active(step, true);
        app.state.pattern.timebase_plocks[0].set(step, Timebase::Eighth);

        let outcome = apply_recorded_step_command(
            &mut app,
            &AppCommand::ToggleStep { track: 0, step },
        )
        .expect("record toggle");
        assert!(matches!(outcome, EditOutcome::Applied(_)));
        assert!(!app.state.pattern.patterns[0].is_active(step));
        assert_eq!(app.history.undo_len(), 1);

        let registry = app.track_registry.clone();
        app.track_registry = crate::sequencer::TrackRegistry::default();
        assert!(matches!(undo(&mut app), HistoryReplay::Failed(_)));
        assert_eq!((app.history.undo_len(), app.history.redo_len()), (1, 0));
        app.track_registry = registry;

        assert!(matches!(undo(&mut app), HistoryReplay::Applied(_)));
        assert!(app.state.pattern.patterns[0].is_active(step));
        assert_eq!(app.state.pattern.timebase_plocks[0].get(step), Some(Timebase::Eighth));
        assert_eq!((app.history.undo_len(), app.history.redo_len()), (0, 1));

        let no_op = apply_recorded_step_command(
            &mut app,
            &AppCommand::SetStepActive {
                track: 0,
                step,
                active: true,
            },
        )
        .expect("same active value is a no-op");
        assert_eq!(no_op, EditOutcome::NoOp);
        assert_eq!((app.history.undo_len(), app.history.redo_len()), (0, 1));

        assert!(matches!(redo(&mut app), HistoryReplay::Applied(_)));
        assert!(!app.state.pattern.patterns[0].is_active(step));
        assert_eq!(app.state.pattern.timebase_plocks[0].get(step), None);
    }

    #[test]
    fn instrument_plock_multi_edit_round_trips_as_one_step_transaction() {
        let state = SequencerState::new(1, vec![default_empty_effect_chain()]);
        let descriptor = crate::effects::EffectDescriptor::builtin_filter();
        state.pattern.instrument_slots[0].apply_descriptor(&descriptor, 1);
        let mut app = test_app(state);
        let steps = [2, 5, 9];
        let before = steps
            .iter()
            .map(|step| app.state.capture_step_snapshot(0, *step))
            .collect::<Vec<_>>();

        try_apply_command(
            &mut app,
            AppCommand::SetInstrumentPlockMulti {
                track: 0,
                steps: steps.to_vec(),
                param_idx: 0,
                value: 0.73,
            },
        )
        .expect("set instrument p-locks");
        let after = steps
            .iter()
            .map(|step| app.state.capture_step_snapshot(0, *step))
            .collect::<Vec<_>>();
        finish_active_gesture(&mut app);
        assert_eq!(app.history.undo_len(), 1);

        assert!(matches!(undo(&mut app), HistoryReplay::Applied(_)));
        for (step, expected) in steps.iter().zip(&before) {
            assert!(step_snapshot_bit_exact_eq(
                &app.state.capture_step_snapshot(0, *step),
                expected,
            ));
        }
        assert!(matches!(redo(&mut app), HistoryReplay::Applied(_)));
        for (step, expected) in steps.iter().zip(&after) {
            assert!(step_snapshot_bit_exact_eq(
                &app.state.capture_step_snapshot(0, *step),
                expected,
            ));
        }
    }

    #[test]
    fn instrument_plock_drag_coalesces_into_one_history_entry() {
        let state = SequencerState::new(1, vec![default_empty_effect_chain()]);
        let descriptor = crate::effects::EffectDescriptor::builtin_filter();
        state.pattern.instrument_slots[0].apply_descriptor(&descriptor, 1);
        let mut app = test_app(state);
        let steps = [2, 5, 9];
        let before = steps
            .iter()
            .map(|step| app.state.capture_step_snapshot(0, *step))
            .collect::<Vec<_>>();

        for value in [0.1, 0.25, 0.5, 0.75, 0.9] {
            assert!(matches!(
                try_apply_command(
                    &mut app,
                    AppCommand::SetInstrumentPlockMulti {
                        track: 0,
                        steps: steps.to_vec(),
                        param_idx: 0,
                        value,
                    },
                ),
                Ok(EditOutcome::Applied(_))
            ));
        }
        assert_eq!(app.history.undo_len(), 0);
        finish_active_gesture(&mut app);
        assert_eq!(app.history.undo_len(), 1);
        let after = steps
            .iter()
            .map(|step| app.state.capture_step_snapshot(0, *step))
            .collect::<Vec<_>>();

        assert!(matches!(undo(&mut app), HistoryReplay::Applied(_)));
        for (step, expected) in steps.iter().zip(&before) {
            assert!(step_snapshot_bit_exact_eq(
                &app.state.capture_step_snapshot(0, *step),
                expected,
            ));
        }
        assert!(matches!(redo(&mut app), HistoryReplay::Applied(_)));
        for (step, expected) in steps.iter().zip(&after) {
            assert!(step_snapshot_bit_exact_eq(
                &app.state.capture_step_snapshot(0, *step),
                expected,
            ));
        }
    }

    #[test]
    fn effect_parameter_batch_drag_coalesces_and_round_trips() {
        let state = SequencerState::new(1, vec![default_empty_effect_chain()]);
        let descriptor = crate::effects::EffectDescriptor::builtin_filter();
        state.pattern.effect_chains[0][0].apply_descriptor(&descriptor, 7);
        assert!(state.save_current_pattern_snapshot(
            1,
            &[-1],
            &[44_100],
            &["Track 1".to_string()],
            &[InstrumentType::Sampler],
        ));
        let mut app = test_app(state);
        app.graph.effect_descriptors = vec![vec![descriptor]];
        let pattern = app.state.effective_track_pattern_id(0).unwrap();
        let before = app
            .state
            .capture_pattern_effect_device_values(0, pattern, 0)
            .unwrap();

        for (cutoff, resonance) in [(300.0, 0.7), (900.0, 1.2), (2_400.0, 2.0)] {
            let commands = [
                AppCommand::SetEffectParam {
                    track: 0,
                    slot_idx: 0,
                    param_idx: 2,
                    value: cutoff,
                },
                AppCommand::SetEffectParam {
                    track: 0,
                    slot_idx: 0,
                    param_idx: 3,
                    value: resonance,
                },
            ];
            let outcome = apply_coalesced_device_value_batch(
                &mut app,
                &commands,
                "effect-curve",
                "Set effect curve",
            );
            assert!(matches!(outcome, Ok(EditOutcome::Applied(_))), "{outcome:?}");
        }
        assert_eq!(app.history.undo_len(), 0);
        finish_active_gesture(&mut app);
        assert_eq!(app.history.undo_len(), 1);
        let after = app
            .state
            .capture_pattern_effect_device_values(0, pattern, 0)
            .unwrap();

        assert!(matches!(undo(&mut app), HistoryReplay::Applied(_)));
        assert!(app
            .state
            .capture_pattern_effect_device_values(0, pattern, 0)
            .unwrap()
            .bit_exact_eq(&before));
        assert!(matches!(redo(&mut app), HistoryReplay::Applied(_)));
        assert!(app
            .state
            .capture_pattern_effect_device_values(0, pattern, 0)
            .unwrap()
            .bit_exact_eq(&after));

        let steps = vec![3, 7];
        let before_locks = steps
            .iter()
            .map(|step| app.state.capture_step_snapshot(0, *step))
            .collect::<Vec<_>>();
        for (cutoff, resonance) in [(600.0, 0.9), (1_800.0, 1.7)] {
            let commands = [
                AppCommand::SetEffectPlockMulti {
                    track: 0,
                    steps: steps.clone(),
                    slot_idx: 0,
                    param_idx: 2,
                    value: cutoff,
                },
                AppCommand::SetEffectPlockMulti {
                    track: 0,
                    steps: steps.clone(),
                    slot_idx: 0,
                    param_idx: 3,
                    value: resonance,
                },
            ];
            let outcome = apply_coalesced_device_plock_batch(
                &mut app,
                &commands,
                "effect-curve",
                "Set effect curve",
            );
            assert!(matches!(outcome, Ok(EditOutcome::Applied(_))), "{outcome:?}");
        }
        finish_active_gesture(&mut app);
        assert_eq!(app.history.undo_len(), 2);
        assert!(matches!(undo(&mut app), HistoryReplay::Applied(_)));
        for (step, expected) in steps.iter().zip(&before_locks) {
            assert!(step_snapshot_bit_exact_eq(
                &app.state.capture_step_snapshot(0, *step),
                expected,
            ));
        }
    }

    #[test]
    fn instrument_default_drag_and_key_locks_round_trip_bit_exactly() {
        let state = SequencerState::new(1, vec![default_empty_effect_chain()]);
        let descriptor = crate::effects::EffectDescriptor::builtin_filter();
        state.pattern.instrument_slots[0].apply_descriptor(&descriptor, 0);
        assert!(state.save_current_pattern_snapshot(
            1,
            &[-1],
            &[44_100],
            &["Track 1".to_string()],
            &[InstrumentType::Sampler],
        ));
        let mut app = test_app(state);
        app.graph.instrument_descriptors = vec![descriptor];
        let pattern = app.state.effective_track_pattern_id(0).unwrap();
        let before = app
            .state
            .capture_pattern_instrument_device_values(0, pattern)
            .unwrap();

        for update in 1..=200 {
            try_apply_command(
                &mut app,
                AppCommand::SetInstrumentParam {
                    track: 0,
                    param_idx: 0,
                    value: update as f32 / 400.0,
                },
            )
            .expect("set instrument default");
        }
        finish_active_gesture(&mut app);
        assert_eq!(app.history.undo_len(), 1);
        try_apply_command(
            &mut app,
            AppCommand::SetInstrumentKeyLockMulti {
                track: 0,
                notes: vec![48, 60, 72],
                param_idx: 0,
                value: 0.37,
            },
        )
        .expect("set instrument key locks");
        let after = app
            .state
            .capture_pattern_instrument_device_values(0, pattern)
            .unwrap();
        assert_eq!(app.history.undo_len(), 2);

        assert!(matches!(undo(&mut app), HistoryReplay::Applied(_)));
        assert!(matches!(undo(&mut app), HistoryReplay::Applied(_)));
        assert!(app
            .state
            .capture_pattern_instrument_device_values(0, pattern)
            .unwrap()
            .bit_exact_eq(&before));
        assert!(matches!(redo(&mut app), HistoryReplay::Applied(_)));
        assert!(matches!(redo(&mut app), HistoryReplay::Applied(_)));
        assert!(app
            .state
            .capture_pattern_instrument_device_values(0, pattern)
            .unwrap()
            .bit_exact_eq(&after));
    }

    #[test]
    fn audio_and_midi_effect_defaults_use_stable_device_history() {
        let state = SequencerState::new(1, vec![default_empty_effect_chain()]);
        let descriptor = crate::effects::EffectDescriptor::builtin_filter();
        state.pattern.effect_chains[0][0].apply_descriptor(&descriptor, 0);
        state.pattern.midi_fx_slots[0][0].apply_descriptor(&descriptor, 0);
        assert!(state.save_current_pattern_snapshot(
            1,
            &[-1],
            &[44_100],
            &["Track 1".to_string()],
            &[InstrumentType::Sampler],
        ));
        let mut app = test_app(state);
        app.graph.effect_descriptors = vec![vec![descriptor]];
        let pattern = app.state.effective_track_pattern_id(0).unwrap();
        let effect_before = app
            .state
            .capture_pattern_effect_device_values(0, pattern, 0)
            .unwrap();
        let midi_before = app
            .state
            .capture_pattern_midi_fx_device_values(0, pattern, 0)
            .unwrap();

        for update in 1..=200 {
            try_apply_command(
                &mut app,
                AppCommand::SetEffectParam {
                    track: 0,
                    slot_idx: 0,
                    param_idx: 0,
                    value: update as f32 / 400.0,
                },
            )
            .expect("set audio effect default");
        }
        finish_active_gesture(&mut app);
        assert_eq!(app.history.undo_len(), 1);
        try_apply_command(
            &mut app,
            AppCommand::SetMidiFxParam {
                track: 0,
                slot_idx: 0,
                param_idx: 0,
                value: 0.8,
            },
        )
        .expect("set MIDI-FX default");
        finish_active_gesture(&mut app);
        let effect_after = app
            .state
            .capture_pattern_effect_device_values(0, pattern, 0)
            .unwrap();
        let midi_after = app
            .state
            .capture_pattern_midi_fx_device_values(0, pattern, 0)
            .unwrap();

        assert!(matches!(undo(&mut app), HistoryReplay::Applied(_)));
        assert!(app
            .state
            .capture_pattern_midi_fx_device_values(0, pattern, 0)
            .unwrap()
            .bit_exact_eq(&midi_before));
        assert!(matches!(undo(&mut app), HistoryReplay::Applied(_)));
        assert!(app
            .state
            .capture_pattern_effect_device_values(0, pattern, 0)
            .unwrap()
            .bit_exact_eq(&effect_before));
        assert!(matches!(redo(&mut app), HistoryReplay::Applied(_)));
        assert!(matches!(redo(&mut app), HistoryReplay::Applied(_)));
        assert!(app
            .state
            .capture_pattern_effect_device_values(0, pattern, 0)
            .unwrap()
            .bit_exact_eq(&effect_after));
        assert!(app
            .state
            .capture_pattern_midi_fx_device_values(0, pattern, 0)
            .unwrap()
            .bit_exact_eq(&midi_after));
    }

    #[test]
    fn rack_strip_drag_and_solo_round_trip_with_derived_state() {
        let state = SequencerState::new(1, vec![default_empty_effect_chain()]);
        let sampler = crate::effects::EffectDescriptor::builtin_sampler();
        state.set_rack_track_for_all_pattern_snapshots(
            0,
            crate::sequencer::RackTrackSnapshot::new(
                crate::sequencer::RackRouting::Broadcast,
                vec![crate::sequencer::RackSlotSnapshot {
                    instrument_type: InstrumentType::Sampler,
                    instrument_run_mode:
                        crate::sequencer::CustomInstrumentRunMode::Instrument,
                    instrument_base_note_offset: 0.0,
                    pad_note: None,
                    choke_group: None,
                    gain: 1.0,
                    pan: 0.0,
                    mute: false,
                    solo: false,
                    max_polyphony: 8,
                    param_plocks: crate::sequencer::RackSlotParamPlocks::new(),
                    instrument_slot:
                        crate::effects::EffectSlotSnapshot::new_default_with_modulator(
                            &sampler,
                            0,
                            0,
                        ),
                    effect_slots: crate::sequencer::RackSlotSnapshot::empty_effect_slots(),
                    effect_descriptors: crate::effects::EffectDescriptor::default_full_chain(),
                    custom_effect_names: crate::sequencer::RackSlotSnapshot::empty_effect_names(),
                    track_sound_state: crate::sequencer::TrackSoundState::default(),
                    sample_id: Some((1, "test.wav".to_string(), 44_100)),
                }],
                crate::sequencer::default_rack_macros(),
            ),
        );
        let mut app = test_app(state);
        let pattern = app.state.effective_track_pattern_id(0).unwrap();
        let before = app
            .state
            .capture_pattern_rack_slot_values(0, pattern, 0)
            .unwrap();

        for update in 1..=200 {
            try_apply_command(
                &mut app,
                AppCommand::SetRackSlotGain {
                    track: 0,
                    slot_idx: 0,
                    value: update as f32 / 400.0,
                },
            )
            .expect("set rack slot gain");
        }
        finish_active_gesture(&mut app);
        try_apply_command(
            &mut app,
            AppCommand::SetRackSlotSolo {
                track: 0,
                slot_idx: 0,
                value: true,
            },
        )
        .expect("solo rack slot");
        let after = app
            .state
            .capture_pattern_rack_slot_values(0, pattern, 0)
            .unwrap();
        assert_eq!(app.history.undo_len(), 2);

        assert!(matches!(undo(&mut app), HistoryReplay::Applied(_)));
        assert!(matches!(undo(&mut app), HistoryReplay::Applied(_)));
        assert!(app
            .state
            .capture_pattern_rack_slot_values(0, pattern, 0)
            .unwrap()
            .bit_exact_eq(&before));
        assert!(matches!(redo(&mut app), HistoryReplay::Applied(_)));
        assert!(matches!(redo(&mut app), HistoryReplay::Applied(_)));
        assert!(app
            .state
            .capture_pattern_rack_slot_values(0, pattern, 0)
            .unwrap()
            .bit_exact_eq(&after));
    }

    #[test]
    fn rack_effect_defaults_and_multi_step_plocks_round_trip() {
        let state = SequencerState::new(1, vec![default_empty_effect_chain()]);
        let sampler = crate::effects::EffectDescriptor::builtin_sampler();
        let effect = tensor_test_descriptor();
        let mut effect_slots = crate::sequencer::RackSlotSnapshot::empty_effect_slots();
        effect_slots[0] = crate::effects::EffectSlotSnapshot::new_default_with_modulator(
            &effect, 0, 0,
        );
        let mut effect_descriptors = crate::effects::EffectDescriptor::default_full_chain();
        effect_descriptors[0] = effect;
        state.set_rack_track_for_all_pattern_snapshots(
            0,
            crate::sequencer::RackTrackSnapshot::new(
                crate::sequencer::RackRouting::Broadcast,
                vec![crate::sequencer::RackSlotSnapshot {
                    instrument_type: InstrumentType::Sampler,
                    instrument_run_mode: crate::sequencer::CustomInstrumentRunMode::Instrument,
                    instrument_base_note_offset: 0.0,
                    pad_note: None,
                    choke_group: None,
                    gain: 1.0,
                    pan: 0.0,
                    mute: false,
                    solo: false,
                    max_polyphony: 8,
                    param_plocks: crate::sequencer::RackSlotParamPlocks::new(),
                    instrument_slot: crate::effects::EffectSlotSnapshot::new_default_with_modulator(
                        &sampler, 0, 0,
                    ),
                    effect_slots,
                    effect_descriptors,
                    custom_effect_names: crate::sequencer::RackSlotSnapshot::empty_effect_names(),
                    track_sound_state: crate::sequencer::TrackSoundState::default(),
                    sample_id: Some((1, "test.wav".to_string(), 44_100)),
                }],
                crate::sequencer::default_rack_macros(),
            ),
        );
        let mut app = test_app(state);
        let pattern = app.state.effective_track_pattern_id(0).unwrap();
        let before = app.state.capture_pattern_rack_slot_values(0, pattern, 0).unwrap();

        try_apply_command(
            &mut app,
            AppCommand::SetRackSlotEffectParam {
                track: 0,
                rack_slot_idx: 0,
                effect_slot_idx: 0,
                param_idx: 0,
                value: 0.41,
            },
        ).unwrap();
        finish_active_gesture(&mut app);
        try_apply_command(
            &mut app,
            AppCommand::SetRackSlotEffectPlockMulti {
                track: 0,
                steps: vec![2, 6],
                rack_slot_idx: 0,
                effect_slot_idx: 0,
                param_idx: 0,
                value: 0.72,
            },
        ).unwrap();
        let after = app.state.capture_pattern_rack_slot_values(0, pattern, 0).unwrap();

        assert!(matches!(undo(&mut app), HistoryReplay::Applied(_)));
        assert!(matches!(undo(&mut app), HistoryReplay::Applied(_)));
        assert!(app.state.capture_pattern_rack_slot_values(0, pattern, 0).unwrap().bit_exact_eq(&before));
        assert!(matches!(redo(&mut app), HistoryReplay::Applied(_)));
        assert!(matches!(redo(&mut app), HistoryReplay::Applied(_)));
        assert!(app.state.capture_pattern_rack_slot_values(0, pattern, 0).unwrap().bit_exact_eq(&after));
    }

    fn tensor_test_descriptor() -> crate::effects::EffectDescriptor {
        let mut descriptor = crate::effects::EffectDescriptor::builtin_filter();
        descriptor.tensor_params = vec![crate::effects::TensorParamDescriptor {
            name: "matrix".to_string(),
            shape: vec![2, 2],
            cell_offset: 64,
            default: vec![0.1, 0.2, 0.3, 0.4],
            min: 0.0,
            max: 1.0,
        }];
        descriptor
    }

    #[test]
    fn scalar_and_tensor_device_locks_set_overwrite_clear_and_round_trip() {
        let state = SequencerState::new(1, vec![default_empty_effect_chain()]);
        let descriptor = tensor_test_descriptor();
        state.pattern.instrument_slots[0].apply_descriptor(&descriptor, 0);
        state.pattern.effect_chains[0][0].apply_descriptor(&descriptor, 0);
        state.pattern.midi_fx_slots[0][0].apply_descriptor(&descriptor, 0);
        assert!(state.save_current_pattern_snapshot(
            1,
            &[-1],
            &[44_100],
            &["Track 1".to_string()],
            &[InstrumentType::Sampler],
        ));
        let mut app = test_app(state);
        app.graph.instrument_descriptors = vec![descriptor.clone()];
        app.graph.effect_descriptors = vec![vec![descriptor]];
        let steps = vec![3, 7];
        let before = steps
            .iter()
            .map(|step| app.state.capture_step_snapshot(0, *step))
            .collect::<Vec<_>>();

        let commands = [
            AppCommand::SetInstrumentTensorPlockCellMulti {
                track: 0,
                steps: steps.clone(),
                tensor_idx: 0,
                cell_idx: 2,
                value: 0.8,
            },
            AppCommand::SetEffectTensorPlockCellMulti {
                track: 0,
                steps: steps.clone(),
                slot_idx: 0,
                tensor_idx: 0,
                cell_idx: 1,
                value: 0.7,
            },
            AppCommand::SetMidiFxTensorPlockCellMulti {
                track: 0,
                steps: steps.clone(),
                slot_idx: 0,
                tensor_idx: 0,
                cell_idx: 0,
                value: 0.6,
            },
        ];
        for command in commands {
            assert!(matches!(
                try_apply_command(&mut app, command),
                Ok(EditOutcome::Applied(_))
            ));
        }
        finish_active_gesture(&mut app);
        assert_eq!(app.history.undo_len(), 3);
        let locked = steps
            .iter()
            .map(|step| app.state.capture_step_snapshot(0, *step))
            .collect::<Vec<_>>();

        try_apply_command(
            &mut app,
            AppCommand::SetInstrumentTensorPlockCellMulti {
                track: 0,
                steps: steps.clone(),
                tensor_idx: 0,
                cell_idx: 2,
                value: 0.9,
            },
        )
        .unwrap();
        try_apply_command(
            &mut app,
            AppCommand::ClearInstrumentTensorPlockMulti {
                track: 0,
                steps: steps.clone(),
                tensor_idx: 0,
            },
        )
        .unwrap();
        try_apply_command(
            &mut app,
            AppCommand::ClearEffectTensorPlockMulti {
                track: 0,
                steps: steps.clone(),
                slot_idx: 0,
                tensor_idx: 0,
            },
        ).unwrap();
        try_apply_command(
            &mut app,
            AppCommand::ClearMidiFxTensorPlockMulti {
                track: 0,
                steps: steps.clone(),
                slot_idx: 0,
                tensor_idx: 0,
            },
        ).unwrap();
        for _ in 0..4 {
            assert!(matches!(undo(&mut app), HistoryReplay::Applied(_)));
        }
        for (step, expected) in steps.iter().zip(&locked) {
            assert!(step_snapshot_bit_exact_eq(
                &app.state.capture_step_snapshot(0, *step),
                expected,
            ));
        }
        for _ in 0..3 {
            assert!(matches!(undo(&mut app), HistoryReplay::Applied(_)));
        }
        for (step, expected) in steps.iter().zip(&before) {
            assert!(step_snapshot_bit_exact_eq(
                &app.state.capture_step_snapshot(0, *step),
                expected,
            ));
        }

        let pattern = app.state.effective_track_pattern_id(0).unwrap();
        let defaults_before = app
            .state
            .capture_pattern_instrument_device_values(0, pattern)
            .unwrap();
        let effect_defaults_before = app
            .state
            .capture_pattern_effect_device_values(0, pattern, 0)
            .unwrap();
        let midi_defaults_before = app
            .state
            .capture_pattern_midi_fx_device_values(0, pattern, 0)
            .unwrap();
        try_apply_command(
            &mut app,
            AppCommand::SetInstrumentTensorCell {
                track: 0,
                tensor_idx: 0,
                cell_idx: 3,
                value: 0.55,
            },
        )
        .unwrap();
        finish_active_gesture(&mut app);
        assert!(matches!(undo(&mut app), HistoryReplay::Applied(_)));
        assert!(app
            .state
            .capture_pattern_instrument_device_values(0, pattern)
            .unwrap()
            .bit_exact_eq(&defaults_before));

        try_apply_command(
            &mut app,
            AppCommand::SetEffectTensorCell {
                track: 0,
                slot_idx: 0,
                tensor_idx: 0,
                cell_idx: 1,
                value: 0.51,
            },
        ).unwrap();
        finish_active_gesture(&mut app);
        try_apply_command(
            &mut app,
            AppCommand::SetMidiFxTensorCell {
                track: 0,
                slot_idx: 0,
                tensor_idx: 0,
                cell_idx: 0,
                value: 0.61,
            },
        ).unwrap();
        finish_active_gesture(&mut app);
        assert!(matches!(undo(&mut app), HistoryReplay::Applied(_)));
        assert!(matches!(undo(&mut app), HistoryReplay::Applied(_)));
        assert!(app.state.capture_pattern_effect_device_values(0, pattern, 0).unwrap().bit_exact_eq(&effect_defaults_before));
        assert!(app.state.capture_pattern_midi_fx_device_values(0, pattern, 0).unwrap().bit_exact_eq(&midi_defaults_before));
    }

    #[test]
    fn derived_modulation_active_defaults_and_plocks_restore_exactly() {
        let state = SequencerState::new(1, vec![default_empty_effect_chain()]);
        let mut descriptor = crate::effects::EffectDescriptor::builtin_filter();
        descriptor.instrument_modulation_targets = vec![
            crate::effects::InstrumentModulationTarget {
                base_param_idx: 0,
                source_param_idx: None,
                modulator_slot: 1,
                depth_param_idx: 0,
                active_param_idx: Some(1),
                depth_min: -1.0,
                depth_max: 1.0,
                depth_unit: None,
            },
        ];
        state.pattern.instrument_slots[0].apply_descriptor(&descriptor, 0);
        state.pattern.effect_chains[0][0].apply_descriptor(&descriptor, 0);
        state.pattern.instrument_slots[0].defaults.set(0, 0.0);
        state.pattern.instrument_slots[0].defaults.set(1, 0.0);
        state.pattern.effect_chains[0][0].defaults.set(0, 0.0);
        state.pattern.effect_chains[0][0].defaults.set(1, 0.0);
        assert!(state.save_current_pattern_snapshot(
            1,
            &[-1],
            &[44_100],
            &["Track 1".to_string()],
            &[InstrumentType::Sampler],
        ));
        let mut app = test_app(state);
        app.graph.instrument_descriptors = vec![descriptor.clone()];
        app.graph.effect_descriptors = vec![vec![descriptor]];

        try_apply_command(
            &mut app,
            AppCommand::SetInstrumentParam {
                track: 0,
                param_idx: 0,
                value: 0.5,
            },
        )
        .unwrap();
        finish_active_gesture(&mut app);
        assert_eq!(app.state.pattern.instrument_slots[0].defaults.get(1), 1.0);
        assert!(matches!(undo(&mut app), HistoryReplay::Applied(_)));
        assert_eq!(app.state.pattern.instrument_slots[0].defaults.get(1).to_bits(), 0.0f32.to_bits());

        try_apply_command(
            &mut app,
            AppCommand::SetEffectPlockMulti {
                track: 0,
                steps: vec![4],
                slot_idx: 0,
                param_idx: 0,
                value: 0.5,
            },
        )
        .unwrap();
        assert_eq!(app.state.pattern.effect_chains[0][0].plocks.get(4, 1), Some(1.0));
        try_apply_command(
            &mut app,
            AppCommand::ClearEffectPlockMulti {
                track: 0,
                steps: vec![4],
                slot_idx: 0,
                param_idx: 0,
            },
        )
        .unwrap();
        assert_eq!(app.state.pattern.effect_chains[0][0].plocks.get(4, 1), None);
        assert!(matches!(undo(&mut app), HistoryReplay::Applied(_)));
        assert_eq!(app.state.pattern.effect_chains[0][0].plocks.get(4, 1), Some(1.0));
    }

    #[test]
    fn device_history_keeps_the_original_pattern_after_scene_switch() {
        let state = SequencerState::new(1, vec![default_empty_effect_chain()]);
        state.replace_pattern_repository(
            vec![
                PatternSnapshot::new_default(1, &[]),
                PatternSnapshot::new_default(1, &[]),
            ],
            0,
        );
        state.restore_current_pattern_from_repository().unwrap();
        let descriptor = crate::effects::EffectDescriptor::builtin_filter();
        state.pattern.instrument_slots[0].apply_descriptor(&descriptor, 0);
        assert!(state.save_current_pattern_snapshot(
            1, &[-1], &[44_100], &["Track 1".to_string()], &[InstrumentType::Sampler],
        ));
        state.launch_scene(
            1, 1, &[-1], &[44_100], &["Track 1".to_string()], &[InstrumentType::Sampler],
        ).unwrap();
        state.pattern.instrument_slots[0].apply_descriptor(&descriptor, 0);
        assert!(state.save_current_pattern_snapshot(
            1, &[-1], &[44_100], &["Track 1".to_string()], &[InstrumentType::Sampler],
        ));
        state.launch_scene(
            0, 1, &[-1], &[44_100], &["Track 1".to_string()], &[InstrumentType::Sampler],
        ).unwrap();
        let mut app = test_app(state);
        app.graph.instrument_descriptors = vec![descriptor];
        let first_pattern = app.state.effective_track_pattern_id(0).unwrap();
        let first_before = app.state.capture_pattern_instrument_device_values(0, first_pattern).unwrap();
        try_apply_command(
            &mut app,
            AppCommand::SetInstrumentParam { track: 0, param_idx: 0, value: 0.23 },
        ).unwrap();
        finish_active_gesture(&mut app);
        app.state.launch_scene(
            1, 1, &[-1], &[44_100], &["Track 1".to_string()], &[InstrumentType::Sampler],
        ).unwrap();
        let second_pattern = app.state.effective_track_pattern_id(0).unwrap();
        let second_before = app.state.capture_pattern_instrument_device_values(0, second_pattern).unwrap();

        assert!(matches!(undo(&mut app), HistoryReplay::Applied(_)));
        assert!(app.state.capture_pattern_instrument_device_values(0, first_pattern).unwrap().bit_exact_eq(&first_before));
        assert!(app.state.capture_pattern_instrument_device_values(0, second_pattern).unwrap().bit_exact_eq(&second_before));
    }

    #[test]
    fn preset_value_transaction_restores_metadata_and_rolls_back_failures() {
        let state = SequencerState::new(1, vec![default_empty_effect_chain()]);
        let descriptor = crate::effects::EffectDescriptor::builtin_filter();
        state.pattern.instrument_slots[0].apply_descriptor(&descriptor, 0);
        state.pattern.track_sound_state.lock().unwrap()[0] = crate::sequencer::TrackSoundState {
            engine_id: Some(7),
            loaded_preset: Some("Old".to_string()),
            dirty: true,
        };
        assert!(state.save_current_pattern_snapshot(
            1, &[-1], &[44_100], &["Track 1".to_string()], &[InstrumentType::Sampler],
        ));
        let mut app = test_app(state);
        app.graph.instrument_descriptors = vec![descriptor];
        let pattern = app.state.effective_track_pattern_id(0).unwrap();
        let before = app.state.capture_pattern_instrument_device_values(0, pattern).unwrap();

        apply_recorded_instrument_values_mutation(&mut app, 0, "Load preset 'Partial'", |app| {
            app.state.pattern.instrument_slots[0].defaults.set(0, 0.42);
            app.state.pattern.track_sound_state.lock().unwrap()[0] = crate::sequencer::TrackSoundState {
                engine_id: Some(7),
                loaded_preset: Some("Partial".to_string()),
                dirty: false,
            };
            Ok(())
        }).unwrap();
        assert!(matches!(undo(&mut app), HistoryReplay::Applied(_)));
        let restored = app.state.capture_pattern_instrument_device_values(0, pattern).unwrap();
        assert!(restored.bit_exact_eq(&before), "restored={restored:#?}\nbefore={before:#?}");

        let history_before = (app.history.undo_len(), app.history.redo_len());
        let failed = apply_recorded_instrument_values_mutation(&mut app, 0, "Invalid preset", |app| {
            app.state.pattern.instrument_slots[0].defaults.set(0, 0.99);
            Err("schema mismatch".to_string())
        });
        assert!(matches!(failed, Err(EditError::ReplayFailed(_))));
        assert_eq!((app.history.undo_len(), app.history.redo_len()), history_before);
        assert!(app.state.capture_pattern_instrument_device_values(0, pattern).unwrap().bit_exact_eq(&before));
    }

    #[test]
    fn key_lock_variant_stamp_and_multi_note_clear_round_trip() {
        let state = SequencerState::new(1, vec![default_empty_effect_chain()]);
        let descriptor = crate::effects::EffectDescriptor::builtin_filter();
        state.pattern.instrument_slots[0].apply_descriptor(&descriptor, 0);
        state.pattern.instrument_slots[0].set_key_lock(60, 0, 0.31);
        let assignment = state.reconcile_key_lock_variant_registry_for_track(0)[60]
            .clone()
            .expect("source key-lock variant");
        assert!(state.save_current_pattern_snapshot(
            1, &[-1], &[44_100], &["Track 1".to_string()], &[InstrumentType::Sampler],
        ));
        let mut app = test_app(state);
        app.graph.instrument_descriptors = vec![descriptor];
        let pattern = app.state.effective_track_pattern_id(0).unwrap();
        let before = app.state.capture_pattern_instrument_device_values(0, pattern).unwrap();

        try_apply_command(
            &mut app,
            AppCommand::StampInstrumentKeyLockVariant {
                track: 0,
                notes: vec![61, 64],
                key: assignment.key,
            },
        ).unwrap();
        let stamped = app.state.capture_pattern_instrument_device_values(0, pattern).unwrap();
        assert!(!stamped.bit_exact_eq(&before));
        try_apply_command(
            &mut app,
            AppCommand::ClearInstrumentKeyLockVariantsForNotes {
                track: 0,
                notes: vec![61, 64],
            },
        ).unwrap();

        assert!(matches!(undo(&mut app), HistoryReplay::Applied(_)));
        assert!(app.state.capture_pattern_instrument_device_values(0, pattern).unwrap().bit_exact_eq(&stamped));
        assert!(matches!(undo(&mut app), HistoryReplay::Applied(_)));
        assert!(app.state.capture_pattern_instrument_device_values(0, pattern).unwrap().bit_exact_eq(&before));
        assert!(matches!(redo(&mut app), HistoryReplay::Applied(_)));
        assert!(matches!(redo(&mut app), HistoryReplay::Applied(_)));
    }

    #[test]
    fn undo_after_scene_switch_targets_original_track_pattern() {
        let state = SequencerState::new(1, vec![default_empty_effect_chain()]);
        let first = PatternSnapshot::new_default(1, &[]);
        let mut second = PatternSnapshot::new_default(1, &[]);
        second.track_bits[0][0] |= 1 << 9;
        state.replace_pattern_repository(vec![first, second], 0);
        state.restore_current_pattern_from_repository().unwrap();
        let mut app = test_app(state);

        apply_recorded_step_command(
            &mut app,
            &AppCommand::SetStepActive {
                track: 0,
                step: 3,
                active: true,
            },
        )
        .expect("record scene-zero edit");
        app.state
            .launch_scene(
                1,
                1,
                &[-1],
                &[44_100],
                &["Track 1".to_string()],
                &[InstrumentType::Sampler],
            )
            .expect("launch scene one");
        assert!(app.state.pattern.patterns[0].is_active(9));
        assert!(!app.state.pattern.patterns[0].is_active(3));

        assert!(matches!(undo(&mut app), HistoryReplay::Applied(_)));
        assert!(app.state.pattern.patterns[0].is_active(9));
        app.state
            .launch_scene(
                0,
                1,
                &[-1],
                &[44_100],
                &["Track 1".to_string()],
                &[InstrumentType::Sampler],
            )
            .expect("return to scene zero");
        assert!(!app.state.pattern.patterns[0].is_active(3));
    }

    #[test]
    fn recorded_step_command_families_obey_the_round_trip_law() {
        let mut app = test_app(SequencerState::new(
            1,
            vec![default_empty_effect_chain()],
        ));
        for (step, velocity) in [(1, 0.2), (3, 0.4), (5, 0.6)] {
            app.state.pattern.patterns[0].set_step_active(step, true);
            app.state
                .pattern
                .step_data[0]
                .set(step, crate::sequencer::StepParam::Velocity, velocity);
        }
        app.state.pattern.chord_data[0].add_note_with_timing(1, 4.0, 0.5, 0.1);

        assert_command_round_trip(
            &mut app,
            AppCommand::SetStepParam {
                track: 0,
                step: 1,
                param: crate::sequencer::StepParam::Transpose,
                value: 7.0,
            },
            &[1],
        );
        assert_command_round_trip(
            &mut app,
            AppCommand::ClearSteps {
                track: 0,
                steps: vec![3, 3, MAX_STEPS + 10],
            },
            &[3],
        );
        assert_command_round_trip(
            &mut app,
            AppCommand::RotateSteps {
                track: 0,
                steps: vec![5, 1, 3, 3],
                direction: 1,
            },
            &[1, 3, 5],
        );
        assert_command_round_trip(
            &mut app,
            AppCommand::ShiftStepRange {
                track: 0,
                lo: 1,
                hi: 3,
                new_lo: 2,
            },
            &[1, 2, 3, 4],
        );
        assert_command_round_trip(
            &mut app,
            AppCommand::ShiftStepRange {
                track: 0,
                lo: 2,
                hi: 4,
                new_lo: 1,
            },
            &[1, 2, 3, 4],
        );

        let pasted = app.state.capture_step_snapshot(0, 1);
        assert_command_round_trip(
            &mut app,
            AppCommand::PasteSteps {
                track: 0,
                source_track: 0,
                clipboard: vec![(0, pasted)],
                dest_start: 6,
                num_steps: 16,
            },
            &[6],
        );
    }

    #[test]
    fn skipped_inactive_paste_is_a_no_op_and_keeps_redo() {
        let mut app = test_app(SequencerState::new(
            1,
            vec![default_empty_effect_chain()],
        ));
        app.state.pattern.patterns[0].set_step_active(2, true);
        apply_recorded_step_command(
            &mut app,
            &AppCommand::SetStepActive {
                track: 0,
                step: 3,
                active: true,
            },
        )
        .expect("record setup edit");
        assert!(matches!(undo(&mut app), HistoryReplay::Applied(_)));
        let empty = app.state.capture_step_snapshot(0, 7);

        let outcome = apply_recorded_step_command(
            &mut app,
            &AppCommand::PasteSteps {
                track: 0,
                source_track: 0,
                clipboard: vec![(0, empty)],
                dest_start: 2,
                num_steps: 16,
            },
        )
        .expect("skip inactive paste over active step");
        assert_eq!(outcome, EditOutcome::NoOp);
        assert!(app.state.pattern.patterns[0].is_active(2));
        assert_eq!((app.history.undo_len(), app.history.redo_len()), (0, 1));
    }

    #[test]
    fn command_boundary_preserves_history_for_no_op_failure_and_performance_actions() {
        let mut app = test_app(SequencerState::new(
            1,
            vec![default_empty_effect_chain()],
        ));
        assert!(matches!(
            try_apply_command(
                &mut app,
                AppCommand::SetStepActive {
                    track: 0,
                    step: 2,
                    active: true,
                },
            ),
            Ok(EditOutcome::Applied(_))
        ));
        assert_eq!(app.history.undo_len(), 1);

        let version = app.state.scheduler_snapshot_version();
        assert_eq!(
            try_apply_command(
                &mut app,
                AppCommand::SetTrackAttack { track: 0, ms: 0.0 },
            ),
            Ok(EditOutcome::NoOp)
        );
        assert_eq!(app.history.undo_len(), 1);
        assert_eq!(app.state.scheduler_snapshot_version(), version);

        assert!(matches!(
            try_apply_command(
                &mut app,
                AppCommand::SetEffectParam {
                    track: 0,
                    slot_idx: usize::MAX,
                    param_idx: 0,
                    value: 0.5,
                },
            ),
            Err(EditError::InvalidTarget(_))
        ));
        assert_eq!(app.history.undo_len(), 1);

        let version = app.state.scheduler_snapshot_version();
        assert_eq!(
            try_apply_command(&mut app, AppCommand::TogglePlay),
            Ok(EditOutcome::AppliedUnrecorded)
        );
        assert_eq!(app.history.undo_len(), 1);
        assert_eq!(app.state.scheduler_snapshot_version(), version + 1);

        let version = app.state.scheduler_snapshot_version();
        assert!(matches!(
            try_apply_command(
                &mut app,
                AppCommand::SetTrackAttack {
                    track: 0,
                    ms: 12.0,
                },
            ),
            Ok(EditOutcome::Applied(_))
        ));
        finish_active_gesture(&mut app);
        assert_eq!((app.history.undo_len(), app.history.redo_len()), (2, 0));
        assert_eq!(app.state.scheduler_snapshot_version(), version + 1);
        let version = app.state.scheduler_snapshot_version();
        assert!(matches!(undo(&mut app), HistoryReplay::Applied(_)));
        assert_eq!(app.state.scheduler_snapshot_version(), version + 1);
        assert_eq!(
            app.state.pattern.track_params[0].get_attack_ms().to_bits(),
            0.0f32.to_bits()
        );
    }

    #[test]
    fn piano_note_toggle_is_one_lossless_history_entry() {
        let mut app = test_app(SequencerState::new(
            1,
            vec![default_empty_effect_chain()],
        ));
        let step = 5;

        assert!(matches!(
            try_apply_command(
                &mut app,
                AppCommand::TogglePianoNote {
                    track: 0,
                    step,
                    semitone: 4,
                },
            ),
            Ok(EditOutcome::Applied(_))
        ));
        assert!(app.state.pattern.patterns[0].is_active(step));
        assert_eq!(
            app.state.pattern.step_data[0]
                .get(step, crate::sequencer::StepParam::Transpose),
            4.0
        );
        assert_eq!(app.history.undo_len(), 1);

        assert!(matches!(
            try_apply_command(
                &mut app,
                AppCommand::TogglePianoNote {
                    track: 0,
                    step,
                    semitone: 7,
                },
            ),
            Ok(EditOutcome::Applied(_))
        ));
        assert_eq!(app.state.pattern.chord_data[0].count(step), 2);
        assert_eq!(app.history.undo_len(), 2);

        assert!(matches!(
            try_apply_command(
                &mut app,
                AppCommand::TogglePianoNote {
                    track: 0,
                    step,
                    semitone: 4,
                },
            ),
            Ok(EditOutcome::Applied(_))
        ));
        assert_eq!(app.state.pattern.chord_data[0].count(step), 0);
        assert_eq!(
            app.state.pattern.step_data[0]
                .get(step, crate::sequencer::StepParam::Transpose),
            7.0
        );
        assert_eq!(app.history.undo_len(), 3);

        assert!(matches!(undo(&mut app), HistoryReplay::Applied(_)));
        assert_eq!(app.state.pattern.chord_data[0].count(step), 2);
        assert_eq!(app.state.pattern.chord_data[0].get(step, 0), 4.0);
        assert_eq!(app.state.pattern.chord_data[0].get(step, 1), 7.0);
        assert!(matches!(redo(&mut app), HistoryReplay::Applied(_)));
        assert_eq!(app.state.pattern.chord_data[0].count(step), 0);
    }

    #[test]
    fn pattern_geometry_commands_round_trip_length_and_duplicated_cells() {
        let mut app = test_app(SequencerState::new(
            1,
            vec![default_empty_effect_chain()],
        ));
        app.state.pattern.patterns[0].set_step_active(3, true);
        app.state.pattern.step_data[0].set(
            3,
            crate::sequencer::StepParam::Velocity,
            0.37,
        );
        app.state.pattern.timebase_plocks[0].set(3, Timebase::Eighth);

        let duplicate = try_apply_command(
            &mut app,
            AppCommand::DuplicateTrackPattern { track: 0 },
        )
        .expect("duplicate pattern through history");
        assert!(matches!(duplicate, EditOutcome::Applied(_)));
        assert_eq!(app.state.pattern.track_params[0].get_num_steps(), 32);
        assert!(app.state.pattern.patterns[0].is_active(19));
        assert_eq!(
            app.state.pattern.step_data[0].get(19, crate::sequencer::StepParam::Velocity),
            0.37,
        );
        assert_eq!(app.state.pattern.timebase_plocks[0].get(19), Some(Timebase::Eighth));

        assert!(matches!(undo(&mut app), HistoryReplay::Applied(_)));
        assert_eq!(app.state.pattern.track_params[0].get_num_steps(), 16);
        assert!(!app.state.pattern.patterns[0].is_active(19));
        assert!(matches!(redo(&mut app), HistoryReplay::Applied(_)));
        assert_eq!(app.state.pattern.track_params[0].get_num_steps(), 32);
        assert!(app.state.pattern.patterns[0].is_active(19));

        let halve = try_apply_command(&mut app, AppCommand::HalveTrackPattern { track: 0 })
            .expect("halve pattern through history");
        assert!(matches!(halve, EditOutcome::Applied(_)));
        assert_eq!(app.state.pattern.track_params[0].get_num_steps(), 16);
        assert!(matches!(undo(&mut app), HistoryReplay::Applied(_)));
        assert_eq!(app.state.pattern.track_params[0].get_num_steps(), 32);
        assert!(app.state.pattern.patterns[0].is_active(19));
    }

    #[test]
    fn track_level_plock_commands_round_trip_as_step_cell_entries() {
        let mut app = test_app(SequencerState::new(
            1,
            vec![default_empty_effect_chain()],
        ));
        let steps = vec![2, 5, 9];
        for command in [
            AppCommand::SetTimebasePlockMulti {
                track: 0,
                steps: steps.clone(),
                timebase: Timebase::Eighth,
            },
            AppCommand::SetTrackSwingPlockMulti {
                track: 0,
                steps: steps.clone(),
                value: 63.0,
            },
            AppCommand::SetTrackSwingResolutionPlockMulti {
                track: 0,
                steps: steps.clone(),
                resolution: SwingResolution::Eighth,
            },
        ] {
            assert_command_round_trip(&mut app, command, &steps);
        }
    }

    #[test]
    fn pattern_geometry_undo_after_scene_switch_targets_original_pattern() {
        let state = SequencerState::new(1, vec![default_empty_effect_chain()]);
        state.replace_pattern_repository(
            vec![
                PatternSnapshot::new_default(1, &[]),
                PatternSnapshot::new_default(1, &[]),
            ],
            0,
        );
        state.restore_current_pattern_from_repository().unwrap();
        let mut app = test_app(state);

        try_apply_command(
            &mut app,
            AppCommand::DuplicateTrackPattern { track: 0 },
        )
        .expect("duplicate first scene track pattern");
        assert_eq!(app.state.pattern.track_params[0].get_num_steps(), 32);

        app.state
            .launch_scene(
                1,
                1,
                &[-1],
                &[44_100],
                &["Track 1".to_string()],
                &[InstrumentType::Sampler],
            )
            .expect("launch second scene");
        assert_eq!(app.state.pattern.track_params[0].get_num_steps(), 16);
        assert!(matches!(undo(&mut app), HistoryReplay::Applied(_)));
        assert_eq!(app.state.pattern.track_params[0].get_num_steps(), 16);

        app.state
            .launch_scene(
                0,
                1,
                &[-1],
                &[44_100],
                &["Track 1".to_string()],
                &[InstrumentType::Sampler],
            )
            .expect("return to first scene");
        assert_eq!(app.state.pattern.track_params[0].get_num_steps(), 16);
    }

    #[test]
    fn no_op_step_gesture_preserves_redo() {
        let mut app = test_app(SequencerState::new(
            1,
            vec![default_empty_effect_chain()],
        ));
        assert!(matches!(
            try_apply_command(
                &mut app,
                AppCommand::SetStepActive {
                    track: 0,
                    step: 4,
                    active: true,
                },
            ),
            Ok(EditOutcome::Applied(_))
        ));
        assert!(matches!(undo(&mut app), HistoryReplay::Applied(_)));
        assert_eq!((app.history.undo_len(), app.history.redo_len()), (0, 1));

        let gesture = StepGestureTransaction::begin(&app, 0, &[2], "No-op gesture")
            .expect("begin no-op gesture");
        assert_eq!(gesture.commit(&mut app), Ok(EditOutcome::NoOp));
        assert_eq!((app.history.undo_len(), app.history.redo_len()), (0, 1));
    }

    #[test]
    fn track_parameter_drag_coalesces_and_round_trips_bit_exactly() {
        let mut app = test_app(SequencerState::new(
            1,
            vec![default_empty_effect_chain()],
        ));
        let before = app.state.live_track_params_snapshot(0).unwrap();
        let scheduler_version = app.state.scheduler_snapshot_version();

        for update in 1..=200 {
            try_apply_command(
                &mut app,
                AppCommand::SetTrackVolume {
                    track: 0,
                    value: update as f32 / 200.0,
                },
            )
            .expect("apply volume gesture update");
        }
        finish_active_gesture(&mut app);
        assert_eq!(app.history.undo_len(), 1);
        assert_eq!(app.state.scheduler_snapshot_version(), scheduler_version);
        let after = app.state.live_track_params_snapshot(0).unwrap();
        assert_eq!(after.volume.to_bits(), 1.0f32.to_bits());

        assert!(matches!(undo(&mut app), HistoryReplay::Applied(_)));
        assert_eq!(app.state.scheduler_snapshot_version(), scheduler_version);
        assert!(track_params_bit_exact_eq(
            &app.state.live_track_params_snapshot(0).unwrap(),
            &before,
        ));
        assert_eq!(
            try_apply_command(
                &mut app,
                AppCommand::SetTrackVolume {
                    track: 0,
                    value: before.volume,
                },
            ),
            Ok(EditOutcome::NoOp)
        );
        assert_eq!((app.history.undo_len(), app.history.redo_len()), (0, 1));
        assert!(matches!(redo(&mut app), HistoryReplay::Applied(_)));
        assert_eq!(app.state.scheduler_snapshot_version(), scheduler_version);
        assert!(track_params_bit_exact_eq(
            &app.state.live_track_params_snapshot(0).unwrap(),
            &after,
        ));
    }

    #[test]
    fn canceling_track_parameter_gesture_restores_before_state_without_history() {
        let mut app = test_app(SequencerState::new(
            1,
            vec![default_empty_effect_chain()],
        ));
        let before = app.state.live_track_params_snapshot(0).unwrap();
        try_apply_command(
            &mut app,
            AppCommand::SetTrackVolume {
                track: 0,
                value: 0.2,
            },
        )
        .expect("begin volume gesture");
        assert_eq!(app.history.undo_len(), 0);

        assert_eq!(cancel_active_gesture(&mut app), Ok(true));
        assert_eq!((app.history.undo_len(), app.history.redo_len()), (0, 0));
        assert!(track_params_bit_exact_eq(
            &app.state.live_track_params_snapshot(0).unwrap(),
            &before,
        ));
    }

    #[test]
    fn track_parameter_gesture_returning_to_origin_commits_nothing() {
        let mut app = test_app(SequencerState::new(
            1,
            vec![default_empty_effect_chain()],
        ));
        let before = app.state.pattern.track_params[0].get_volume();
        try_apply_command(
            &mut app,
            AppCommand::SetTrackVolume {
                track: 0,
                value: 0.2,
            },
        )
        .unwrap();
        assert_eq!(
            try_apply_command(
                &mut app,
                AppCommand::SetTrackVolume {
                    track: 0,
                    value: before,
                },
            ),
            Ok(EditOutcome::NoOp)
        );
        assert!(app.history.active_gesture().is_none());
        assert_eq!((app.history.undo_len(), app.history.redo_len()), (0, 0));
    }

    #[test]
    fn coupled_track_params_and_base_note_restore_after_scene_switch() {
        let state = SequencerState::new(1, vec![default_empty_effect_chain()]);
        state.replace_pattern_repository(
            vec![
                PatternSnapshot::new_default(1, &[]),
                PatternSnapshot::new_default(1, &[]),
            ],
            0,
        );
        state.restore_current_pattern_from_repository().unwrap();
        let mut app = test_app(state);
        let first_pattern = app.state.effective_track_pattern_id(0).unwrap();
        let first_before = app
            .state
            .capture_pattern_track_params(0, first_pattern)
            .unwrap();

        try_apply_command(
            &mut app,
            AppCommand::SetTrackAccumIdx {
                track: 0,
                idx: 2,
                default_limit: Some(31.0),
                script_name: None,
            },
        )
        .expect("set coupled accumulator fields");
        try_apply_command(
            &mut app,
            AppCommand::SetInstrumentBaseNoteOffset {
                track: 0,
                value: -12.0,
            },
        )
        .expect("set base note offset");
        finish_active_gesture(&mut app);

        app.state
            .launch_scene(
                1,
                1,
                &[-1],
                &[44_100],
                &["Track 1".to_string()],
                &[InstrumentType::Sampler],
            )
            .expect("launch second scene");
        assert!(matches!(undo(&mut app), HistoryReplay::Applied(_)));
        assert!(matches!(undo(&mut app), HistoryReplay::Applied(_)));
        let restored = app
            .state
            .capture_pattern_track_params(0, first_pattern)
            .unwrap();
        assert!(track_params_bit_exact_eq(&restored, &first_before));
        assert_eq!(
            app.state
                .capture_pattern_instrument_base_note_offset(0, first_pattern)
                .unwrap()
                .to_bits(),
            0.0f32.to_bits()
        );
    }

    #[test]
    fn transport_params_round_trip_bpm_and_master_volume() {
        let mut app = test_app(SequencerState::new(
            1,
            vec![default_empty_effect_chain()],
        ));
        let before = capture_transport_authoring(&app);
        let scheduler_version = app.state.scheduler_snapshot_version();
        try_apply_command(&mut app, AppCommand::SetBpm { bpm: 173 }).unwrap();
        assert_eq!(app.state.scheduler_snapshot_version(), scheduler_version);
        finish_active_gesture(&mut app);
        assert_eq!(app.state.scheduler_snapshot_version(), scheduler_version + 1);
        try_apply_command(
            &mut app,
            AppCommand::SetMasterVolume { value: 1.25 },
        )
        .unwrap();
        assert_eq!(app.state.scheduler_snapshot_version(), scheduler_version + 1);
        finish_active_gesture(&mut app);
        let after = capture_transport_authoring(&app);

        assert!(matches!(undo(&mut app), HistoryReplay::Applied(_)));
        assert_eq!(app.state.scheduler_snapshot_version(), scheduler_version + 1);
        assert!(matches!(undo(&mut app), HistoryReplay::Applied(_)));
        assert_eq!(app.state.scheduler_snapshot_version(), scheduler_version + 2);
        assert_eq!(capture_transport_authoring(&app), before);
        assert!(matches!(redo(&mut app), HistoryReplay::Applied(_)));
        assert!(matches!(redo(&mut app), HistoryReplay::Applied(_)));
        assert_eq!(capture_transport_authoring(&app), after);
    }

    #[test]
    fn multi_track_mixer_drag_is_one_coalesced_entry() {
        let mut app = test_app(SequencerState::new(
            2,
            vec![default_empty_effect_chain(), default_empty_effect_chain()],
        ));
        app.tracks = vec!["Track 1".to_string(), "Track 2".to_string()];
        app.track_registry = crate::sequencer::TrackRegistry::for_legacy_track_count(2).unwrap();
        let before = (0..2)
            .map(|track| app.state.live_track_params_snapshot(track).unwrap())
            .collect::<Vec<_>>();

        for update in 1..=200 {
            let value = update as f32 / 200.0;
            apply_recorded_track_params_batch(
                &mut app,
                &[
                    AppCommand::SetTrackVolume { track: 0, value },
                    AppCommand::SetTrackVolume { track: 1, value },
                ],
            )
            .expect("apply multi-track mixer update");
        }
        finish_active_gesture(&mut app);
        assert_eq!(app.history.undo_len(), 1);
        assert!(matches!(undo(&mut app), HistoryReplay::Applied(_)));
        for (track, expected) in before.iter().enumerate() {
            assert!(track_params_bit_exact_eq(
                &app.state.live_track_params_snapshot(track).unwrap(),
                expected,
            ));
        }
    }

    #[test]
    fn track_bus_send_drag_is_one_coalesced_entry() {
        let mut app = test_app(SequencerState::new(
            1,
            vec![default_empty_effect_chain()],
        ));
        let pattern = app.state.effective_track_pattern_id(0).unwrap();
        let before = app
            .state
            .capture_pattern_track_params(0, pattern)
            .unwrap();

        for update in 1..=200 {
            try_apply_command(
                &mut app,
                AppCommand::SetTrackSends {
                    track: 0,
                    sends: vec![crate::sequencer::TrackSendSnapshot {
                        destination: BusId::DEFAULT_A,
                        amount: update as f32 / 200.0,
                    }],
                },
            )
            .expect("apply track bus send update");
        }
        finish_active_gesture(&mut app);
        let after = app
            .state
            .capture_pattern_track_params(0, pattern)
            .unwrap();

        assert_eq!(app.history.undo_len(), 1);
        assert!(matches!(undo(&mut app), HistoryReplay::Applied(_)));
        assert!(track_params_bit_exact_eq(
            &app
                .state
                .capture_pattern_track_params(0, pattern)
                .unwrap(),
            &before,
        ));
        assert!(matches!(redo(&mut app), HistoryReplay::Applied(_)));
        assert!(track_params_bit_exact_eq(
            &app
                .state
                .capture_pattern_track_params(0, pattern)
                .unwrap(),
            &after,
        ));
    }

    #[test]
    fn bus_mixer_volume_mute_and_solo_round_trip() {
        let mut app = test_app(SequencerState::new(
            1,
            vec![default_empty_effect_chain()],
        ));
        let bus = BusId::DEFAULT_A;
        let before = capture_bus_mixer(&app, bus).unwrap();

        for update in 1..=200 {
            try_apply_command(
                &mut app,
                AppCommand::SetBusVolume {
                    bus,
                    value: update as f32 / 400.0,
                },
            )
            .expect("apply bus volume update");
        }
        finish_active_gesture(&mut app);
        assert_eq!(app.history.undo_len(), 1);

        try_apply_command(&mut app, AppCommand::ToggleBusMute { bus })
            .expect("toggle bus mute");
        try_apply_command(&mut app, AppCommand::ToggleBusSolo { bus })
            .expect("toggle bus solo");
        let after = capture_bus_mixer(&app, bus).unwrap();
        assert_eq!(app.history.undo_len(), 3);
        app.buses.swap(1, 2);

        assert!(matches!(undo(&mut app), HistoryReplay::Applied(_)));
        assert!(!capture_bus_mixer(&app, bus).unwrap().solo);
        assert!(matches!(undo(&mut app), HistoryReplay::Applied(_)));
        assert!(!capture_bus_mixer(&app, bus).unwrap().mute);
        assert!(matches!(undo(&mut app), HistoryReplay::Applied(_)));
        assert_eq!(capture_bus_mixer(&app, bus).unwrap(), before);

        assert!(matches!(redo(&mut app), HistoryReplay::Applied(_)));
        assert!(matches!(redo(&mut app), HistoryReplay::Applied(_)));
        assert!(matches!(redo(&mut app), HistoryReplay::Applied(_)));
        assert_eq!(capture_bus_mixer(&app, bus).unwrap(), after);
    }

    #[test]
    fn track_parameter_gesture_splits_when_the_active_scene_changes() {
        let state = SequencerState::new(1, vec![default_empty_effect_chain()]);
        state.replace_pattern_repository(
            vec![
                PatternSnapshot::new_default(1, &[]),
                PatternSnapshot::new_default(1, &[]),
            ],
            0,
        );
        state.restore_current_pattern_from_repository().unwrap();
        let mut app = test_app(state);
        let first_pattern = app.state.effective_track_pattern_id(0).unwrap();
        let first_before = app
            .state
            .capture_pattern_track_params(0, first_pattern)
            .unwrap();

        try_apply_command(
            &mut app,
            AppCommand::SetTrackVolume {
                track: 0,
                value: 0.2,
            },
        )
        .expect("edit first scene");
        app.state
            .launch_scene(
                1,
                1,
                &[-1],
                &[44_100],
                &["Track 1".to_string()],
                &[InstrumentType::Sampler],
            )
            .expect("launch second scene");
        let second_pattern = app.state.effective_track_pattern_id(0).unwrap();
        let second_before = app
            .state
            .capture_pattern_track_params(0, second_pattern)
            .unwrap();
        try_apply_command(
            &mut app,
            AppCommand::SetTrackVolume {
                track: 0,
                value: 0.4,
            },
        )
        .expect("edit second scene");
        finish_active_gesture(&mut app);

        assert_eq!(app.history.undo_len(), 2);
        assert!(matches!(undo(&mut app), HistoryReplay::Applied(_)));
        assert!(track_params_bit_exact_eq(
            &app
                .state
                .capture_pattern_track_params(0, second_pattern)
                .unwrap(),
            &second_before,
        ));
        assert!(!track_params_bit_exact_eq(
            &app
                .state
                .capture_pattern_track_params(0, first_pattern)
                .unwrap(),
            &first_before,
        ));
        assert!(matches!(undo(&mut app), HistoryReplay::Applied(_)));
        assert!(track_params_bit_exact_eq(
            &app
                .state
                .capture_pattern_track_params(0, first_pattern)
                .unwrap(),
            &first_before,
        ));
    }

    #[test]
    fn scheduler_observed_multi_track_gesture_publishes_once_at_end() {
        let mut app = test_app(SequencerState::new(
            2,
            vec![default_empty_effect_chain(), default_empty_effect_chain()],
        ));
        app.tracks = vec!["Track 1".to_string(), "Track 2".to_string()];
        app.track_registry = crate::sequencer::TrackRegistry::for_legacy_track_count(2).unwrap();
        let version = app.state.scheduler_snapshot_version();
        for update in 1..=200 {
            let ms = update as f32;
            apply_recorded_track_params_batch(
                &mut app,
                &[
                    AppCommand::SetTrackAttack { track: 0, ms },
                    AppCommand::SetTrackAttack { track: 1, ms },
                ],
            )
            .expect("apply multi-track attack update");
        }
        assert_eq!(app.state.scheduler_snapshot_version(), version);
        finish_active_gesture(&mut app);
        assert_eq!(app.state.scheduler_snapshot_version(), version + 1);
        assert_eq!(app.history.undo_len(), 1);
    }

    #[test]
    fn variant_stamp_and_clear_are_lossless_step_transactions() {
        let mut app = test_app(SequencerState::new(
            1,
            vec![default_empty_effect_chain()],
        ));
        app.state.pattern.timebase_plocks[0].set(2, Timebase::Eighth);
        app.state.pattern.swing_plocks[0].set(2, 63.0);
        app.state.pattern.swing_resolution_plocks[0]
            .set(2, SwingResolution::Eighth);
        let assignment = app.state.reconcile_plock_variant_registry_for_track(0)[2]
            .clone()
            .expect("source variant");
        let before = app.state.capture_step_snapshot(0, 5);

        apply_recorded_step_mutation(&mut app, 0, &[5], "Stamp step variant", |app| {
            app.state
                .stamp_variant_key_to_steps_no_publish(0, &assignment.key, &[5]);
            Ok(())
        })
        .expect("record variant stamp");
        let stamped = app.state.capture_step_snapshot(0, 5);
        assert!(!step_snapshot_bit_exact_eq(&before, &stamped));
        assert!(matches!(undo(&mut app), HistoryReplay::Applied(_)));
        assert!(step_snapshot_bit_exact_eq(
            &before,
            &app.state.capture_step_snapshot(0, 5),
        ));
        assert!(matches!(redo(&mut app), HistoryReplay::Applied(_)));
        assert!(step_snapshot_bit_exact_eq(
            &stamped,
            &app.state.capture_step_snapshot(0, 5),
        ));

        apply_recorded_step_mutation(&mut app, 0, &[5], "Clear step variant locks", |app| {
            app.state.clear_variant_locks_for_steps_no_publish(0, &[5]);
            Ok(())
        })
        .expect("record variant clear");
        assert!(matches!(undo(&mut app), HistoryReplay::Applied(_)));
        assert!(step_snapshot_bit_exact_eq(
            &stamped,
            &app.state.capture_step_snapshot(0, 5),
        ));
    }

    #[test]
    fn slice3_track_command_families_obey_the_round_trip_law() {
        let mut app = test_app(SequencerState::new(
            1,
            vec![default_empty_effect_chain()],
        ));
        let before = app.state.live_track_params_snapshot(0).unwrap();
        let base_before = app.state.pattern.instrument_base_note_offsets[0]
            .load(Ordering::Relaxed);
        let commands = vec![
            AppCommand::ToggleTrackGate { track: 0 },
            AppCommand::ToggleTrackPolyphonic { track: 0 },
            AppCommand::ToggleTrackMute { track: 0 },
            AppCommand::ToggleTrackSolo { track: 0 },
            AppCommand::SetTrackMaxPolyphony { track: 0, value: 12 },
            AppCommand::SetTrackAttack { track: 0, ms: 17.25 },
            AppCommand::SetTrackRelease { track: 0, ms: 912.5 },
            AppCommand::SetTrackSwing { track: 0, value: 61.5 },
            AppCommand::SetTrackSwingResolution {
                track: 0,
                resolution: SwingResolution::Eighth,
            },
            AppCommand::SetTrackVolume { track: 0, value: 0.31 },
            AppCommand::SetTrackPan { track: 0, value: -0.42 },
            AppCommand::SetTrackSend { track: 0, value: 0.63 },
            AppCommand::SetTrackOutput {
                track: 0,
                output: crate::sequencer::TrackOutput::None,
            },
            AppCommand::SetTrackSends {
                track: 0,
                sends: vec![crate::sequencer::TrackSendSnapshot {
                    destination: crate::sequencer::BusId(44),
                    amount: 0.27,
                }],
            },
            AppCommand::SetTrackTimebase {
                track: 0,
                timebase: Timebase::Quarter,
            },
            AppCommand::SetTrackFtsScale {
                track: 0,
                scale_idx: 3,
            },
            AppCommand::SetTrackAccumIdx {
                track: 0,
                idx: 2,
                default_limit: Some(24.0),
                script_name: None,
            },
            AppCommand::SetTrackAccumLimit {
                track: 0,
                value: 19.5,
            },
            AppCommand::SetTrackAccumMode { track: 0, mode: 2 },
            AppCommand::SetTrackMuteGroup { track: 0, group: 4 },
            AppCommand::SetTrackGlobalTranspose {
                track: 0,
                enabled: false,
            },
            AppCommand::SetInstrumentBaseNoteOffset {
                track: 0,
                value: -7.0,
            },
        ];
        for (index, command) in commands.into_iter().enumerate() {
            let outcome = try_apply_command(&mut app, command);
            assert!(
                matches!(outcome, Ok(EditOutcome::Applied(_))),
                "Slice 3 command {index} returned {outcome:?}",
            );
        }
        finish_active_gesture(&mut app);
        let after = app.state.live_track_params_snapshot(0).unwrap();
        let base_after = app.state.pattern.instrument_base_note_offsets[0]
            .load(Ordering::Relaxed);
        let entry_count = app.history.undo_len();
        assert!(entry_count > 0);

        for _ in 0..entry_count {
            assert!(matches!(undo(&mut app), HistoryReplay::Applied(_)));
        }
        assert!(track_params_bit_exact_eq(
            &app.state.live_track_params_snapshot(0).unwrap(),
            &before,
        ));
        assert_eq!(
            app.state.pattern.instrument_base_note_offsets[0].load(Ordering::Relaxed),
            base_before,
        );
        for _ in 0..entry_count {
            assert!(matches!(redo(&mut app), HistoryReplay::Applied(_)));
        }
        assert!(track_params_bit_exact_eq(
            &app.state.live_track_params_snapshot(0).unwrap(),
            &after,
        ));
        assert_eq!(
            app.state.pattern.instrument_base_note_offsets[0].load(Ordering::Relaxed),
            base_after,
        );
    }
}
