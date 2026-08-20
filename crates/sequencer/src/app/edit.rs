use crate::macro_engine::{Macro, MacroMapping};
use crate::effects::{EffectDescriptor, EffectSlotSnapshot, BUILTIN_SLOT_COUNT};
use crate::plock_variants::PlockVariantRegistry;
use crate::sequencer::{
    BusId, PatternId, StepCellSnapshot, TrackId, TrackParamsSnapshot, TrackPatternId,
    DRUM_RACK_FIRST_PAD_NOTE, DRUM_RACK_LAST_PAD_NOTE, MAX_STEPS,
};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

use super::command::{history_policy, sanitize_pasted_step_snapshot, AppCommand};
use super::history::{
    step_snapshot_bit_exact_eq, ActiveGesture, ApplyMode, BusEffectChainPatch,
    BusEffectChainState, BusEffectValuesPatch, BusMixerPatch, BusMixerSnapshot,
    BusGroupStructurePatch, BusGroupStructureState, BusStructureState,
    DeviceId, DeviceValueSnapshot, DeviceValuesPatch, EditPatch, EffectChainPatch,
    EffectChainState, EffectInstanceState, EffectPatternSlots, GestureId, HistoryMove,
    GroupStructureState, HistoryPolicy, HistoryReplay, InstrumentBindingPatch, MacroConfigurationPatch, MergeKey, MidiFxChainPatch,
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

/// Header tint a freshly created drum rack starts with; members can differ.
const DEFAULT_RACK_COLOR: [f32; 3] = [0.5, 0.5, 0.5];

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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PatternLengthChange {
    Double,
    Halve,
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

fn adopt_replaced_effect_instance_id(
    before: &[EffectInstanceState],
    after: &mut [EffectInstanceState],
) -> bool {
    if before.len() != after.len() {
        return false;
    }
    let missing_before = before
        .iter()
        .filter(|instance| !after.iter().any(|candidate| candidate.id == instance.id))
        .map(|instance| instance.id)
        .collect::<Vec<_>>();
    let new_after = after
        .iter()
        .filter(|instance| !before.iter().any(|candidate| candidate.id == instance.id))
        .map(|instance| instance.id)
        .collect::<Vec<_>>();
    if missing_before.len() != 1 || new_after.len() != 1 {
        return false;
    }
    let Some(instance) = after.iter_mut().find(|instance| instance.id == new_after[0]) else {
        return false;
    };
    instance.id = missing_before[0];
    true
}

trait EffectChainHistoryState: Clone {
    fn instances_mut(&mut self) -> &mut Vec<EffectInstanceState>;
}

impl EffectChainHistoryState for RackEffectChainState {
    fn instances_mut(&mut self) -> &mut Vec<EffectInstanceState> {
        &mut self.instances
    }
}

impl EffectChainHistoryState for BusEffectChainState {
    fn instances_mut(&mut self) -> &mut Vec<EffectInstanceState> {
        &mut self.instances
    }
}

impl EffectChainHistoryState for EffectChainState {
    fn instances_mut(&mut self) -> &mut Vec<EffectInstanceState> {
        &mut self.instances
    }
}

fn rollback_effect_chain_edit<S: EffectChainHistoryState>(
    app: &mut App,
    before: &S,
    retained_ids_by_node: &HashMap<u32, crate::sequencer::EffectInstanceId>,
    capture: impl FnOnce(
        &mut App,
        Option<&HashMap<u32, crate::sequencer::EffectInstanceId>>,
    ) -> Result<S, String>,
    restore: impl FnOnce(&mut App, &S, &S) -> Result<(), String>,
    context: &str,
    error: String,
) -> String {
    let partial = match capture(app, Some(retained_ids_by_node)) {
        Ok(partial) => partial,
        Err(_) => {
            // Restore only needs the current instance list to reuse loaded
            // sources. An empty list deliberately forces it to reconstruct
            // the original chain from the retained before-state; this keeps
            // rollback available when the failed operation made the normal
            // after-state capture itself impossible.
            let mut reconstruct = before.clone();
            reconstruct.instances_mut().clear();
            reconstruct
        }
    };
    match restore(app, &partial, before) {
        Ok(()) => format!("{context} failed ({error}); the original chain was restored"),
        Err(rollback_error) => format!(
            "{context} failed ({error}); restoring the original chain also failed ({rollback_error})"
        ),
    }
}

/// Pad order used when adopting a plain group's existing member tracks: begin
/// on the default C4 page, then wrap from the top of the pad domain to C1.
fn group_conversion_pad_notes() -> impl Iterator<Item = i32> {
    (0..=DRUM_RACK_LAST_PAD_NOTE).chain(DRUM_RACK_FIRST_PAD_NOTE..0)
}

impl App {
    pub(super) fn capture_synchronized_scene_structure_state(
        &mut self,
    ) -> Result<crate::sequencer::ProjectScenes, String> {
        // Save-back seam: nothing captured here may carry an engaged macro
        // override (takes spec 17.10) — it would persist into pool entities.
        self.debug_assert_no_macro_override_leak();
        finish_active_gesture(self);
        self.save_current_bus_pattern();
        if !self.state.save_current_pattern_snapshot(
            self.tracks.len(),
            &self.graph.track_buffer_ids,
            &self.graph.track_sample_rates,
            &self.tracks,
            &self.graph.track_instrument_types,
        ) {
            return Err("Could not synchronize the current scene for history".to_string());
        }
        Ok(self.state.capture_project_scenes())
    }

    pub fn begin_recording_take_history(&mut self) -> Result<(), String> {
        if self.recording_history.is_some() {
            return Ok(());
        }
        let before = self.capture_synchronized_scene_structure_state()?;
        self.recording_history = Some(super::RecordingHistoryTransaction {
            before,
            changed: false,
        });
        Ok(())
    }

    pub fn mark_recording_take_changed(&mut self) {
        if let Some(transaction) = self.recording_history.as_mut() {
            transaction.changed = true;
        }
    }

    pub fn finish_recording_take_history(&mut self) -> Result<Option<HistoryMove>, String> {
        let Some(transaction) = self.recording_history.take() else {
            return Ok(None);
        };
        if !transaction.changed {
            return Ok(None);
        }
        self.commit_applied_scene_structure_mutation_checked(
            transaction.before,
            "Record take",
        ).map(Some)
    }

    /// Transport-scoped boundary for the recording-take undo transaction.
    ///
    /// The transaction is open exactly while recording is armed AND the
    /// transport is running; either edge (stop/pause or disarm) commits it.
    /// Each play pass while armed therefore lands as its own "Record take"
    /// undo entry, so undo after a pause peels back one pass instead of
    /// everything since the record button lit — and edits made while paused
    /// stay outside the take's scene snapshot.
    pub fn sync_recording_history_boundary(
        &mut self,
        recording_armed: bool,
        playing: bool,
        transaction_open: &mut bool,
    ) -> Result<(), String> {
        let should_be_open = recording_armed && playing;
        if should_be_open == *transaction_open {
            return Ok(());
        }
        let result = if should_be_open {
            self.begin_recording_take_history()
        } else {
            self.finish_recording_take_history().map(|_| ())
        };
        *transaction_open = should_be_open && result.is_ok();
        result
    }

    pub fn cancel_recording_take_history(&mut self) -> Result<bool, String> {
        let Some(transaction) = self.recording_history.take() else {
            return Ok(false);
        };
        if transaction.changed {
            self.restore_scene_structure_state(&transaction.before)?;
        }
        Ok(transaction.changed)
    }

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
            ids.push(match retained_ids_by_node {
                Some(retained) => retained
                    .get(&node_id)
                    .copied()
                    .unwrap_or_else(|| self.device_registry.allocate_effect_instance()),
                None => self.device_registry.rack_audio_effect(rack_slot_id, *slot),
            });
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
                return Err(rollback_effect_chain_edit(
                    self,
                    &before,
                    &retained_ids_by_node,
                    |app, retained| app.capture_rack_effect_chain_state(track, rack_slot, retained),
                    |app, current, target| {
                        app.restore_rack_effect_chain_state(track, rack_slot, current, target)
                    },
                    "Rack-slot effect edit",
                    error,
                ));
            }
        };
        let mut after = match self.capture_rack_effect_chain_state(
            track,
            rack_slot,
            Some(&retained_ids_by_node),
        ) {
            Ok(after) => after,
            Err(error) => return Err(rollback_effect_chain_edit(
                self,
                &before,
                &retained_ids_by_node,
                |app, retained| app.capture_rack_effect_chain_state(track, rack_slot, retained),
                |app, current, target| {
                    app.restore_rack_effect_chain_state(track, rack_slot, current, target)
                },
                "Capturing the rack-slot effect edit",
                error,
            )),
        };
        if adopt_replaced_effect_instance_id(&before.instances, &mut after.instances) {
            let ids = after.instances.iter().map(|instance| instance.id).collect::<Vec<_>>();
            if let Err(error) = self.device_registry.bind_rack_audio_effect_chain(rack_slot_id, &ids) {
                return Err(rollback_effect_chain_edit(
                    self,
                    &before,
                    &retained_ids_by_node,
                    |app, retained| app.capture_rack_effect_chain_state(track, rack_slot, retained),
                    |app, current, target| {
                        app.restore_rack_effect_chain_state(track, rack_slot, current, target)
                    },
                    "Rebinding rack-slot effect history identity",
                    error,
                ));
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
                        let manifest = result.manifest.clone();
                        self.apply_compiled_rack_slot_effect_to_slot_sync(
                            track, rack_slot, slot, name, result,
                        )?;
                        self.retain_effect_source(locator, slot, desired.source.clone())?;
                        let node_id = self.rack_slot_effect_snapshot(track, rack_slot)?
                            .effect_slots[slot].node_id as i32;
                        if let Some(ir_slots) = ir_slots {
                            crate::effects::conv_reverb::record_ir_slots(node_id, ir_slots);
                        }
                        crate::effects::filter_table::record_compiled_instance(
                            node_id, &manifest, source,
                        );
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
                    // Retained sources carry bare descriptor names; rack-slot
                    // chains are saved verbatim, so qualify built-ins here or
                    // they reload as missing custom effects (eseq-zck).
                    let name = match &instance.source {
                        RetainedEffectSource::NativeBuiltin { name } => name.clone(),
                        RetainedEffectSource::Compiled { name, .. } => name.clone(),
                    };
                    saved.custom_effect_names[slot] = Some(
                        crate::effects::builtin_effect_project_name(&name).unwrap_or(name),
                    );
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
            crate::effects::dgen_builtin::clear_instance(slot.node_id);
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
            if let (Some(reference), Some(table)) = (&values.table, &values.prepared_table) {
                self.restore_prepared_rack_filter_table(
                    track, rack_slot, slot, reference, table.clone(),
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
            ids.push(match retained_ids_by_node {
                Some(retained) => retained
                    .get(&node_id)
                    .copied()
                    .unwrap_or_else(|| self.device_registry.allocate_effect_instance()),
                None => self.device_registry.bus_audio_effect(bus_id, *slot),
            });
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
                return Err(rollback_effect_chain_edit(
                    self,
                    &before,
                    &retained_ids_by_node,
                    |app, retained| app.capture_bus_effect_chain_state(bus_idx, retained),
                    |app, current, target| app.restore_bus_effect_chain_state(bus_idx, current, target),
                    "Bus effect edit",
                    error,
                ));
            }
        };
        let mut after = match self.capture_bus_effect_chain_state(
            bus_idx,
            Some(&retained_ids_by_node),
        ) {
            Ok(after) => after,
            Err(error) => return Err(rollback_effect_chain_edit(
                self,
                &before,
                &retained_ids_by_node,
                |app, retained| app.capture_bus_effect_chain_state(bus_idx, retained),
                |app, current, target| app.restore_bus_effect_chain_state(bus_idx, current, target),
                "Capturing the bus effect edit",
                error,
            )),
        };
        if adopt_replaced_effect_instance_id(&before.instances, &mut after.instances) {
            let ids = after.instances.iter().map(|instance| instance.id).collect::<Vec<_>>();
            if let Err(error) = self.device_registry.bind_bus_audio_effect_chain(bus_id, &ids) {
                return Err(rollback_effect_chain_edit(
                    self,
                    &before,
                    &retained_ids_by_node,
                    |app, retained| app.capture_bus_effect_chain_state(bus_idx, retained),
                    |app, current, target| app.restore_bus_effect_chain_state(bus_idx, current, target),
                    "Rebinding bus effect history identity",
                    error,
                ));
            }
        }
        let mut old_to_new = vec![None; crate::lisp_host::MAX_CUSTOM_FX];
        for (old_slot, old) in before.instances.iter().enumerate() {
            old_to_new[old_slot] = after.instances.iter()
                .position(|candidate| candidate.id == old.id);
        }
        self.macro_engine.remap_effect_mappings_for_bus(bus_id, &old_to_new);
        self.state.publish_macro_overrides(self.macro_engine.override_snapshot());
        after = match self.capture_bus_effect_chain_state(bus_idx, None) {
            Ok(after) => after,
            Err(error) => return Err(rollback_effect_chain_edit(
                self,
                &before,
                &retained_ids_by_node,
                |app, retained| app.capture_bus_effect_chain_state(bus_idx, retained),
                |app, current, target| app.restore_bus_effect_chain_state(bus_idx, current, target),
                "Capturing remapped bus effect history",
                error,
            )),
        };
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
        let scene = self.state.current_scene_id()
            .ok_or_else(|| "Current scene has no stable identity".to_string())?;
        self.save_current_bus_pattern();
        let current_before = self.buses.get(bus_idx)
            .and_then(|bus| bus.effect_slots.get(slot))
            .map(EffectSlotSnapshot::authoring_values)
            .ok_or_else(|| format!("Bus effect slot {} is out of range", slot + 1))?;
        let merge_key = MergeKey::new(format!(
            "bus-effect:{}:scene:{}:{}",
            instance.0,
            scene.0,
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
                self.publish_bus_effect_runtime();
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
                        let manifest = result.manifest.clone();
                        self.apply_compiled_bus_effect_to_slot_sync(bus_idx, slot, name, result)?;
                        self.retain_effect_source(locator, slot, desired.source.clone())?;
                        let node_id = self.buses[bus_idx].effect_slots[slot].node_id as i32;
                        if let Some(ir_slots) = ir_slots {
                            crate::effects::conv_reverb::record_ir_slots(node_id, ir_slots);
                        }
                        crate::effects::filter_table::record_compiled_instance(
                            node_id, &manifest, source,
                        );
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
            crate::effects::dgen_builtin::clear_instance(slot.node_id);
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
            if let (Some(reference), Some(table)) = (&values.table, &values.prepared_table) {
                self.restore_prepared_bus_filter_table(bus_idx, slot, reference, table.clone())?;
            }
        }
        let ids = target.instances.iter().map(|instance| instance.id).collect::<Vec<_>>();
        self.device_registry.bind_bus_audio_effect_chain(bus_id, &ids)?;
        self.macro_engine.restore_effect_mappings_for_bus(bus_id, &target.macro_mappings)
            .map_err(|error| format!("{error:?}"))?;
        self.state.publish_macro_overrides(self.macro_engine.override_snapshot());
        self.refresh_effect_sidechain_labels();
        self.publish_bus_effect_runtime();
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
            let id = match retained_ids_by_node {
                Some(retained) => retained
                    .get(&node_id)
                    .copied()
                    .unwrap_or_else(|| self.device_registry.allocate_effect_instance()),
                None => self.device_registry.audio_effect(track_id, *slot),
            };
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
                } else if let Some(builtin) = crate::effects::dgen_builtin::find(&name) {
                    RetainedEffectSource::Compiled {
                        name,
                        source: builtin.source.to_string(),
                        asset_base: None,
                        origin: builtin.origin,
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
                return Err(rollback_effect_chain_edit(
                    self,
                    &before,
                    &retained_ids_by_node,
                    |app, retained| app.capture_track_effect_chain_state(track, retained),
                    |app, current, target| app.restore_track_effect_chain_state(track, current, target),
                    "Track effect edit",
                    error,
                ));
            }
        };
        let mut after = match self.capture_track_effect_chain_state(
            track,
            Some(&retained_ids_by_node),
        ) {
            Ok(after) => after,
            Err(error) => return Err(rollback_effect_chain_edit(
                self,
                &before,
                &retained_ids_by_node,
                |app, retained| app.capture_track_effect_chain_state(track, retained),
                |app, current, target| app.restore_track_effect_chain_state(track, current, target),
                "Capturing the track effect edit",
                error,
            )),
        };
        if adopt_replaced_effect_instance_id(&before.instances, &mut after.instances) {
            let rebound = after.instances.iter().map(|instance| instance.id).collect::<Vec<_>>();
            if let Err(error) = self.device_registry.bind_audio_effect_chain(
                track_id,
                BUILTIN_SLOT_COUNT,
                &rebound,
            ) {
                return Err(rollback_effect_chain_edit(
                    self,
                    &before,
                    &retained_ids_by_node,
                    |app, retained| app.capture_track_effect_chain_state(track, retained),
                    |app, current, target| app.restore_track_effect_chain_state(track, current, target),
                    "Rebinding track effect history identity",
                    error,
                ));
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
        if let Err(error) = self.state.remap_track_effect_references(
            track,
            &old_to_new,
            &drop_neural_slots,
            &self.graph.effect_descriptors[track],
        ) {
            return Err(rollback_effect_chain_edit(
                self,
                &before,
                &retained_ids_by_node,
                |app, retained| app.capture_track_effect_chain_state(track, retained),
                |app, current, target| app.restore_track_effect_chain_state(track, current, target),
                "Remapping track effect references",
                error,
            ));
        }
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
        after = match self.capture_track_effect_chain_state(track, None) {
            Ok(after) => after,
            Err(error) => return Err(rollback_effect_chain_edit(
                self,
                &before,
                &retained_ids_by_node,
                |app, retained| app.capture_track_effect_chain_state(track, retained),
                |app, current, target| app.restore_track_effect_chain_state(track, current, target),
                "Capturing remapped track effect history",
                error,
            )),
        };
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
                        let manifest = result.manifest.clone();
                        self.apply_compiled_effect_to_slot_sync(result, name, slot, track)?;
                        self.retain_effect_source(locator, slot, desired.source.clone())?;
                        let node_id = self.state.pattern.effect_chains[track][slot]
                            .node_id
                            .load(Ordering::Relaxed) as i32;
                        if let Some(ir_slots) = ir_slots {
                            crate::effects::conv_reverb::record_ir_slots(node_id, ir_slots);
                        }
                        crate::effects::filter_table::record_compiled_instance(
                            node_id, &manifest, source,
                        );
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
            crate::effects::dgen_builtin::clear_instance(slot.node_id);
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
                    if let (Some(reference), Some(table)) =
                        (&values.table, &values.prepared_table)
                    {
                        self.restore_prepared_track_filter_table(
                            track,
                            BUILTIN_SLOT_COUNT + offset,
                            reference,
                            table.clone(),
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
            display_name_user_authored: self.track_name_user_authored
                .get(track)
                .copied()
                .unwrap_or(false),
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
            rack_pad: self.groups.iter()
                .find(|group| group.members.contains(&track))
                .and_then(|group| {
                    let rack = group.rack.as_ref()?;
                    let member = group.members.iter().position(|m| *m == track)?;
                    let pad = rack.pad_index_for_member(member)?;
                    Some(rack.pads[pad].pad_note)
                }),
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

    fn restore_grouped_track_before_state(
        &mut self,
        track: usize,
        instrument: &TrackInstrumentState,
        effects: &EffectChainState,
    ) -> Result<(), String> {
        self.restore_track_instrument_state(track, instrument)?;
        let current_effects = self.capture_track_effect_chain_state(track, None)?;
        self.restore_track_effect_chain_state(track, &current_effects, effects)
    }

    pub fn group_track_to_instrument_rack_recorded(
        &mut self,
        track: usize,
    ) -> Result<(), String> {
        finish_active_gesture(self);
        let track_id = self
            .track_registry
            .id_at(track)
            .ok_or_else(|| format!("Track {} has no stable identity", track + 1))?;
        let before_instrument = self.capture_track_instrument_state(track)?;
        let before_effects = self.capture_track_effect_chain_state(track, None)?;

        if let Err(error) = self.graph_controller().group_track_to_instrument_rack(track) {
            return match self.restore_grouped_track_before_state(
                track,
                &before_instrument,
                &before_effects,
            ) {
                Ok(()) => Err(error),
                Err(rollback_error) => Err(format!(
                    "Grouping the track failed ({error}); restoring the original track also failed ({rollback_error})"
                )),
            };
        }

        let after_instrument = match self.capture_track_instrument_state(track) {
            Ok(state) => state,
            Err(error) => {
                return match self.restore_grouped_track_before_state(
                    track,
                    &before_instrument,
                    &before_effects,
                ) {
                    Ok(()) => Err(format!(
                        "Grouped track was restored because its history state could not be captured: {error}"
                    )),
                    Err(rollback_error) => Err(format!(
                        "Grouped-track history capture failed ({error}); restoring the original track also failed ({rollback_error})"
                    )),
                };
            }
        };
        let after_effects = match self.capture_track_effect_chain_state(track, None) {
            Ok(state) => state,
            Err(error) => {
                return match self.restore_grouped_track_before_state(
                    track,
                    &before_instrument,
                    &before_effects,
                ) {
                    Ok(()) => Err(format!(
                        "Grouped track was restored because its effect history state could not be captured: {error}"
                    )),
                    Err(rollback_error) => Err(format!(
                        "Grouped-track effect history capture failed ({error}); restoring the original track also failed ({rollback_error})"
                    )),
                };
            }
        };

        // Grouping moves the flat track effect chain into the first rack slot.
        // Replay the flat-chain removal before materializing the rack, and undo
        // in the reverse order, so an effect host never exists in both places.
        let effect_patch = EffectChainPatch {
            track: track_id,
            before: before_effects,
            after: after_effects,
        };
        let instrument_patch = InstrumentBindingPatch {
            track: track_id,
            before: before_instrument,
            after: after_instrument,
        };
        let retained_bytes = std::mem::size_of::<Vec<EditPatch>>()
            + effect_patch.retained_bytes()
            + instrument_patch.retained_bytes();
        self.history.commit(
            "Group track to Instrument Rack",
            None,
            EditPatch::Composite(vec![
                EditPatch::EffectChain(effect_patch),
                EditPatch::InstrumentBinding(instrument_patch),
            ]),
            retained_bytes,
        );
        Ok(())
    }

    pub fn apply_recorded_scene_structure_mutation<T>(
        &mut self,
        label: &'static str,
        mutate: impl FnOnce(&mut App) -> Result<T, String>,
    ) -> Result<T, String> {
        let profile = std::env::var_os("METAL_SEQ_PROFILE_PATTERN_SWITCH").is_some();
        let profile_started = Instant::now();
        let before = self.capture_synchronized_scene_structure_state()?;
        let capture_before_elapsed = profile_started.elapsed();
        let mutate_started = Instant::now();
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
        let mutate_elapsed = mutate_started.elapsed();
        let save_started = Instant::now();
        self.save_current_bus_pattern();
        if !self.state.save_current_pattern_snapshot(
            self.tracks.len(),
            &self.graph.track_buffer_ids,
            &self.graph.track_sample_rates,
            &self.tracks,
            &self.graph.track_instrument_types,
        ) {
            return match self.restore_scene_structure_state(&before) {
                Ok(()) => Err(
                    "Scene edit was rolled back because its after-state could not be synchronized"
                        .to_string(),
                ),
                Err(rollback_error) => Err(format!(
                    "Scene after-state synchronization failed; rollback also failed ({rollback_error})"
                )),
            };
        }
        let save_elapsed = save_started.elapsed();
        let capture_after_started = Instant::now();
        let after = self.state.capture_project_scenes();
        let capture_after_elapsed = capture_after_started.elapsed();
        let commit_started = Instant::now();
        let patch = SceneStructurePatch { before, after };
        let retained_bytes = patch.retained_bytes();
        self.history.commit(label, None, EditPatch::SceneStructure(patch), retained_bytes);
        if profile {
            eprintln!(
                "[scene-structure-profile] label={label} total={:.2}ms capture_before={:.2}ms mutate={:.2}ms save_after={:.2}ms capture_after={:.2}ms commit={:.2}ms",
                profile_started.elapsed().as_secs_f64() * 1000.0,
                capture_before_elapsed.as_secs_f64() * 1000.0,
                mutate_elapsed.as_secs_f64() * 1000.0,
                save_elapsed.as_secs_f64() * 1000.0,
                capture_after_elapsed.as_secs_f64() * 1000.0,
                commit_started.elapsed().as_secs_f64() * 1000.0,
            );
        }
        Ok(result)
    }

    pub fn commit_applied_scene_structure_mutation(
        &mut self,
        before: crate::sequencer::ProjectScenes,
        label: &'static str,
    ) {
        if let Err(error) = self.commit_applied_scene_structure_mutation_checked(before, label) {
            self.editor.status_message = Some((error, Instant::now()));
        }
    }

    pub fn commit_applied_scene_structure_mutation_checked(
        &mut self,
        before: crate::sequencer::ProjectScenes,
        label: &'static str,
    ) -> Result<HistoryMove, String> {
        finish_active_gesture(self);
        self.save_current_bus_pattern();
        if !self.state.save_current_pattern_snapshot(
            self.tracks.len(),
            &self.graph.track_buffer_ids,
            &self.graph.track_sample_rates,
            &self.tracks,
            &self.graph.track_instrument_types,
        ) {
            let message = match self.restore_scene_structure_state(&before) {
                Ok(()) => "Authoring edit was rolled back because its history snapshot could not be synchronized".to_string(),
                Err(error) => format!(
                    "Authoring history synchronization failed and rollback also failed: {error}"
                ),
            };
            return Err(message);
        }
        let after = self.state.capture_project_scenes();
        let patch = SceneStructurePatch { before, after };
        let retained_bytes = patch.retained_bytes();
        Ok(self.history.commit(
            label,
            None,
            EditPatch::SceneStructure(patch),
            retained_bytes,
        ))
    }

    pub fn apply_recorded_track_name(
        &mut self,
        track: usize,
        name: &str,
    ) -> Result<EditOutcome, String> {
        finish_active_gesture(self);
        let name = name.trim();
        if name.is_empty() {
            return Ok(EditOutcome::NoOp);
        }
        let track_id = self.track_registry.id_at(track)
            .ok_or_else(|| format!("Track {} does not exist", track + 1))?;
        self.normalize_track_name_authorship();
        if self.tracks[track] == name && self.track_name_user_authored[track] {
            return Ok(EditOutcome::NoOp);
        }
        self.normalize_track_colors();
        self.normalize_track_collapsed();
        let before = TrackPresentationState {
            name: self.tracks[track].clone(),
            name_user_authored: self.track_name_user_authored[track],
            color: self.track_colors[track],
            collapsed: self.track_collapsed[track],
        };
        let after = TrackPresentationState {
            name: name.to_string(),
            name_user_authored: true,
            color: before.color,
            collapsed: before.collapsed,
        };
        self.tracks[track] = after.name.clone();
        self.track_name_user_authored[track] = true;
        let patch = TrackPresentationPatch {
            changes: vec![TrackPresentationChange { track: track_id, before, after }],
        };
        let retained_bytes = patch.retained_bytes();
        Ok(EditOutcome::Applied(self.history.commit(
            "Rename track",
            None,
            EditPatch::TrackPresentation(patch),
            retained_bytes,
        )))
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
                name: self.tracks[track].clone(),
                name_user_authored: self.track_name_user_authored
                    .get(track)
                    .copied()
                    .unwrap_or(false),
                color: self.track_colors[track],
                collapsed: self.track_collapsed[track],
            };
            if before.collapsed == *target {
                return None;
            }
            let after = TrackPresentationState {
                name: before.name.clone(),
                name_user_authored: before.name_user_authored,
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
        self.normalize_track_name_authorship();
        for (track, target) in resolved {
            self.tracks[track] = target.name.clone();
            self.track_name_user_authored[track] = target.name_user_authored;
            self.track_colors[track] = target.color;
            self.track_collapsed[track] = target.collapsed;
        }
        Ok(())
    }

    pub(super) fn restore_scene_structure_state(
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

    fn capture_bus_group_structure_state(&mut self) -> Result<BusGroupStructureState, String> {
        self.save_current_bus_pattern();
        if !self.state.save_current_pattern_snapshot(
            self.tracks.len(),
            &self.graph.track_buffer_ids,
            &self.graph.track_sample_rates,
            &self.tracks,
            &self.graph.track_instrument_types,
        ) {
            return Err("Could not snapshot track routing before editing bus topology".to_string());
        }
        let mut buses = Vec::with_capacity(self.buses.len());
        for bus_idx in 0..self.buses.len() {
            let bus = &self.buses[bus_idx];
            let (id, name, volume, mute, solo) =
                (bus.id, bus.name.clone(), bus.volume, bus.mute, bus.solo);
            let effects = self.capture_bus_effect_chain_state(bus_idx, None)?;
            buses.push(BusStructureState {
                id,
                name,
                volume,
                mute,
                solo,
                effects,
                output: self.buses[bus_idx].output,
            });
        }
        let groups = self.groups.iter().map(|group| {
            let members = group.members.iter().map(|track| {
                self.track_registry.id_at(*track)
                    .ok_or_else(|| format!("Track group {} has an invalid member", group.id))
            }).collect::<Result<Vec<_>, String>>()?;
            Ok(GroupStructureState {
                id: group.id,
                name: group.name.clone(),
                color: group.color,
                collapsed: group.collapsed,
                members,
                bus_id: BusId(group.bus_id),
                rack: group.rack.clone(),
                rack_members: group.rack_members.clone(),
            })
        }).collect::<Result<Vec<_>, String>>()?;
        Ok(BusGroupStructureState {
            buses,
            groups,
            scenes: self.state.capture_project_scenes(),
        })
    }

    fn restore_bus_group_structure_state(
        &mut self,
        target: &BusGroupStructureState,
    ) -> Result<(), String> {
        let mut ids = std::collections::HashSet::new();
        if target.buses.iter().any(|bus| !ids.insert(bus.id)) {
            return Err("Bus history contains duplicate stable bus ids".to_string());
        }
        if !ids.contains(&BusId::MIX) {
            return Err("Bus history is missing the main mix bus".to_string());
        }
        if target.groups.iter().any(|group| !ids.contains(&group.bus_id)) {
            return Err("Bus history contains an invalid group reference".to_string());
        }
        let groups = target.groups.iter().map(|group| {
            let members = group.members.iter().map(|track| {
                self.track_registry.index_of(*track)
                    .ok_or_else(|| format!("Track {:?} no longer exists", track))
            }).collect::<Result<Vec<_>, String>>()?;
            Ok(crate::project::ProjectTrackGroup {
                id: group.id,
                name: group.name.clone(),
                color: group.color,
                collapsed: group.collapsed,
                members,
                bus_id: group.bus_id.0,
                rack: group.rack.clone(),
                rack_members: group.rack_members.clone(),
            })
        }).collect::<Result<Vec<_>, String>>()?;

        let obsolete = self.buses.iter()
            .map(|bus| bus.id)
            .filter(|id| *id != BusId::MIX && !ids.contains(id))
            .collect::<Vec<_>>();
        for id in obsolete {
            if !self.delete_bus_channel(id) {
                return Err(format!("Could not remove bus {:?} during history replay", id));
            }
        }
        for bus in &target.buses {
            if !self.buses.iter().any(|current| current.id == bus.id) {
                self.add_bus_channel_with_id(bus.id, &bus.name)?;
            }
        }
        for (target_index, target_bus) in target.buses.iter().enumerate() {
            let current_index = self.buses.iter().position(|bus| bus.id == target_bus.id)
                .ok_or_else(|| format!("Bus {:?} disappeared during history replay", target_bus.id))?;
            if current_index != target_index {
                let bus = self.buses.remove(current_index);
                self.buses.insert(target_index, bus);
            }
        }
        for (bus_idx, target_bus) in target.buses.iter().enumerate() {
            let current_effects = self.capture_bus_effect_chain_state(bus_idx, None)?;
            if !current_effects.instances.is_empty() || !target_bus.effects.instances.is_empty() {
                self.restore_bus_effect_chain_state(bus_idx, &current_effects, &target_bus.effects)?;
            }
            let bus = &mut self.buses[bus_idx];
            bus.name.clone_from(&target_bus.name);
            bus.volume = target_bus.volume;
            bus.mute = target_bus.mute;
            bus.solo = target_bus.solo;
            bus.output = target_bus.output;
        }
        self.groups = groups;
        // Chained bus outputs are replayed as part of the topology: the
        // destination bus may only have just been recreated above.
        self.graph_controller().apply_all_bus_output_routing();
        self.restore_scene_structure_state(&target.scenes)?;
        for track in 0..self.tracks.len() {
            let mut graph = self.graph_controller();
            graph.apply_track_output_routing(track);
            graph.apply_track_bus_sends(track);
        }
        self.publish_bus_effect_runtime();
        self.publish_rack_choke_runtime();
        self.state.publish_scheduler_snapshot();
        Ok(())
    }

    pub fn apply_recorded_bus_group_structure_mutation<T>(
        &mut self,
        label: &'static str,
        mutate: impl FnOnce(&mut App) -> Result<T, String>,
    ) -> Result<T, String> {
        finish_active_gesture(self);
        let before = self.capture_bus_group_structure_state()?;
        let result = match mutate(self) {
            Ok(result) => result,
            Err(error) => {
                return match self.restore_bus_group_structure_state(&before) {
                    Ok(()) => Err(error),
                    Err(rollback_error) => Err(format!(
                        "Bus/group edit failed ({error}); restoring its before-state also failed ({rollback_error})"
                    )),
                };
            }
        };
        let after = match self.capture_bus_group_structure_state() {
            Ok(after) => after,
            Err(capture_error) => {
                return match self.restore_bus_group_structure_state(&before) {
                    Ok(()) => Err(format!(
                        "Bus/group edit was rolled back because its after-state could not be captured: {capture_error}"
                    )),
                    Err(rollback_error) => Err(format!(
                        "Bus/group after-state capture failed ({capture_error}); rollback also failed ({rollback_error})"
                    )),
                };
            }
        };
        self.publish_rack_choke_runtime();
        let patch = BusGroupStructurePatch { before, after };
        let retained_bytes = patch.retained_bytes();
        self.history.commit(label, None, EditPatch::BusGroupStructure(patch), retained_bytes);
        Ok(result)
    }

    pub fn add_bus_recorded(&mut self, name: String) -> Result<BusId, String> {
        self.apply_recorded_bus_group_structure_mutation("Add bus", |app| {
            Ok(app.add_bus_channel(name))
        })
    }

    pub fn delete_bus_recorded(&mut self, id: BusId) -> Result<(), String> {
        self.apply_recorded_bus_group_structure_mutation("Delete bus", |app| {
            app.delete_bus_channel(id)
                .then_some(())
                .ok_or_else(|| format!("Bus {:?} cannot be deleted", id))
        })
    }

    pub fn rename_bus_recorded(&mut self, id: BusId, name: String) -> Result<(), String> {
        let name = name.trim().to_string();
        self.apply_recorded_bus_group_structure_mutation("Rename bus", |app| {
            if name.is_empty() {
                return Err("Bus name cannot be empty".to_string());
            }
            let bus = app.buses.iter_mut().find(|bus| bus.id == id)
                .ok_or_else(|| format!("Bus {:?} does not exist", id))?;
            if bus.name == name {
                return Err("Bus name is unchanged".to_string());
            }
            bus.name.clone_from(&name);
            Ok(())
        })
    }

    pub fn reorder_bus_recorded(&mut self, source: usize, target: usize) -> Result<(), String> {
        self.apply_recorded_bus_group_structure_mutation("Reorder bus", |app| {
            if source >= app.buses.len() || target >= app.buses.len() {
                return Err("Bus index is out of range".to_string());
            }
            if app.buses[source].id == BusId::MIX || app.buses[target].id == BusId::MIX {
                return Err("The main mix bus has a fixed position".to_string());
            }
            if source == target {
                return Err("Bus order is unchanged".to_string());
            }
            let bus = app.buses.remove(source);
            app.buses.insert(target, bus);
            Ok(())
        })
    }

    pub fn group_tracks_recorded(&mut self, members: Vec<usize>) -> Result<BusId, String> {
        self.group_tracks_and_racks_recorded(members, Vec::new())
    }

    /// Groups loose tracks and whole racks into one new plain group. A rack
    /// joins as a single unit: it keeps its own group and bus, and that bus
    /// chains into the new parent's bus, so parent volume/fx/mute reach the
    /// rack through the audio chain (docs/drum-rack-v2-spec.md, "Racks inside
    /// track groups"). Two members of any kind are required.
    pub fn group_tracks_and_racks_recorded(
        &mut self,
        mut members: Vec<usize>,
        mut racks: Vec<u64>,
    ) -> Result<BusId, String> {
        members.sort_unstable();
        members.dedup();
        racks.dedup();
        self.apply_recorded_bus_group_structure_mutation("Group tracks", |app| {
            if members.len() + racks.len() < 2
                || members.iter().any(|track| *track >= app.tracks.len())
                || members.iter().any(|track| app.groups.iter().any(|group| group.members.contains(track)))
            {
                return Err("At least two ungrouped tracks are required".to_string());
            }
            for rack in &racks {
                app.check_rack_is_groupable(*rack)?;
            }
            let group_index = app.groups.len() + 1;
            let bus = app.add_bus_channel(format!("Group {group_index}"));
            for track in &members {
                app.set_track_output_all_scenes_unrecorded(*track, crate::sequencer::TrackOutput::Bus(bus));
            }
            // With only racks selected there is no loose member to take a color
            // from, so fall back to the first rack's own group color.
            let color = members.first()
                .and_then(|track| app.track_colors.get(*track))
                .map(|color| [color.r, color.g, color.b])
                .or_else(|| racks.first().and_then(|rack| {
                    app.groups.iter().find(|group| group.id == *rack).map(|group| group.color)
                }))
                .unwrap_or([0.5, 0.5, 0.5]);
            let group_id = app.groups.iter().map(|group| group.id).max().unwrap_or(0) + 1;
            app.groups.push(crate::project::ProjectTrackGroup {
                id: group_id,
                name: format!("Group {group_index}"),
                color,
                collapsed: false,
                members,
                bus_id: bus.0,
                rack: None,
                rack_members: racks.clone(),
            });
            for rack in &racks {
                app.set_rack_bus_output(*rack, Some(bus))?;
            }
            Ok(bus)
        })
    }

    /// Rejects a group id that cannot join a plain group as a rack unit. The
    /// nesting rule (docs/drum-rack-v2-spec.md): only racks nest, and only into
    /// one parent — plain groups never contain plain groups, and racks never
    /// contain racks.
    fn check_rack_is_groupable(&self, rack_id: u64) -> Result<(), String> {
        let rack = self.groups.iter().find(|group| group.id == rack_id)
            .ok_or_else(|| format!("Track group {rack_id} does not exist"))?;
        if !rack.is_rack() {
            return Err("Only drum racks can be grouped as a unit".to_string());
        }
        if self.rack_parent_group(rack_id).is_some() {
            return Err("This rack already belongs to a group".to_string());
        }
        Ok(())
    }

    /// Index (in `groups`) of the plain group holding this rack, if any.
    pub fn rack_parent_group(&self, rack_id: u64) -> Option<usize> {
        self.groups
            .iter()
            .position(|group| group.rack_members.contains(&rack_id))
    }

    /// Points a rack's backing bus at `parent` (or back at the master mix with
    /// `None`) and re-wires the graph to match.
    fn set_rack_bus_output(&mut self, rack_id: u64, parent: Option<BusId>) -> Result<(), String> {
        let rack_bus = self.groups.iter().find(|group| group.id == rack_id)
            .map(|group| BusId(group.bus_id))
            .ok_or_else(|| format!("Track group {rack_id} does not exist"))?;
        let output = match parent {
            Some(parent) if parent != rack_bus => crate::project::BusOutput::Bus(parent.0),
            _ => crate::project::BusOutput::Mix,
        };
        let bus = self.buses.iter_mut().find(|bus| bus.id == rack_bus)
            .ok_or_else(|| format!("Track group {rack_id} has no backing bus"))?;
        bus.output = output;
        self.graph_controller().apply_bus_output_routing(rack_bus);
        // The audible set under solo follows the chain, so re-derive it.
        self.push_bus_solo_mutes();
        Ok(())
    }

    /// Makes every rack bus's output agree with the group nesting, which is the
    /// source of truth. Used on load, where the two are deserialized
    /// independently and can disagree.
    pub(crate) fn reconcile_rack_group_bus_outputs(&mut self) {
        let desired = self.groups.iter()
            .filter(|group| group.is_rack())
            .map(|group| (group.id, self.rack_parent_group(group.id)
                .map(|parent| BusId(self.groups[parent].bus_id))))
            .collect::<Vec<_>>();
        for (rack_id, parent) in desired {
            let _ = self.set_rack_bus_output(rack_id, parent);
        }
    }

    /// Number of membership units in a group: tracks plus child racks. A plain
    /// group dissolves below two, exactly as it did when tracks were the only
    /// kind of member.
    fn group_unit_count(&self, group_index: usize) -> usize {
        let group = &self.groups[group_index];
        group.members.len() + group.rack_members.len()
    }

    /// Moves a rack into a plain group as one unit, chaining its bus into the
    /// parent's. Leaving a previous parent dissolves that parent if it drops
    /// below two units, mirroring `move_track_to_group_recorded`.
    pub fn move_rack_to_group_recorded(
        &mut self,
        rack_id: u64,
        target_group: usize,
    ) -> Result<(), String> {
        self.apply_recorded_bus_group_structure_mutation("Move rack to group", |app| {
            let target_id = app.groups.get(target_group)
                .map(|group| group.id)
                .ok_or_else(|| "Group is out of range".to_string())?;
            if !app.groups.iter().any(|group| group.id == rack_id && group.is_rack()) {
                return Err("Only drum racks can be grouped as a unit".to_string());
            }
            if app.groups[target_group].is_rack() {
                return Err("A drum rack cannot contain another rack".to_string());
            }
            if app.groups[target_group].rack_members.contains(&rack_id) {
                return Err("Rack is already in this group".to_string());
            }
            let source_id = app.rack_parent_group(rack_id).map(|index| app.groups[index].id);
            if source_id.is_some() {
                app.detach_rack_member(rack_id);
            }
            let target = app.groups.iter().position(|group| group.id == target_id)
                .expect("target group was resolved above");
            app.groups[target].rack_members.push(rack_id);
            let parent_bus = BusId(app.groups[target].bus_id);
            app.set_rack_bus_output(rack_id, Some(parent_bus))?;
            if let Some(source_id) = source_id {
                app.dissolve_group_if_undersized(source_id)?;
            }
            Ok(())
        })
    }

    /// Takes a rack back out of its parent group; its bus returns to the mix.
    pub fn remove_rack_from_group_recorded(&mut self, rack_id: u64) -> Result<(), String> {
        self.apply_recorded_bus_group_structure_mutation("Remove rack from group", |app| {
            let parent = app.rack_parent_group(rack_id)
                .ok_or_else(|| "Rack is not grouped".to_string())?;
            let parent_id = app.groups[parent].id;
            app.detach_rack_member(rack_id);
            app.set_rack_bus_output(rack_id, None)?;
            app.dissolve_group_if_undersized(parent_id)
        })
    }

    /// Removes `rack_id` from whatever plain group holds it. Bus routing is the
    /// caller's business, as with `detach_group_member`.
    fn detach_rack_member(&mut self, rack_id: u64) -> Option<usize> {
        let parent = self.rack_parent_group(rack_id)?;
        let position = self.groups[parent].rack_members.iter()
            .position(|member| *member == rack_id)?;
        self.groups[parent].rack_members.remove(position);
        Some(parent)
    }

    /// Dissolves a plain group (and its bus) once it holds fewer than two
    /// units. Racks never dissolve: their pads are lazy, so an empty rack is a
    /// legal kit waiting for sounds.
    fn dissolve_group_if_undersized(&mut self, group_id: u64) -> Result<(), String> {
        let Some(index) = self.groups.iter().position(|group| group.id == group_id) else {
            return Ok(());
        };
        if self.groups[index].is_rack() || self.group_unit_count(index) >= 2 {
            return Ok(());
        }
        let orphans = self.groups[index].rack_members.clone();
        for rack in orphans {
            self.detach_rack_member(rack);
            self.set_rack_bus_output(rack, None)?;
        }
        let bus = BusId(self.groups[index].bus_id);
        if !self.delete_bus_channel(bus) {
            return Err("Could not dissolve the group bus".to_string());
        }
        let index = self.groups.iter().position(|group| group.id == group_id)
            .expect("group was resolved above");
        self.groups.remove(index);
        Ok(())
    }

    /// Removes `track` from whatever group holds it, dropping the pad it backed
    /// and shifting the pads behind it. Returns the group index it left.
    /// Routing and group dissolution are the caller's business.
    fn detach_group_member(&mut self, track: usize) -> Option<usize> {
        let group_index = self.groups.iter()
            .position(|group| group.members.contains(&track))?;
        let group = &mut self.groups[group_index];
        let position = group.members.iter().position(|member| *member == track)?;
        group.members.remove(position);
        if let Some(rack) = group.rack.as_mut() {
            rack.remap_after_member_removed(position);
        }
        Some(group_index)
    }

    pub fn move_track_to_group_recorded(
        &mut self,
        track: usize,
        target_group: usize,
    ) -> Result<(), String> {
        self.apply_recorded_bus_group_structure_mutation("Move track to group", |app| {
            if track >= app.tracks.len() {
                return Err("Track is out of range".to_string());
            }
            let target_id = app.groups.get(target_group)
                .map(|group| group.id)
                .ok_or_else(|| "Group is out of range".to_string())?;
            if app.groups[target_group].members.contains(&track) {
                return Err("Track is already in this group".to_string());
            }
            let source_id = app.groups.iter()
                .find(|group| group.id != target_id && group.members.contains(&track))
                .map(|group| group.id);
            // Leave the old group first so the pad map (which addresses members
            // by position) is remapped before the track joins its new home.
            if source_id.is_some() {
                app.detach_group_member(track);
            }
            app.attach_track_to_group(track, target_id, None)?;
            if let Some(source_id) = source_id {
                app.dissolve_group_if_undersized(source_id)?;
            }
            Ok(())
        })
    }

    pub fn remove_track_from_group_recorded(&mut self, track: usize) -> Result<(), String> {
        self.apply_recorded_bus_group_structure_mutation("Remove track from group", |app| {
            let group = app.detach_group_member(track)
                .ok_or_else(|| "Track is not grouped".to_string())?;
            let group_id = app.groups[group].id;
            app.set_track_output_all_scenes_unrecorded(track, crate::sequencer::TrackOutput::Mix);
            // A rack keeps existing with zero members: its pads are lazy.
            app.dissolve_group_if_undersized(group_id)?;
            Ok(())
        })
    }

    /// Dissolves a group without deleting any of its member tracks. Direct
    /// members move into the enclosing plain group when dissolving a nested
    /// rack, or back to the master mix otherwise. Child racks follow the same
    /// scope. The group bus and rack-only pad metadata disappear in the same
    /// recorded topology edit, so undo restores the complete group identity.
    pub fn ungroup_tracks_recorded(&mut self, group_id: u64) -> Result<(), String> {
        self.apply_recorded_bus_group_structure_mutation("Ungroup tracks", |app| {
            let group_index = app.groups.iter().position(|group| group.id == group_id)
                .ok_or_else(|| format!("Track group {group_id} does not exist"))?;
            let group = app.groups[group_index].clone();
            let parent_id = app.rack_parent_group(group_id)
                .map(|parent| app.groups[parent].id);

            if parent_id.is_some() {
                app.detach_rack_member(group_id);
            }
            app.groups.remove(group_index);

            let destination = if let Some(parent_id) = parent_id {
                let parent = app.groups.iter_mut().find(|parent| parent.id == parent_id)
                    .ok_or_else(|| format!("Parent track group {parent_id} disappeared"))?;
                parent.members.extend(group.members.iter().copied());
                parent.rack_members.extend(group.rack_members.iter().copied());
                crate::sequencer::TrackOutput::Bus(BusId(parent.bus_id))
            } else {
                crate::sequencer::TrackOutput::Mix
            };

            for track in &group.members {
                app.set_track_output_all_scenes_unrecorded(*track, destination.clone());
            }
            let parent_bus = match destination {
                crate::sequencer::TrackOutput::Bus(bus) => Some(bus),
                _ => None,
            };
            for rack in &group.rack_members {
                app.set_rack_bus_output(*rack, parent_bus)?;
            }

            if !app.delete_bus_channel(BusId(group.bus_id)) {
                return Err("Could not delete the track group's backing bus".to_string());
            }
            if let Some(parent_id) = parent_id {
                app.dissolve_group_if_undersized(parent_id)?;
            }
            Ok(())
        })
    }

    pub fn delete_group_recorded(&mut self, group_id: u64) -> Result<(), String> {
        self.apply_recorded_bus_group_structure_mutation("Delete track group", |app| {
            let group = app.groups.iter().position(|group| group.id == group_id)
                .ok_or_else(|| format!("Track group {group_id} does not exist"))?;
            let members = app.groups[group].members.clone();
            for track in members {
                app.set_track_output_all_scenes_unrecorded(
                    track,
                    crate::sequencer::TrackOutput::Mix,
                );
            }
            // Child racks outlive their parent as free racks routed to the mix.
            let child_racks = app.groups[group].rack_members.clone();
            for rack in child_racks {
                app.detach_rack_member(rack);
                app.set_rack_bus_output(rack, None)?;
            }
            // Deleting a rack that sits inside a group vacates that slot, which
            // can leave the parent below two units.
            let parent_id = app.rack_parent_group(group_id)
                .map(|parent| app.groups[parent].id);
            if parent_id.is_some() {
                app.detach_rack_member(group_id);
            }
            let group = app.groups.iter().position(|group| group.id == group_id)
                .expect("group was resolved above");
            let bus = BusId(app.groups[group].bus_id);
            if !app.delete_bus_channel(bus) {
                return Err("Could not delete the track group's backing bus".to_string());
            }
            app.groups.remove(group);
            if let Some(parent_id) = parent_id {
                app.dissolve_group_if_undersized(parent_id)?;
            }
            Ok(())
        })
    }

    /// Deletes a mixer group as a container: its direct tracks and every rack
    /// nested in it are removed with their complete track lanes. The ordinary
    /// `delete_group_recorded` operation remains the non-destructive dissolve
    /// primitive; this is the destructive badge + Backspace operation.
    pub fn delete_group_with_members_recorded(&mut self, group_id: u64) -> Result<usize, String> {
        let group_index = self.groups.iter().position(|group| group.id == group_id)
            .ok_or_else(|| format!("Track group {group_id} does not exist"))?;
        let child_racks = self.groups[group_index].rack_members.clone();
        let mut tracks = self.groups[group_index].members.clone();
        for rack_id in &child_racks {
            let rack = self.groups.iter().find(|group| group.id == *rack_id)
                .ok_or_else(|| format!("Child rack {rack_id} does not exist"))?;
            tracks.extend(rack.members.iter().copied());
        }
        tracks.sort_unstable();
        tracks.dedup();

        let checkpoint = self.history.clone();
        let checkpoint_len = self.history.undo_len();
        let result = (|| {
            // Remove containers first. Track-deletion patches then capture the
            // already-unlinked structure, so undo restores tracks before it
            // restores group membership and bus routing.
            self.delete_group_recorded(group_id)?;
            for rack_id in child_racks {
                self.delete_group_recorded(rack_id)?;
            }
            // The graph deliberately keeps one live track. If this group owns
            // every track, create a fresh loose sampler before removing its
            // members; the creation joins the same composite history entry.
            if tracks.len() == self.tracks.len() {
                let replacement = self.graph_controller().add_blank_sampler_track()?;
                self.commit_created_track(replacement, "Replace deleted group")?;
            }
            let mut selected = self.tracks.len().saturating_sub(1);
            for track in tracks.into_iter().rev() {
                selected = self.delete_track_recorded(track)?;
            }
            squash_history_since(self, checkpoint_len, "Delete track group");
            Ok(selected)
        })();
        match result {
            Ok(selected) => Ok(selected),
            Err(error) => match rollback_history_to(self, checkpoint) {
                Ok(()) => Err(error),
                Err(rollback_error) => Err(format!(
                    "Track-group delete failed ({error}); rolling it back also failed ({rollback_error:?})"
                )),
            },
        }
    }

    /// Creates an empty drum rack: a track group carrying a rack config, its
    /// backing bus, and *zero* member tracks. Pads claim tracks lazily, when a
    /// sound is dropped on them (docs/drum-rack-v2-spec.md, "Track budget").
    pub fn create_drum_rack_recorded(
        &mut self,
        name: Option<String>,
    ) -> Result<(u64, BusId), String> {
        self.apply_recorded_bus_group_structure_mutation("Create drum rack", move |app| {
            let name = name.unwrap_or_else(|| format!("Drum Rack {}", app.groups.len() + 1));
            let bus = app.add_bus_channel(name.clone());
            let group_id = app.groups.iter().map(|group| group.id).max().unwrap_or(0) + 1;
            app.groups.push(crate::project::ProjectTrackGroup {
                id: group_id,
                name,
                color: DEFAULT_RACK_COLOR,
                collapsed: false,
                members: Vec::new(),
                bus_id: bus.0,
                rack: Some(crate::project::ProjectRackConfig::default()),
                rack_members: Vec::new(),
            });
            Ok((group_id, bus))
        })
    }

    /// Upgrades a plain track group to a drum rack without replacing its bus
    /// or member lanes. Existing members claim consecutive pads from C4 in
    /// group order, wrapping to C1 after the highest supported pad note.
    pub fn convert_group_to_drum_rack_recorded(
        &mut self,
        group_id: u64,
    ) -> Result<(), String> {
        self.apply_recorded_bus_group_structure_mutation("Convert group to drum rack", |app| {
            let group = app.groups.iter_mut().find(|group| group.id == group_id)
                .ok_or_else(|| format!("Track group {group_id} does not exist"))?;
            if group.is_rack() {
                return Err(format!("Track group {group_id} is already a drum rack"));
            }
            if !group.rack_members.is_empty() {
                return Err(
                    "A group containing drum racks cannot be converted to a drum rack"
                        .to_string(),
                );
            }
            if group.members.len() > crate::sequencer::DRUM_RACK_TOTAL_PAD_NOTES {
                return Err(format!(
                    "Track group {group_id} has more members than a drum rack has pads"
                ));
            }

            let mut rack = crate::project::ProjectRackConfig::default();
            for (member, pad_note) in group_conversion_pad_notes()
                .take(group.members.len())
                .enumerate()
            {
                rack.push_pad(crate::project::ProjectRackPad { pad_note, member });
            }
            if rack.pads.len() != group.members.len() {
                return Err("Could not assign every group member to a drum rack pad".to_string());
            }
            group.rack = Some(rack);
            Ok(())
        })
    }

    /// Browser "create drum rack", both halves as one transaction: the rack
    /// group (with its backing bus) and — when a sound was dropped onto the
    /// browser action — the member track that claims the rack's first pad.
    ///
    /// The group half records first, so a failing sample half used to leave a
    /// phantom rack behind: recorded, but denied by the status message and
    /// never synced into the UI. Roll the whole thing back instead, and on
    /// success squash the halves into a single "Add drum rack" undo entry.
    pub fn create_drum_rack_with_pad_recorded(
        &mut self,
        sample: Option<&std::path::Path>,
    ) -> Result<(u64, Option<usize>), String> {
        let checkpoint = self.history.clone();
        let checkpoint_len = self.history.undo_len();
        match self.create_drum_rack_with_pad(sample) {
            Ok((group_id, track)) => {
                if track.is_some() {
                    squash_history_since(self, checkpoint_len, "Add drum rack");
                }
                Ok((group_id, track))
            }
            Err(error) => match rollback_history_to(self, checkpoint) {
                Ok(()) => Err(error),
                Err(rollback_error) => Err(format!(
                    "Drum rack creation failed ({error}); rolling it back also failed ({rollback_error:?})"
                )),
            },
        }
    }

    fn create_drum_rack_with_pad(
        &mut self,
        sample: Option<&std::path::Path>,
    ) -> Result<(u64, Option<usize>), String> {
        let (group_id, _) = self.create_drum_rack_recorded(None)?;
        let Some(sample) = sample else {
            return Ok((group_id, None));
        };
        let track = self.graph_controller().add_track(sample)?;
        // The track is not in history yet, so undo cannot take it back: drop it
        // here if it never reaches the pad.
        let pad = Some(DRUM_RACK_FIRST_PAD_NOTE);
        if let Err(error) = self.attach_track_to_group(track, group_id, pad) {
            let _ = self.graph_controller().delete_track(track);
            return Err(error);
        }
        self.commit_created_track(track, "Add drum rack pad")?;
        Ok((group_id, Some(track)))
    }

    /// Adds an existing track to a group as a member routed into the group's
    /// backing bus. With `pad_note`, the group must be a rack and the track
    /// becomes the member backing that pad. Without one, a rack group still
    /// maps the member — to its next free pad note — because a padless rack
    /// member is invisible in the pad grid and unreachable by pad click, armed
    /// key or choke (docs/drum-rack-v2-spec.md, "Core model"). Every add path
    /// (mixer group-header drop, move-track-to-group, badge drag) therefore
    /// lands on the grid without having to know about pads.
    /// Unrecorded: callers either wrap this in a bus/group structure mutation
    /// or commit the created track.
    pub fn attach_track_to_group(
        &mut self,
        track: usize,
        group_id: u64,
        pad_note: Option<i32>,
    ) -> Result<(), String> {
        if track >= self.tracks.len() {
            return Err(format!("Invalid track index {}", track + 1));
        }
        let group_index = self.groups.iter().position(|group| group.id == group_id)
            .ok_or_else(|| format!("Track group {group_id} does not exist"))?;
        if self.groups.iter().any(|group| group.members.contains(&track)) {
            return Err(format!("Track {} already belongs to a group", track + 1));
        }
        let pad_note = match (pad_note, self.groups[group_index].rack.as_ref()) {
            (Some(_), None) => {
                return Err(format!("Track group {group_id} is not a drum rack"));
            }
            (Some(pad_note), Some(rack)) => {
                if !(DRUM_RACK_FIRST_PAD_NOTE..=DRUM_RACK_LAST_PAD_NOTE).contains(&pad_note) {
                    return Err(format!("Unsupported drum rack pad note {pad_note}"));
                }
                if rack.pad_index_for_note(pad_note).is_some() {
                    return Err(format!("Drum rack pad {pad_note} is already occupied"));
                }
                Some(pad_note)
            }
            // Joining a rack without a note: claim the next free pad rather
            // than land as an unreachable padless member. All notes mapped is
            // a full rack, not a silent half-add.
            (None, Some(rack)) => Some(rack.next_free_pad_note().ok_or_else(|| {
                format!("Drum rack {group_id} has no free pad left")
            })?),
            (None, None) => None,
        };
        let bus_id = BusId(self.groups[group_index].bus_id);
        self.set_track_output_all_scenes_unrecorded(
            track,
            crate::sequencer::TrackOutput::Bus(bus_id),
        );
        let group = &mut self.groups[group_index];
        // Members stay sorted; pads address members by position, so pads at or
        // after the insertion point shift up with them.
        let position = group.members.partition_point(|member| *member < track);
        group.members.insert(position, track);
        if let Some(rack) = group.rack.as_mut() {
            for pad in &mut rack.pads {
                if pad.member >= position {
                    pad.member += 1;
                }
            }
            if let Some(pad_note) = pad_note {
                rack.push_pad(crate::project::ProjectRackPad { pad_note, member: position });
            }
        }
        self.publish_rack_choke_runtime();
        Ok(())
    }

    /// Recorded form of [`App::attach_track_to_group`] for assigning an
    /// existing track to a rack pad.
    pub fn assign_rack_pad_track_recorded(
        &mut self,
        group_id: u64,
        pad_note: i32,
        track: usize,
    ) -> Result<(), String> {
        self.apply_recorded_bus_group_structure_mutation("Assign drum rack pad", |app| {
            app.attach_track_to_group(track, group_id, Some(pad_note))
        })
    }

    /// Sets (or clears, with `None`) the choke group of a rack pad. Choke is a
    /// cross-track voice release at trigger time (docs/drum-rack-v2-spec.md,
    /// "Trigger routing"), so the recorded mutation republishes the audio
    /// thread's per-track choke table on the way out.
    pub fn set_rack_pad_choke_group_recorded(
        &mut self,
        group_id: u64,
        pad_note: i32,
        choke: Option<u8>,
    ) -> Result<(), String> {
        // Choke group 0 packs to the "unassigned" runtime key (`rack_choke_key`),
        // so a pad stored with Some(0) would look assigned but never choke.
        if choke == Some(0) {
            return Err("Choke groups start at 1; use None to clear".to_string());
        }
        self.apply_recorded_bus_group_structure_mutation("Set drum rack choke group", |app| {
            let group = app.groups.iter_mut().find(|group| group.id == group_id)
                .ok_or_else(|| format!("Track group {group_id} does not exist"))?;
            let rack = group.rack.as_mut()
                .ok_or_else(|| format!("Track group {group_id} is not a drum rack"))?;
            let pad_index = rack.pad_index_for_note(pad_note)
                .ok_or_else(|| format!("Drum rack has no pad {pad_note}"))?;
            if rack.choke_group(pad_index) == choke {
                return Err("Pad choke group is unchanged".to_string());
            }
            rack.set_choke_group(pad_index, choke);
            Ok(())
        })
    }

    /// Moves a pad to a different note on the pad keyboard. The pad keeps its
    /// grid position, member track and choke group — only the note the live
    /// keyboard answers with changes (docs/drum-rack-v2-spec.md, "UI": the
    /// pad-note badge on a member row).
    pub fn set_rack_pad_note_recorded(
        &mut self,
        group_id: u64,
        pad_note: i32,
        new_note: i32,
    ) -> Result<(), String> {
        if !(DRUM_RACK_FIRST_PAD_NOTE..=DRUM_RACK_LAST_PAD_NOTE).contains(&new_note) {
            return Err(format!("Unsupported drum rack pad note {new_note}"));
        }
        self.apply_recorded_bus_group_structure_mutation("Set drum rack pad note", |app| {
            let group = app.groups.iter_mut().find(|group| group.id == group_id)
                .ok_or_else(|| format!("Track group {group_id} does not exist"))?;
            let rack = group.rack.as_mut()
                .ok_or_else(|| format!("Track group {group_id} is not a drum rack"))?;
            let pad_index = rack.pad_index_for_note(pad_note)
                .ok_or_else(|| format!("Drum rack has no pad {pad_note}"))?;
            if new_note == pad_note {
                return Err("Pad note is unchanged".to_string());
            }
            if rack.pad_index_for_note(new_note).is_some() {
                return Err(format!("Drum rack pad {new_note} is already occupied"));
            }
            rack.pads[pad_index].pad_note = new_note;
            Ok(())
        })
    }

    /// Resizes every member's own pattern in one history transaction. Members
    /// deliberately keep independent lengths, so each command applies the
    /// normal per-track clamping and duplication semantics.
    pub fn resize_drum_rack_patterns_recorded(
        &mut self,
        group_id: u64,
        change: PatternLengthChange,
    ) -> Result<EditOutcome, EditError> {
        let group = self
            .groups
            .iter()
            .find(|group| group.id == group_id)
            .ok_or_else(|| EditError::InvalidTarget(format!(
                "track group {group_id} does not exist"
            )))?;
        if group.rack.is_none() {
            return Err(EditError::InvalidTarget(format!(
                "track group {group_id} is not a drum rack"
            )));
        }
        let commands = group
            .members
            .iter()
            .copied()
            .map(|track| match change {
                PatternLengthChange::Double => AppCommand::DuplicateTrackPattern { track },
                PatternLengthChange::Halve => AppCommand::HalveTrackPattern { track },
            })
            .collect::<Vec<_>>();
        let label = match change {
            PatternLengthChange::Double => "Double drum rack patterns",
            PatternLengthChange::Halve => "Halve drum rack patterns",
        };
        apply_recorded_pattern_geometry_commands(self, &commands, label)
    }

    pub fn rename_group_recorded(&mut self, group_id: u64, name: String) -> Result<(), String> {
        let name = name.trim().to_string();
        self.apply_recorded_bus_group_structure_mutation("Rename track group", |app| {
            if name.is_empty() {
                return Err("Track group name cannot be empty".to_string());
            }
            let group = app.groups.iter_mut().find(|group| group.id == group_id)
                .ok_or_else(|| format!("Track group {group_id} does not exist"))?;
            if group.name == name {
                return Err("Track group name is unchanged".to_string());
            }
            group.name.clone_from(&name);
            if let Some(bus) = app.buses.iter_mut().find(|bus| bus.id.0 == group.bus_id) {
                bus.name.clone_from(&name);
            }
            Ok(())
        })
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
            TrackInstrumentSource::Modulator => crate::instruments::track_modulator::descriptor(),
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
        self.normalize_track_name_authorship();
        self.track_name_user_authored[track] = target.display_name_user_authored;
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
            self.normalize_track_name_authorship();
            self.track_name_user_authored[track] = patch.state.display_name_user_authored;
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
        if let Some((group_id, _)) = patch.group {
            self.attach_track_to_group(track, group_id, patch.rack_pad)?;
        }
        self.state.publish_scheduler_snapshot();
        Ok(())
    }

    fn remap_groups_after_track_delete(&mut self, deleted: usize) {
        for group in &mut self.groups {
            let position = group.members.iter().position(|member| *member == deleted);
            group.members.retain(|member| *member != deleted);
            for member in &mut group.members {
                if *member > deleted {
                    *member -= 1;
                }
            }
            // Pads address members by position in `members`, so a removed
            // member drops its pad and shifts the pads behind it down.
            if let (Some(position), Some(rack)) = (position, group.rack.as_mut()) {
                rack.remap_after_member_removed(position);
            }
        }
        // A rack with no pads is still a rack (lazy pads); a plain group is
        // still a group while it holds a child rack, whose bus chains into
        // this group's bus. A plain group with neither is nothing at all.
        // Mirrors the load-path predicate in `projects.rs` (see the group
        // filter around `!group.rack_members.is_empty()`); the two must agree,
        // or a group dropped here silently reroutes its racks to the mix on
        // reload.
        self.groups.retain(|group| {
            group.is_rack() || !group.members.is_empty() || !group.rack_members.is_empty()
        });
        self.publish_rack_choke_runtime();
    }

    /// Deletes several tracks as one atomic, undoable edit. Indices refer to
    /// the topology at method entry and are removed in descending order so
    /// each deletion continues to address the intended track.
    pub fn delete_tracks_recorded(&mut self, mut tracks: Vec<usize>) -> Result<usize, String> {
        finish_active_gesture(self);
        tracks.sort_unstable();
        tracks.dedup();
        if tracks.is_empty() {
            return Err("No tracks selected for deletion".to_string());
        }
        if tracks.last().is_some_and(|track| *track >= self.tracks.len()) {
            return Err("The track selection contains a missing track".to_string());
        }
        if tracks.len() >= self.tracks.len() {
            return Err("Cannot delete all remaining tracks".to_string());
        }

        let checkpoint = self.history.clone();
        let checkpoint_len = self.history.undo_len();
        let result = (|| {
            let mut selected = self.tracks.len().saturating_sub(1);
            for track in tracks.into_iter().rev() {
                selected = self.delete_track_recorded(track)?;
            }

            // Plain groups represent at least two mixer units. Deleting
            // several members at once can leave one track or rack behind, so
            // dissolve those containers inside the same history transaction.
            let undersized_groups: Vec<u64> = self.groups.iter()
                .enumerate()
                .filter(|(index, group)| !group.is_rack() && self.group_unit_count(*index) < 2)
                .map(|(_, group)| group.id)
                .collect();
            for group_id in undersized_groups {
                self.delete_group_recorded(group_id)?;
            }
            squash_history_since(self, checkpoint_len, "Delete tracks");
            Ok(selected.min(self.tracks.len().saturating_sub(1)))
        })();
        match result {
            Ok(selected) => Ok(selected),
            Err(error) => match rollback_history_to(self, checkpoint) {
                Ok(()) => Err(error),
                Err(rollback_error) => Err(format!(
                    "Multi-track delete failed ({error}); rolling it back also failed ({rollback_error:?})"
                )),
            },
        }
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
        self.graph.record_armed[appended] = patch.record_armed;
        self.state.move_appended_track_pattern_lane_to(patch.index, &patch.patterns)?;
        self.graph_controller().move_appended_track_to(patch.index)?;
        self.groups = patch.groups.clone();
        // The runtime choke table still holds the post-delete reindexed keys;
        // restoring the pre-delete groups has to republish it or choke fires
        // on the wrong tracks until the next group edit.
        self.publish_rack_choke_runtime();
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
        self.normalize_track_name_authorship();
        self.track_name_user_authored[track] = target.display_name_user_authored;
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
        let pattern_id = app
            .state
            .effective_track_pattern_id(track)
            .ok_or(EditError::MissingTrackPattern)?;
        Self::begin_for(app, track, pattern_id, steps, label)
    }

    /// Begin against an explicit Track Pattern target (clip-edit-target spec
    /// 3.4): `capture_pattern_step_cells` reads the live lanes when the target
    /// is effective, else the pool, so one constructor covers both routes.
    pub fn begin_for(
        app: &App,
        track: usize,
        pattern_id: PatternId,
        steps: &[usize],
        label: &'static str,
    ) -> Result<Self, EditError> {
        let track_id = app
            .track_registry
            .id_at(track)
            .ok_or(EditError::TrackOutOfRange { track })?;
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
        self.capture_additional_steps_for_target(app, track, steps)
    }

    /// `capture_additional_steps` without the effective-pattern requirement:
    /// pinned pool/take targets (clip-edit-target spec 3.4) stay valid while
    /// not effective. Callers (`FocusStepGesture`) own the focus bail-out.
    fn capture_additional_steps_for_target(
        &mut self,
        app: &App,
        track: usize,
        steps: &[usize],
    ) -> Result<(), EditError> {
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
        let label = self.label;
        match self.build_patch(app)? {
            None => Ok(EditOutcome::NoOp),
            Some(patch) => {
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
        }
    }

    /// The capture-after/delta/normalize half of `commit`, without the
    /// history entry: `FocusStepGesture` composes several per-pattern patches
    /// into ONE undo entry (clip-edit-target spec 3.4, take chunks).
    fn build_patch(self, app: &mut App) -> Result<Option<StepCellsPatch>, EditError> {
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
            return Ok(None);
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
        Ok(Some(patch))
    }
}

/// One editor gesture against the resolved focus (clip-edit-target spec 3.4):
/// a set of per-pattern [`StepGestureTransaction`]s — one for a pattern
/// target, one per touched chunk for a take — committed as a single history
/// entry. Steps are addressed on the FOCUS axis (pattern steps, or the take's
/// continuous step axis) and mapped onto chunk-local steps here.
pub struct FocusStepGesture {
    label: &'static str,
    focus: crate::app::focus::EditFocus,
    parts: Vec<StepGestureTransaction>,
}

/// Map focus-axis steps onto per-pattern (target, local steps) groups.
///
/// Take steps map through the same arithmetic as `TrackTake::chunk_step_at`
/// (takes spec 6.1): at/past `total_len_steps` is the silent tail and is
/// rejected — the editor must not write notes the take can never play.
fn focus_step_targets(
    app: &App,
    focus: crate::app::focus::EditFocus,
    steps: &[usize],
) -> Result<Vec<(PatternId, Vec<usize>)>, EditError> {
    use crate::app::focus::EditFocus;
    match focus {
        EditFocus::Live { track } => {
            let pattern = app
                .state
                .effective_track_pattern_id(track)
                .ok_or(EditError::MissingTrackPattern)?;
            Ok(vec![(pattern, normalized_steps(steps))])
        }
        EditFocus::Pattern { pattern, .. } => Ok(vec![(pattern, normalized_steps(steps))]),
        EditFocus::Take { track, take } => {
            let take = app
                .state
                .with_project_scenes(|scenes| {
                    scenes
                        .take_pools
                        .get(track)
                        .and_then(|takes| takes.get(take))
                        .cloned()
                })
                .ok_or(EditError::MissingTrackPattern)?;
            // Take-axis steps legitimately exceed MAX_STEPS (the axis spans
            // every chunk), so `normalized_steps`'s MAX_STEPS filter must not
            // run here — `chunk_step_at` enforces the real bound.
            let mut axis_steps = steps.to_vec();
            axis_steps.sort_unstable();
            axis_steps.dedup();
            let mut groups: Vec<(PatternId, Vec<usize>)> = Vec::new();
            for step in axis_steps {
                let (chunk_idx, local) = take
                    .chunk_step_at(step as f64)
                    .ok_or(EditError::InvalidStepRange)?;
                let chunk = *take
                    .chunks
                    .get(chunk_idx)
                    .ok_or(EditError::InvalidStepRange)?;
                let local = local as usize;
                match groups.iter_mut().find(|(id, _)| *id == chunk) {
                    Some((_, locals)) => locals.push(local),
                    None => groups.push((chunk, vec![local])),
                }
            }
            Ok(groups)
        }
    }
}

impl FocusStepGesture {
    pub fn begin(
        app: &mut App,
        focus: crate::app::focus::EditFocus,
        steps: &[usize],
        label: &'static str,
    ) -> Result<Self, EditError> {
        let track = focus.track();
        if focus.is_live() {
            // A bare scene cell materializes its pattern on first edit
            // (takes spec 11.1), exactly like the legacy live path.
            ensure_effective_track_pattern(app, track).ok_or(EditError::MissingTrackPattern)?;
        }
        let parts = focus_step_targets(app, focus, steps)?
            .into_iter()
            .map(|(pattern, local_steps)| {
                StepGestureTransaction::begin_for(app, track, pattern, &local_steps, label)
            })
            .collect::<Result<Vec<_>, _>>()?;
        if parts.is_empty() {
            return Err(EditError::InvalidStepRange);
        }
        Ok(Self { label, focus, parts })
    }

    pub fn focus(&self) -> crate::app::focus::EditFocus {
        self.focus
    }

    /// Extend the gesture to more focus-axis steps, bailing out when the
    /// RESOLVED focus moved under the drag (clip-edit-target spec 3.3.3) —
    /// a scene launch in follow mode, or a re-bind/invalidation in song mode.
    pub fn capture_additional_steps(
        &mut self,
        app: &mut App,
        steps: &[usize],
    ) -> Result<(), EditError> {
        let track = self.focus.track();
        if app.track_edit_focus(track) != self.focus {
            return Err(EditError::MissingTrackPattern);
        }
        // Follow mode carries no pattern id in the focus itself, so the
        // legacy bail must run HERE: a scene launch mid-drag re-resolves to
        // a different effective pattern, and continuing (or growing a new
        // part for it) would silently migrate the drag onto the launched
        // scene's pattern (spec 3.3.3).
        if let crate::app::focus::EditFocus::Live { .. } = self.focus {
            let begun = self
                .parts
                .first()
                .map(|part| part.target.pattern)
                .ok_or(EditError::MissingTrackPattern)?;
            if app.state.effective_track_pattern_id(track) != Some(begun) {
                return Err(EditError::MissingTrackPattern);
            }
        }
        for (pattern, local_steps) in focus_step_targets(app, self.focus, steps)? {
            match self
                .parts
                .iter_mut()
                .find(|part| part.target.pattern == pattern)
            {
                Some(part) => {
                    part.capture_additional_steps_for_target(app, track, &local_steps)?
                }
                // Only a take gesture may legitimately grow new parts (the
                // drag reached another chunk); pattern-focus gestures always
                // resolve to the single part begun above.
                None if matches!(self.focus, crate::app::focus::EditFocus::Take { .. }) => {
                    self.parts.push(StepGestureTransaction::begin_for(
                        app,
                        track,
                        pattern,
                        &local_steps,
                        self.label,
                    )?)
                }
                None => return Err(EditError::MissingTrackPattern),
            }
        }
        Ok(())
    }

    pub fn rollback(self, app: &mut App) -> Result<(), EditError> {
        let mut first_error = None;
        for part in self.parts {
            if let Err(error) = part.rollback(app) {
                first_error.get_or_insert(error);
            }
        }
        match first_error {
            None => Ok(()),
            Some(error) => Err(error),
        }
    }

    /// Commit every touched pattern as ONE history entry: a plain step-cells
    /// patch for a single target, a composite for a multi-chunk take gesture.
    pub fn commit(self, app: &mut App) -> Result<EditOutcome, EditError> {
        let label = self.label;
        let focus = self.focus;
        let track = focus.track();
        let mut patches = Vec::new();
        let mut parts = self.parts.into_iter();
        while let Some(part) = parts.next() {
            match part.build_patch(app) {
                Ok(Some(patch)) => patches.push(patch),
                Ok(None) => {}
                Err(error) => {
                    // Best effort: unwind what this gesture already changed so
                    // no history-less edits survive a failed commit.
                    for patch in &patches {
                        let before_cells = patch
                            .cells
                            .iter()
                            .map(|cell| (cell.step, cell.before.clone()))
                            .collect::<Vec<_>>();
                        let _ = app.state.restore_pattern_step_cells_no_publish(
                            track,
                            patch.target.pattern,
                            &before_cells,
                            &patch.variant_registry_before,
                        );
                    }
                    for part in parts {
                        let _ = part.rollback(app);
                    }
                    return Err(error);
                }
            }
        }
        if patches.is_empty() {
            return Ok(EditOutcome::NoOp);
        }
        let targets: Vec<PatternId> =
            patches.iter().map(|patch| patch.target.pattern).collect();
        let patch = if patches.len() == 1 {
            EditPatch::StepCells(patches.pop().expect("one patch"))
        } else {
            EditPatch::Composite(patches.into_iter().map(EditPatch::StepCells).collect())
        };
        let retained_bytes = edit_patch_retained_bytes(&patch);
        finish_active_gesture(app);
        let history_move = app.history.commit(label, None, patch, retained_bytes);
        // A pinned pool/take target the playing song resolves needs the
        // prebuilt row snapshots refreshed (they cloned the pattern at
        // preflight). The LIVE publish deliberately does not live here:
        // gesture finishes must not republish (drag frames already did —
        // the coalescing contract), so `apply_recorded_focus_step_mutation`
        // owns the legacy replay tail for one-shot edits.
        if !focus.is_live() {
            for pattern in targets {
                invalidate_song_rows_for_edit(app, track, pattern);
            }
        }
        Ok(EditOutcome::Applied(history_move))
    }
}

/// [`apply_recorded_step_mutation`] against a resolved focus: pool-first
/// writes with the effective-pattern mirror rule (clip-edit-target spec 3.4),
/// steps on the focus axis, one undo entry.
pub fn apply_recorded_focus_step_mutation(
    app: &mut App,
    focus: crate::app::focus::EditFocus,
    steps: &[usize],
    label: &'static str,
    mutate: impl FnOnce(&mut App) -> Result<(), EditError>,
) -> Result<EditOutcome, EditError> {
    if steps.is_empty() {
        return Ok(EditOutcome::NoOp);
    }
    let gesture = FocusStepGesture::begin(app, focus, steps, label)?;
    if let Err(error) = mutate(app) {
        return match gesture.rollback(app) {
            Ok(()) => Err(error),
            Err(rollback_error) => Err(EditError::ReplayFailed(format!(
                "{error:?}; rollback also failed: {rollback_error:?}"
            ))),
        };
    }
    let outcome = gesture.commit(app)?;
    // The legacy `replay_step_patch(Redo)` tail for a one-shot live edit:
    // the audio thread reads the PUBLISHED snapshot, so the effective
    // target must publish, and playing song rows that resolve it must
    // re-preflight. (Gesture drags publish per frame instead.)
    if matches!(outcome, EditOutcome::Applied(_)) && focus.is_live() {
        let track = focus.track();
        app.state.publish_scheduler_track(track);
        if let Some(pattern) = app.state.effective_track_pattern_id(track) {
            invalidate_song_rows_for_edit(app, track, pattern);
        }
    }
    Ok(outcome)
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
    let TrackParamsSnapshot {
        gate,
        attack_ms,
        release_ms,
        swing,
        swing_resolution,
        num_steps,
        volume,
        pan,
        mute,
        send,
        output,
        sends,
        polyphonic,
        max_polyphony,
        timebase,
        accumulator_idx,
        script_accumulator_name,
        midi_fx_chain,
        midi_fx_position,
        accum_limit,
        accum_mode,
        fts_scale,
        mute_group,
        global_transpose,
    } = snapshot;
    let mut bytes = WitnessBytes::default();
    bytes.bool(*gate);
    bytes.f32(*attack_ms);
    bytes.f32(*release_ms);
    bytes.f32(*swing);
    bytes.u32(*swing_resolution as u32);
    bytes.usize(*num_steps);
    bytes.f32(*volume);
    bytes.f32(*pan);
    bytes.bool(*mute);
    bytes.f32(*send);
    match output {
        crate::sequencer::TrackOutput::Mix => bytes.u32(0),
        crate::sequencer::TrackOutput::Bus(id) => {
            bytes.u32(1);
            bytes.u64(id.0);
        }
        crate::sequencer::TrackOutput::None => bytes.u32(2),
    }
    bytes.usize(sends.len());
    for send in sends {
        bytes.u64(send.destination.0);
        bytes.f32(send.amount);
    }
    bytes.bool(*polyphonic);
    bytes.usize(*max_polyphony);
    bytes.u32(*timebase as u32);
    bytes.usize(*accumulator_idx);
    bytes.bool(script_accumulator_name.is_some());
    if let Some(name) = script_accumulator_name {
        bytes.usize(name.len());
        bytes.0.extend_from_slice(name.as_bytes());
    }
    bytes.usize(midi_fx_chain.len());
    for name in midi_fx_chain {
        bytes.usize(name.len());
        bytes.0.extend_from_slice(name.as_bytes());
    }
    bytes.u32(*midi_fx_position as u32);
    bytes.f32(*accum_limit);
    bytes.u32(*accum_mode);
    bytes.usize(*fts_scale);
    bytes.u32(*mute_group as u32);
    bytes.bool(*global_transpose);
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
    let EffectSlotSnapshot {
        node_id: _,
        modulator_node_id: _,
        num_params,
        defaults,
        plocks,
        plock_param_ids: _,
        key_locks,
        key_lock_param_ids: _,
        tensor_params,
        param_node_indices: _,
        param_node_spans: _,
        transport_phase_param_idx: _,
        ir,
        table,
    } = snapshot;
    bytes.usize(*num_params as usize);
    bytes.f32_slice(defaults);
    encode_optional_f32_rows(bytes, plocks);
    bytes.usize(key_locks.len());
    for (note, values) in key_locks {
        bytes.0.push(*note);
        bytes.usize(values.len());
        for value in values {
            bytes.optional_f32(*value);
        }
    }
    bytes.usize(tensor_params.len());
    for tensor in tensor_params {
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
    bytes.bool(ir.is_some());
    if let Some(ir) = ir {
        bytes.usize(ir.len());
        bytes.0.extend_from_slice(ir.as_bytes());
    }
    bytes.bool(table.is_some());
    if let Some(table) = table {
        bytes.usize(table.len());
        bytes.0.extend_from_slice(table.as_bytes());
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
        AppCommand::ClearInstrumentKeyLocksForNote { track, .. }
        | AppCommand::StampInstrumentKeyLockVariant { track, .. }
        | AppCommand::ClearInstrumentKeyLockVariantsForNotes { track, .. } => {
            if app.state.pattern.instrument_slots.get(*track).is_none() {
                return Err(EditError::TrackOutOfRange { track: *track });
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
        | AppCommand::DuplicateTrackPattern { .. }
        | AppCommand::HalveTrackPattern { .. }
        | AppCommand::SetTimebasePlock { .. }
        | AppCommand::SetTimebasePlockMulti { .. }
        | AppCommand::ClearTimebasePlockMulti { .. }
        | AppCommand::ToggleTrackGate { .. }
        | AppCommand::ToggleTrackPolyphonic { .. }
        | AppCommand::ToggleTrackMute { .. }
        | AppCommand::ToggleTrackSolo { .. }
        | AppCommand::AdjustTrackMaxPolyphony { .. }
        | AppCommand::SetTrackMaxPolyphony { .. }
        | AppCommand::SetTrackAttack { .. }
        | AppCommand::AdjustTrackAttack { .. }
        | AppCommand::SetTrackRelease { .. }
        | AppCommand::AdjustTrackRelease { .. }
        | AppCommand::SetTrackSwing { .. }
        | AppCommand::SetTrackSwingPlock { .. }
        | AppCommand::SetTrackSwingPlockMulti { .. }
        | AppCommand::ClearTrackSwingPlockMulti { .. }
        | AppCommand::AdjustTrackSwing { .. }
        | AppCommand::SetTrackSwingResolution { .. }
        | AppCommand::SetTrackSwingResolutionPlock { .. }
        | AppCommand::SetTrackSwingResolutionPlockMulti { .. }
        | AppCommand::ClearTrackSwingResolutionPlockMulti { .. }
        | AppCommand::NextTrackSwingResolution { .. }
        | AppCommand::PrevTrackSwingResolution { .. }
        | AppCommand::SetTrackNumSteps { .. }
        | AppCommand::AdjustTrackNumSteps { .. }
        | AppCommand::SetTrackVolume { .. }
        | AppCommand::AdjustTrackVolume { .. }
        | AppCommand::SetTrackPan { .. }
        | AppCommand::AdjustTrackPan { .. }
        | AppCommand::SetTrackSend { .. }
        | AppCommand::AdjustTrackSend { .. }
        | AppCommand::SetTrackOutput { .. }
        | AppCommand::SetTrackSends { .. }
        | AppCommand::SetTrackBusSendPlock { .. }
        | AppCommand::SetBusVolume { .. }
        | AppCommand::ToggleBusMute { .. }
        | AppCommand::ToggleBusSolo { .. }
        | AppCommand::SetMasterVolume { .. }
        | AppCommand::AdjustMasterVolume { .. }
        | AppCommand::SetReverbParam { .. }
        | AppCommand::SetTrackTimebase { .. }
        | AppCommand::NextTrackTimebase { .. }
        | AppCommand::PrevTrackTimebase { .. }
        | AppCommand::SetTrackFtsScale { .. }
        | AppCommand::SetTrackAccumIdx { .. }
        | AppCommand::SetTrackAccumLimit { .. }
        | AppCommand::AdjustTrackAccumLimit { .. }
        | AppCommand::SetTrackAccumMode { .. }
        | AppCommand::SetTrackMuteGroup { .. }
        | AppCommand::SetTrackGlobalTranspose { .. }
        | AppCommand::SetInstrumentBaseNoteOffset { .. }
        | AppCommand::MacroCreate { .. }
        | AppCommand::MacroCreateScene { .. }
        | AppCommand::MacroSceneConfig { .. }
        | AppCommand::MacroEnsure { .. }
        | AppCommand::MacroDelete { .. }
        | AppCommand::MacroRename { .. }
        | AppCommand::MacroSetValue { .. }
        | AppCommand::MacroRelease { .. }
        | AppCommand::ScenePushBegin { .. }
        | AppCommand::ScenePushSetValue { .. }
        | AppCommand::ScenePushEnd
        | AppCommand::MacroMapParam { .. }
        | AppCommand::MacroSetRange { .. }
        | AppCommand::MacroSetCurve { .. }
        | AppCommand::MacroUnmap { .. }
        | AppCommand::TogglePlay
        | AppCommand::SetBpm { .. }
        | AppCommand::AdjustRecordQuantizeThresh { .. } => {}
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
        | AppCommand::SetTrackBusSendPlock { track, step, .. }
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
        AppCommand::SetReverbParam { param_idx, .. } => {
            let value = match param_idx {
                0 => app.ui.reverb_size,
                1 => app.ui.reverb_brightness,
                2 => app.ui.reverb_replace,
                _ => return Err(EditError::UnsupportedCommand),
            };
            Ok(BarrierWitness::Bytes(value.to_bits().to_le_bytes().to_vec()))
        }
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
    BusSendPlock {
        steps: Vec<usize>,
        destination: BusId,
        value: Option<f32>,
    },
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
            | Self::BusSendPlock { steps, .. }
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
            Self::BusSendPlock { value: Some(_), .. } => "Set track bus-send p-lock",
            Self::BusSendPlock { value: None, .. } => "Clear track bus-send p-lock",
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
        AppCommand::SetTrackBusSendPlock {
            track,
            step,
            destination,
            value,
        } => (
            *track,
            ResolvedStepCommand::BusSendPlock {
                steps: vec![*step],
                destination: *destination,
                value: *value,
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
            app.state.set_step_param_no_publish(track, *step, *param, *value);
        }
        ResolvedStepCommand::AdjustParam { step, param, delta } => {
            let current = app.state.pattern.step_data[track].get(*step, *param);
            app.state
                .set_step_param_no_publish(track, *step, *param, current + delta);
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
                app.state.set_step_param_no_publish(
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
        ResolvedStepCommand::BusSendPlock {
            steps,
            destination,
            value,
        } => {
            for step in steps {
                match value {
                    Some(value) => app.state.pattern.track_send_plocks[track]
                        .set(*step, *destination, *value),
                    None => app.state.pattern.track_send_plocks[track]
                        .clear(*step, *destination),
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
            Some(format!("rack:{slot_idx}:strip:{}", param.index()))
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
    // Lazily materialize the pattern when the current scene is bare for the
    // track (takes spec 11.1): the first edit in a bare scene creates the
    // pattern and lifts the empty-cell silencing.
    let pattern_id =
        ensure_effective_track_pattern(app, track).ok_or(EditError::MissingTrackPattern)?;
    let target = TrackPatternId {
        track: track_id,
        pattern: pattern_id,
    };
    let affected_key = affected.iter().map(usize::to_string).collect::<Vec<_>>().join(",");
    let merge_key = MergeKey::new(format!(
        "device-plock:{}:{}:{merge_suffix}:steps:{affected_key}",
        target.track.0,
        target.pattern.0,
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

/// Effective pattern id for `track`, lazily materializing one when the
/// current scene is bare for the track (takes spec 11.1): device edits are
/// keyed per-pattern, so the first edit in a bare scene creates the pattern
/// from the live track state — with the step content blanked, because the
/// live step grid still holds the previous scene's notes, which the bare
/// scene must not inherit.
fn ensure_effective_track_pattern(
    app: &mut App,
    track: usize,
) -> Option<crate::sequencer::PatternId> {
    if let Some(id) = app.state.effective_track_pattern_id(track) {
        return Some(id);
    }
    // Capture seam (takes spec 17.10): the materialized pattern must carry
    // base values, never an engaged macro override.
    app.debug_assert_no_macro_override_leak();
    let snapshot = app.state.capture_current_pattern_snapshot(
        app.tracks.len(),
        &app.graph.track_buffer_ids,
        &app.graph.track_sample_rates,
        &app.tracks,
        &app.graph.track_instrument_types,
    );
    let mut data = snapshot.track_pattern_data(track)?;
    data.track_bits = Default::default();
    data.neural_reset_bits = Default::default();
    for chord in &mut data.chord_snapshot.steps {
        chord.clear();
    }
    for durations in &mut data.chord_snapshot.durations {
        durations.clear();
    }
    for delays in &mut data.chord_snapshot.delays {
        delays.clear();
    }
    app.state.materialize_current_scene_pattern(track, data)
}

/// Rule 3's write target, view-keyed (track-sound spec §2.2.2): in
/// ARRANGEMENT context the track owns the sound, so the edit lands on the
/// track-sound carrier — never a cell, never a materialization, wherever the
/// cursor sits and whatever cells exist (symptom 8's stopped-time preset). In
/// SEQ context it is the classic effective cell, with the carrier only as the
/// bare-lane fallback that keeps device edits from minting a cell.
fn rule_three_device_target(app: &App, track: usize) -> Option<crate::sequencer::PatternId> {
    if app.arrangement_view_visible {
        return app.state.track_sound_pattern_id(track);
    }
    app.state
        .effective_track_pattern_id(track)
        .or_else(|| app.state.track_sound_pattern_id(track))
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
    // Device edits follow the track's sound binding (takes spec 16.4): a
    // bound take or track clip owns them, and only rule 3 falls back to the
    // view's owner (track-sound spec §2.2.2) — the TRACK SOUND's carrier in
    // arrangement context, the effective cell in Seq context. A device edit
    // never materializes a scene cell. No dual-write — a bound edit never
    // touches the scene pattern.
    let pattern = match app.bound_read_pattern(track) {
        Some(pattern) if !app.track_sound_binding(track).is_scene() => pattern,
        _ => rule_three_device_target(app, track).ok_or(EditError::MissingTrackPattern)?,
    };
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
    let note_list = |notes: &[u8]| {
        let mut notes = notes.to_vec();
        notes.sort_unstable();
        notes.dedup();
        notes.iter().map(u8::to_string).collect::<Vec<_>>().join(",")
    };
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
        AppCommand::SetInstrumentKeyLock { note, param_idx, .. }
        | AppCommand::ClearInstrumentKeyLock { note, param_idx, .. } => {
            format!("key-lock:note:{note}:param:{param_idx}")
        }
        AppCommand::SetInstrumentKeyLockMulti { notes, param_idx, .. } => {
            format!("key-lock:notes:{}:param:{param_idx}", note_list(notes))
        }
        AppCommand::ClearInstrumentKeyLocksForNote { note, .. } => {
            format!("key-lock:note:{note}:all")
        }
        AppCommand::StampInstrumentKeyLockVariant { notes, key, .. } => {
            let entries = key.entries.iter().map(|entry| format!(
                "{}:{}:{}:{}:{}",
                plock_variant_domain_merge_component(entry.domain),
                entry.slot,
                entry.param,
                entry.cell.map(|cell| cell.to_string()).unwrap_or_default(),
                entry.value_bits,
            )).collect::<Vec<_>>().join(";");
            format!("key-lock:notes:{}:variant:{entries}", note_list(notes))
        }
        AppCommand::ClearInstrumentKeyLockVariantsForNotes { notes, .. } => {
            format!("key-lock:notes:{}:all", note_list(notes))
        }
        _ => device_value_label(cmd).to_string(),
    }
}

fn plock_variant_domain_merge_component(
    domain: crate::plock_variants::PlockVariantDomain,
) -> &'static str {
    use crate::plock_variants::PlockVariantDomain;
    match domain {
        PlockVariantDomain::TrackTimebase => "track-timebase",
        PlockVariantDomain::TrackSwing => "track-swing",
        PlockVariantDomain::TrackSwingResolution => "track-swing-resolution",
        PlockVariantDomain::MidiEffect => "midi-effect",
        PlockVariantDomain::MidiEffectTensor => "midi-effect-tensor",
        PlockVariantDomain::Instrument => "instrument",
        PlockVariantDomain::InstrumentTensor => "instrument-tensor",
        PlockVariantDomain::Effect => "effect",
        PlockVariantDomain::EffectTensor => "effect-tensor",
        PlockVariantDomain::RackMacro => "rack-macro",
        PlockVariantDomain::RackSlotParam => "rack-slot-param",
        PlockVariantDomain::RackSlotInstrument => "rack-slot-instrument",
        PlockVariantDomain::RackSlotInstrumentTensor => "rack-slot-instrument-tensor",
        PlockVariantDomain::InstrumentKeyLock => "instrument-key-lock",
    }
}

fn device_id_merge_component(id: DeviceId) -> String {
    match id {
        DeviceId::TrackInstrument(id) => format!("instrument:{}", id.0),
        DeviceId::AudioEffect(id) => format!("audio-effect:{}", id.0),
        DeviceId::MidiEffect(id) => format!("midi-effect:{}", id.0),
        DeviceId::RackSlot(id) => format!("rack-slot:{}", id.0),
        DeviceId::RackInstrument(id) => format!("rack-instrument:{}", id.0),
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

/// Edit-through for a pool pattern the playing song may resolve: re-preflight
/// the rows so everything the playhead has not reached becomes correct
/// (takes spec 16.7 for device edits,
/// docs/realtime-arrangement-feedback-spec.md 5.1 for note edits).
///
/// Re-preflighting is far too heavy for every frame of a knob drag, and the
/// audible row already heard a device value through the direct engine push —
/// so while a gesture is open this defers to its end through
/// `pending_song_row_invalidation` (flushed in `finish_active_gesture`).
pub(crate) fn invalidate_song_rows_for_edit(
    app: &mut App,
    track: usize,
    pattern: crate::sequencer::PatternId,
) {
    if app.history.active_gesture().is_some() {
        app.pending_song_row_invalidation = Some((track, pattern));
    } else {
        app.invalidate_song_rows_for_pattern(track, pattern);
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

fn rollback_device_value_edit(
    app: &mut App,
    target: ResolvedDeviceTarget,
    before: &DeviceValueSnapshot,
    error: EditError,
) -> EditError {
    let rollback = restore_device_value_snapshot(app, target, before)
        .and_then(|_| push_live_device_values(app, target, Some(before)));
    // Publish even when the rollback itself failed: the commands already
    // mutated live state, and skipping the publish leaves the scheduler
    // playing stale values until the next unrelated publish.
    app.state.publish_scheduler_snapshot();
    match rollback {
        Ok(()) => error,
        Err(rollback_error) => EditError::ReplayFailed(format!(
            "{error:?}; restoring device values also failed: {rollback_error:?}"
        )),
    }
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
            "device:{}:pattern:{}:{}",
            device_id_merge_component(target.id),
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
    let after = match capture_device_value_snapshot(app, target) {
        Ok(after) => after,
        Err(error) => {
            return Err(rollback_device_value_edit(
                app,
                target,
                &current_before,
                error,
            ));
        }
    };
    if current_before.bit_exact_eq(&after) {
        return Ok(EditOutcome::NoOp);
    }
    if let Err(error) = restore_device_value_snapshot(app, target, &after) {
        return Err(rollback_device_value_edit(
            app,
            target,
            &current_before,
            error,
        ));
    }
    // One entity write reaches every referent (§17.3); edit-through (16.7)
    // still re-preflights the playing song's prebuilt rows.
    invalidate_song_rows_for_edit(app, target.track, target.pattern);

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
        if !matches!(
            history_policy(command),
            HistoryPolicy::Record | HistoryPolicy::Coalesce(_)
        )
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
    // Preset loads follow the sound binding like any other device edit
    // (takes spec 16.4); rule 3 is view-keyed (track-sound spec §2.2.2) —
    // same resolution as `resolve_device_value_target`, never materializing
    // a scene cell.
    let pattern = match app.bound_read_pattern(track) {
        Some(pattern) if !app.track_sound_binding(track).is_scene() => pattern,
        _ => rule_three_device_target(app, track).ok_or(EditError::MissingTrackPattern)?,
    };
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
    // One entity write reaches every referent (§17.3); edit-through (16.7)
    // still re-preflights the playing song's prebuilt rows.
    invalidate_song_rows_for_edit(app, target.track, target.pattern);
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

fn prepare_filter_table_mutation(
    source_path: &std::path::Path,
    reference: &str,
) -> Result<(String, String, std::sync::Arc<crate::effects::filter_table::MagnitudeTable>), EditError> {
    // Baked assets load their payload directly; audio sources analyze under
    // the reference's explicit mode or the recommendation. The stored
    // reference always records what actually happened so undo/redo and
    // reload reproduce the identical table.
    if crate::effects::filter_table_asset::is_asset_path(source_path)
        || crate::effects::filter_table_asset::decode_asset_ref(reference).is_some()
    {
        let asset = crate::effects::filter_table_asset::read_asset(source_path)
            .map_err(EditError::InvalidTarget)?;
        let stem = source_path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .unwrap_or(&asset.meta.name)
            .to_string();
        let stored = crate::effects::filter_table_asset::encode_asset_ref(&stem);
        Ok((stem, stored, std::sync::Arc::new(asset.table)))
    } else {
        let (sample_ref, requested) = crate::effects::filter_table::decode_table_ref(reference);
        let (table, mode) = match requested {
            Some(mode) => (
                crate::effects::filter_table::prepare_table_with_mode(source_path, mode)
                    .map_err(EditError::InvalidTarget)?,
                mode,
            ),
            None => crate::effects::filter_table::prepare_table(source_path)
                .map_err(EditError::InvalidTarget)?,
        };
        let sample_ref = sample_ref.to_string();
        let stored = crate::effects::filter_table::encode_table_ref(&sample_ref, mode);
        Ok((sample_ref, stored, std::sync::Arc::new(table)))
    }
}

pub fn apply_recorded_track_filter_table_mutation(
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
    let (sample_ref, stored, prepared) =
        prepare_filter_table_mutation(source_path, reference)?;
    finish_active_gesture(app);
    let node_id = app.state.pattern.effect_chains
        .get(track)
        .and_then(|chain| chain.get(slot_idx))
        .map(|slot| slot.node_id.load(Ordering::Relaxed) as i32)
        .ok_or_else(|| EditError::InvalidTarget("Track effect slot not found".to_string()))?;
    app.apply_prepared_filter_table_to_node(node_id, prepared, &stored, source_path)
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
        format!("Load Filter Table '{sample_ref}'"),
        None,
        EditPatch::DeviceValues(patch),
        retained_bytes,
    );
    Ok(EditOutcome::Applied(history_move))
}

pub fn apply_recorded_rack_filter_table_mutation(
    app: &mut App,
    track: usize,
    rack_slot: usize,
    effect_slot: usize,
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
        id: DeviceId::RackSlot(app.device_registry.rack_slot(track_id, rack_slot)),
        track,
        pattern,
        slot_idx: Some(rack_slot),
    };
    let before = capture_device_value_snapshot(app, target)?;
    let (sample_ref, stored, prepared) =
        prepare_filter_table_mutation(source_path, reference)?;
    finish_active_gesture(app);
    let node_id = app
        .rack_slot_effect_snapshot(track, rack_slot)
        .map_err(EditError::InvalidTarget)?
        .effect_slots
        .get(effect_slot)
        .map(|slot| slot.node_id as i32)
        .ok_or_else(|| EditError::InvalidTarget("Rack effect slot not found".to_string()))?;
    app.apply_prepared_filter_table_to_node(
        node_id,
        prepared.clone(),
        &stored,
        source_path,
    )
    .map_err(EditError::InvalidTarget)?;
    let mut after = before.clone();
    let DeviceValueSnapshot::RackSlot(values) = &mut after else {
        return Err(EditError::InvalidTarget(
            "Rack Filter Table target did not capture rack values".to_string(),
        ));
    };
    let effect = values.effect_slots.get_mut(effect_slot).ok_or_else(|| {
        EditError::InvalidTarget("Rack effect slot not found in captured values".to_string())
    })?;
    effect.table = Some(stored);
    effect.prepared_table = Some(prepared);
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
        format!("Load rack Filter Table '{sample_ref}'"),
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

/// The pool's stored track params, never the live mirror — the read half of
/// editing a pattern whose lane is currently on loan to a sound binding.
fn capture_pool_track_params(
    app: &App,
    track: usize,
    pattern: crate::sequencer::PatternId,
) -> Result<TrackParamsSnapshot, String> {
    app.state
        .with_pool_pattern(track, pattern, |data| data.track_params.clone())
        .ok_or_else(|| "Track Pattern target no longer exists".to_string())
}

fn rollback_track_params_edit(
    app: &mut App,
    track: usize,
    pattern: crate::sequencer::PatternId,
    before: &TrackParamsSnapshot,
    base_note_before: u32,
    error: EditError,
) -> EditError {
    let current = app.state.capture_pattern_track_params(track, pattern).ok();
    let current_base = app
        .state
        .capture_pattern_instrument_base_note_offset(track, pattern)
        .ok()
        .map(f32::to_bits);
    let rollback = app
        .state
        .restore_pattern_track_params_no_publish(track, pattern, before)
        .and_then(|_| {
            app.state.restore_pattern_instrument_base_note_offset_no_publish(
                track,
                pattern,
                f32::from_bits(base_note_before),
            )
        });
    if let Err(rollback_error) = rollback {
        return EditError::ReplayFailed(format!(
            "{error:?}; restoring track parameters also failed: {rollback_error}"
        ));
    }
    if let (Some(current), Some(current_base)) = (current.as_ref(), current_base) {
        apply_live_track_param_effects(
            app,
            track,
            current,
            before,
            current_base,
            base_note_before,
        );
    } else {
        app.push_track_volume(track);
        app.push_track_pan(track);
        app.push_send_gain(track);
        app.push_track_mute(track);
        app.push_track_solo_mutes();
        app.graph_controller().apply_track_output_routing(track);
        app.graph_controller().apply_track_bus_sends(track);
        app.state.request_accumulator_reset(track);
    }
    app.state.publish_scheduler_snapshot();
    error
}

fn rollback_track_params_batch_edit(
    app: &mut App,
    before: &[(usize, TrackPatternId, TrackParamsSnapshot, u32)],
    error: EditError,
) -> EditError {
    let mut rollback_errors = Vec::new();
    for (track, target, snapshot, base_note) in before {
        let current = app
            .state
            .capture_pattern_track_params(*track, target.pattern)
            .ok();
        let current_base = app
            .state
            .capture_pattern_instrument_base_note_offset(*track, target.pattern)
            .ok()
            .map(f32::to_bits);
        let rollback = app
            .state
            .restore_pattern_track_params_no_publish(*track, target.pattern, snapshot)
            .and_then(|_| {
                app.state.restore_pattern_instrument_base_note_offset_no_publish(
                    *track,
                    target.pattern,
                    f32::from_bits(*base_note),
                )
            });
        if let Err(rollback_error) = rollback {
            rollback_errors.push(format!("track {}: {rollback_error}", track + 1));
            continue;
        }
        if let (Some(current), Some(current_base)) = (current.as_ref(), current_base) {
            apply_live_track_param_effects(
                app,
                *track,
                current,
                snapshot,
                current_base,
                *base_note,
            );
        } else {
            app.push_track_volume(*track);
            app.push_track_pan(*track);
            app.push_send_gain(*track);
            app.push_track_mute(*track);
            app.push_track_solo_mutes();
            app.graph_controller().apply_track_output_routing(*track);
            app.graph_controller().apply_track_bus_sends(*track);
            app.state.request_accumulator_reset(*track);
        }
    }
    app.state.publish_scheduler_snapshot();
    if rollback_errors.is_empty() {
        error
    } else {
        EditError::ReplayFailed(format!(
            "{error:?}; restoring the track-parameter batch also failed: {}",
            rollback_errors.join("; ")
        ))
    }
}

fn capture_transport_authoring(app: &App) -> TransportAuthoringSnapshot {
    TransportAuthoringSnapshot {
        bpm: app.state.transport.bpm.load(Ordering::Relaxed),
        master_volume_bits: app.state.transport.master_volume.load(Ordering::Relaxed),
        reverb_size_bits: app.ui.reverb_size.to_bits(),
        reverb_brightness_bits: app.ui.reverb_brightness.to_bits(),
        reverb_replace_bits: app.ui.reverb_replace.to_bits(),
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
    scene: crate::sequencer::SceneId,
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

pub(crate) fn ensure_coalescing_gesture(app: &mut App, merge_key: &MergeKey) {
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
    // Mixer/track-param edits follow the sound binding exactly like device
    // values (takes spec 16.4): with a take or pinned clip bound, the fader
    // IS the bound source's stored sound, and writing the effective scene
    // pattern instead both loses the edit on the next row push and leaks a
    // take-intended tweak into the scene cell. Structural step-machine
    // fields (timebase/swing) stay live/pattern-owned — the borrow never
    // loads them from the bound source.
    let effective_id = app.state.effective_track_pattern_id(track);
    // What the live surface is a mirror OF right now. The loan is installed
    // on the reactive tick, so the resolved binding and the mirror disagree
    // for the rest of a drain whenever something released the lane (a scene
    // launch / row save-back calls `release_bound_device_state`, "the App
    // rebinds on its next tick"). Trusting the binding alone would take the
    // scene pattern's sound off the released mirror and stamp it onto the
    // take's chunks.
    let mirror_pattern = app
        .state
        .with_project_scenes(|scenes| app.state.mirror_device_pattern_id(track, scenes));
    let bound_pattern = if track_params_command_is_structural(cmd) {
        None
    } else {
        match app.bound_read_pattern(track) {
            Some(pattern)
                if !app.track_sound_binding(track).is_scene()
                    && Some(pattern) != effective_id
                    && mirror_pattern == Some(pattern) =>
            {
                Some(pattern)
            }
            _ => None,
        }
    };
    // Track-param write-through (track-sound spec §2.9): the edit persists
    // into the OWNING entity at edit time, exactly like a device value —
    // never parked in the mirror awaiting a stop save-back that a
    // borrow/release/row-apply may preempt (the `polyphonic` reset, symptom
    // 8). Rule 3's owner is view-keyed, so a bare lane in Seq context and
    // every unclaimed lane in arrangement context reach the track-sound
    // carrier instead of erroring out with no target.
    let pattern_id = match bound_pattern {
        Some(pattern) => pattern,
        None => rule_three_device_target(app, track).ok_or(EditError::MissingTrackPattern)?,
    };
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
    // `capture_pattern_track_params` hands back the LIVE surface whenever the
    // target is the effective pattern — but while the lane is on loan that
    // surface holds the BOUND source's sound, and only the four structural
    // fields still belong to this pattern. Read the pool copy instead (the
    // binding-aware rule its base-note twin already follows), or the
    // whole-snapshot write below would stamp the loaned sound onto the scene
    // pattern with no way back.
    // "The mirror holds the target" is the live surface BY DEFINITION, and
    // since rev 4 the target can be the track-sound carrier — which
    // `capture_pattern_track_params` would read out of the pool (it only
    // shortcuts to live for the EFFECTIVE pattern), turning every
    // write-through into a no-op against an unmoved pool snapshot.
    let mirror_holds_target = mirror_pattern == Some(pattern_id);
    let current_before = if mirror_holds_target && bound_pattern.is_none() {
        app.state.capture_live_track_params(track)
    } else {
        // A BOUND target's structural fields live only in the pool (the
        // borrow never loads them), so it reads the pool even though the
        // mirror shows its device half.
        capture_pool_track_params(app, track, pattern_id)
    }
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
    // The command executed against the LIVE surface. For the effective
    // pattern the live surface IS the pattern; for a bound pool target the
    // live surface is the borrowed mirror of that source, so merge the live
    // values onto the pool snapshot — preserving the structural fields the
    // borrow never loads (a take chunk must keep its MAX_STEPS width).
    let after = if bound_pattern.is_some() {
        match app.state.capture_live_track_params(track) {
            Ok(live) => {
                let mut merged = live;
                merged.num_steps = current_before.num_steps;
                merged.timebase = current_before.timebase.clone();
                merged.swing = current_before.swing;
                merged.swing_resolution = current_before.swing_resolution;
                merged
            }
            Err(error) => return Err(rollback_track_params_edit(
                app,
                track,
                pattern_id,
                &current_before,
                current_base_before,
                EditError::ReplayFailed(error),
            )),
        }
    } else if mirror_holds_target {
        match app.state.capture_live_track_params(track) {
            Ok(after) => after,
            Err(error) => return Err(rollback_track_params_edit(
                app,
                track,
                pattern_id,
                &current_before,
                current_base_before,
                EditError::ReplayFailed(error),
            )),
        }
    } else {
        // Loaned lane: take only the structural fields off the live surface
        // (they are the track's, not the bound source's) and leave the rest
        // of the scene pattern's stored sound exactly as it was.
        match app.state.capture_live_track_params(track) {
            Ok(live) => {
                let mut merged = current_before.clone();
                merged.num_steps = live.num_steps;
                merged.timebase = live.timebase.clone();
                merged.swing = live.swing;
                merged.swing_resolution = live.swing_resolution;
                merged
            }
            Err(error) => return Err(rollback_track_params_edit(
                app,
                track,
                pattern_id,
                &current_before,
                current_base_before,
                EditError::ReplayFailed(error),
            )),
        }
    };
    // Same live-surface rule for the base-note offset: a bound edit executed
    // against the live atomic, not the pool value.
    if bound_pattern.is_some() {
        let live_base = app
            .state
            .pattern
            .instrument_base_note_offsets
            .get(track)
            .map(|bits| bits.load(std::sync::atomic::Ordering::Relaxed));
        let Some(live_base) = live_base else {
            return Err(rollback_track_params_edit(
                app,
                track,
                pattern_id,
                &current_before,
                current_base_before,
                EditError::TrackOutOfRange { track },
            ));
        };
        return finish_track_params_edit(
            app,
            cmd,
            merge_key,
            track,
            pattern_id,
            target,
            current_before,
            current_base_before,
            entry_before,
            entry_base_before,
            after,
            live_base,
            true,
        );
    }
    let base_after = match app
        .state
        .capture_pattern_instrument_base_note_offset(track, pattern_id)
    {
        Ok(base_after) => base_after.to_bits(),
        Err(error) => return Err(rollback_track_params_edit(
            app,
            track,
            pattern_id,
            &current_before,
            current_base_before,
            EditError::ReplayFailed(error),
        )),
    };
    finish_track_params_edit(
        app,
        cmd,
        merge_key,
        track,
        pattern_id,
        target,
        current_before,
        current_base_before,
        entry_before,
        entry_base_before,
        after,
        base_after,
        false,
    )
}

/// Shared tail of a track-params edit: persist `after` to the target pattern
/// (pool for a bound source, live-and-pool for the effective one), fan a
/// bound take's write out to every chunk (takes spec 16.4), and commit or
/// stage exactly one history entry.
#[allow(clippy::too_many_arguments)]
fn finish_track_params_edit(
    app: &mut App,
    cmd: &AppCommand,
    merge_key: Option<MergeKey>,
    track: usize,
    pattern_id: crate::sequencer::PatternId,
    target: TrackPatternId,
    current_before: TrackParamsSnapshot,
    current_base_before: u32,
    entry_before: TrackParamsSnapshot,
    entry_base_before: u32,
    after: TrackParamsSnapshot,
    base_after: u32,
    bound: bool,
) -> Result<EditOutcome, EditError> {
    if track_params_bit_exact_eq(&current_before, &after) && current_base_before == base_after {
        return Ok(EditOutcome::NoOp);
    }

    if let Err(error) = app.state
        .restore_pattern_track_params_no_publish(track, pattern_id, &after)
    {
        return Err(rollback_track_params_edit(
            app,
            track,
            pattern_id,
            &current_before,
            current_base_before,
            EditError::ReplayFailed(error),
        ));
    }
    if let Err(error) = app.state
        .restore_pattern_instrument_base_note_offset_no_publish(
            track,
            pattern_id,
            f32::from_bits(base_after),
        )
    {
        return Err(rollback_track_params_edit(
            app,
            track,
            pattern_id,
            &current_before,
            current_base_before,
            EditError::ReplayFailed(error),
        ));
    }
    if bound {
        // The write landed on the bound source's entities (§17.3); its take
        // siblings share them structurally, so there is nothing to mirror.
        // Edit-through (16.7): a playing song's prebuilt rows cloned this
        // pattern at preflight and would keep the pre-edit sound.
        invalidate_song_rows_for_edit(app, track, pattern_id);
    }
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

/// Track-param commands whose fields the sound-binding borrow never loads
/// (`restore_device_state_to` keeps live num_steps/timebase/swing): these
/// stay routed to the live/effective pattern even with a source bound.
fn track_params_command_is_structural(cmd: &AppCommand) -> bool {
    matches!(
        cmd,
        AppCommand::SetTrackSwing { .. }
            | AppCommand::AdjustTrackSwing { .. }
            | AppCommand::SetTrackSwingResolution { .. }
            | AppCommand::NextTrackSwingResolution { .. }
            | AppCommand::PrevTrackSwingResolution { .. }
            | AppCommand::SetTrackTimebase { .. }
            | AppCommand::NextTrackTimebase { .. }
            | AppCommand::PrevTrackTimebase { .. }
    )
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
        // Same view-keyed owner as the single-command path (§2.9
        // write-through); batches are solo/mute-style mixer gestures, which
        // are never bound-source edits.
        let pattern_id =
            rule_three_device_target(app, track).ok_or(EditError::MissingTrackPattern)?;
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
    let targets = resolved
        .iter()
        .map(|(_, target, _, _)| format!("{}:{}", target.track.0, target.pattern.0))
        .collect::<Vec<_>>()
        .join(",");
    let merge_key = MergeKey::new(format!("track-batch:{label}:{targets}"));
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

    let mut after_states = Vec::with_capacity(resolved.len());
    for (track, target, _, _) in &resolved {
        let after = match app
            .state
            .capture_pattern_track_params(*track, target.pattern)
        {
            Ok(after) => after,
            Err(error) => return Err(rollback_track_params_batch_edit(
                app,
                &resolved,
                EditError::ReplayFailed(error),
            )),
        };
        let base_after = match app
            .state
            .capture_pattern_instrument_base_note_offset(*track, target.pattern)
        {
            Ok(base_after) => base_after.to_bits(),
            Err(error) => return Err(rollback_track_params_batch_edit(
                app,
                &resolved,
                EditError::ReplayFailed(error),
            )),
        };
        after_states.push((after, base_after));
    }

    let mut patches = Vec::with_capacity(resolved.len());
    let mut changed = false;
    for ((track, target, current_before, current_base_before), (after, base_after))
        in resolved.iter().cloned().zip(after_states)
    {
        changed |= !track_params_bit_exact_eq(&current_before, &after)
            || current_base_before != base_after;
        if let Err(error) = app.state
            .restore_pattern_track_params_no_publish(track, target.pattern, &after)
        {
            return Err(rollback_track_params_batch_edit(
                app,
                &resolved,
                EditError::ReplayFailed(error),
            ));
        }
        if let Err(error) = app.state
            .restore_pattern_instrument_base_note_offset_no_publish(
                track,
                target.pattern,
                f32::from_bits(base_after),
            )
        {
            return Err(rollback_track_params_batch_edit(
                app,
                &resolved,
                EditError::ReplayFailed(error),
            ));
        }
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
        AppCommand::SetReverbParam { .. } => "Set reverb parameter",
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
    apply_recorded_pattern_geometry_commands(
        app,
        std::slice::from_ref(cmd),
        pattern_geometry_label(cmd),
    )
}

/// Applies pattern geometry edits to distinct tracks as one atomic history
/// entry. Capturing every target before executing any command keeps a rack-wide
/// resize from becoming a sequence of independently undoable track edits.
fn apply_recorded_pattern_geometry_commands(
    app: &mut App,
    commands: &[AppCommand],
    label: &'static str,
) -> Result<EditOutcome, EditError> {
    struct PendingGeometry {
        command: AppCommand,
        track: usize,
        target: TrackPatternId,
        pattern_id: PatternId,
        steps: Vec<usize>,
        before: Vec<StepCellSnapshot>,
        registry_before: PlockVariantRegistry,
        num_steps_before: usize,
    }

    let mut seen_tracks = HashSet::new();
    let mut pending = Vec::with_capacity(commands.len());
    for command in commands {
        let track = pattern_geometry_track(command).ok_or(EditError::UnsupportedCommand)?;
        if !seen_tracks.insert(track) {
            return Err(EditError::InvalidTarget(format!(
                "pattern geometry batch contains track {track} more than once"
            )));
        }
        let track_id = app
            .track_registry
            .id_at(track)
            .ok_or(EditError::TrackOutOfRange { track })?;
        // Lazily materialize the pattern when the current scene is bare for the
        // track (takes spec 11.1), exactly as a single-track geometry edit does.
        let pattern_id =
            ensure_effective_track_pattern(app, track).ok_or(EditError::MissingTrackPattern)?;
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
        pending.push(PendingGeometry {
            command: command.clone(),
            track,
            target,
            pattern_id,
            steps,
            before,
            registry_before,
            num_steps_before,
        });
    }

    let mut patches = Vec::with_capacity(pending.len());
    for pending in pending {
        super::command::execute_command(app, pending.command);
        app.state
            .reconcile_plock_variant_registry_for_track(pending.track);
        let (after, registry_after) = app
            .state
            .capture_pattern_step_cells(pending.track, pending.pattern_id, &pending.steps)
            .map_err(EditError::ReplayFailed)?;
        let num_steps_after = app
            .state
            .capture_pattern_num_steps(pending.track, pending.pattern_id)
            .map_err(EditError::ReplayFailed)?;
        let cells = pending
            .steps
            .into_iter()
            .zip(pending.before)
            .zip(after)
            .filter_map(|((step, before), after)| {
                (!step_snapshot_bit_exact_eq(&before, &after)).then_some(StepCellDelta {
                    step,
                    before,
                    after,
                })
            })
            .collect::<Vec<_>>();
        if cells.is_empty() && pending.num_steps_before == num_steps_after {
            continue;
        }
        patches.push(PatternGeometryPatch {
            target: pending.target,
            num_steps_before: pending.num_steps_before,
            num_steps_after,
            cells: StepCellsPatch {
                target: pending.target,
                cells,
                variant_registry_before: pending.registry_before,
                variant_registry_after: registry_after,
            },
        });
    }

    if patches.is_empty() {
        return Ok(EditOutcome::NoOp);
    }
    let patch = if patches.len() == 1 {
        EditPatch::PatternGeometry(patches.pop().expect("one geometry patch"))
    } else {
        EditPatch::Composite(
            patches
                .into_iter()
                .map(EditPatch::PatternGeometry)
                .collect(),
        )
    };
    if let Err(error) = replay_patch(app, &patch, ApplyMode::Redo) {
        return match replay_patch(app, &patch, ApplyMode::Undo) {
            Ok(_) => Err(error),
            Err(rollback_error) => Err(EditError::ReplayFailed(format!(
                "{error:?}; rollback also failed: {rollback_error:?}"
            ))),
        };
    }
    let retained_bytes = edit_patch_retained_bytes(&patch);
    finish_active_gesture(app);
    let history_move = app.history.commit(label, None, patch, retained_bytes);
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
    // Lazily materialize the pattern when the current scene is bare for the
    // track (takes spec 11.1): the first edit in a bare scene creates the
    // pattern and lifts the empty-cell silencing.
    let pattern_id =
        ensure_effective_track_pattern(app, track).ok_or(EditError::MissingTrackPattern)?;
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

fn is_macro_configuration_command(cmd: &AppCommand) -> bool {
    matches!(
        cmd,
        AppCommand::MacroCreate { .. }
            | AppCommand::MacroCreateScene { .. }
            | AppCommand::MacroSceneConfig { .. }
            | AppCommand::MacroEnsure { .. }
            | AppCommand::MacroDelete { .. }
            | AppCommand::MacroRename { .. }
            | AppCommand::MacroMapParam { .. }
            | AppCommand::MacroSetRange { .. }
            | AppCommand::MacroSetCurve { .. }
            | AppCommand::MacroUnmap { .. }
    )
}

fn apply_recorded_macro_configuration_command(
    app: &mut App,
    cmd: AppCommand,
) -> Result<EditOutcome, EditError> {
    let before = app.macro_engine.capture_configuration();
    super::command::execute_command(app, cmd);
    let after = app.macro_engine.capture_configuration();
    if before == after {
        return Ok(EditOutcome::NoOp);
    }
    app.state.publish_macro_overrides(app.macro_engine.override_snapshot());
    app.state.publish_scheduler_snapshot();
    let patch = MacroConfigurationPatch { before, after };
    let retained_bytes = patch.retained_bytes();
    let history_move = app.history.commit(
        "Edit macro configuration",
        None,
        EditPatch::MacroConfiguration(patch),
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
        HistoryPolicy::Record if is_macro_configuration_command(&cmd) => {
            apply_recorded_macro_configuration_command(app, cmd)
        }
        HistoryPolicy::Record
            if matches!(
                cmd,
                AppCommand::SetMasterVolume { .. }
                    | AppCommand::AdjustMasterVolume { .. }
                    | AppCommand::SetReverbParam { .. }
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
                    | AppCommand::SetReverbParam { .. }
                    | AppCommand::SetBpm { .. }
            ) =>
        {
            apply_recorded_transport_command(app, &cmd, Some(key))
        }
        HistoryPolicy::Coalesce(_) => Err(EditError::UnsupportedCommand),
    }
}

/// Replay a step patch and drive note edit-through (spec 5.1): the scheduler
/// plays preflight-cloned row snapshots, so publishing the live track alone
/// leaves the edit inaudible for the rest of the song. This seam is the commit
/// tail for a step edit AND the undo/redo replay, so both drive the same
/// refresh. A note edit never moves row layout, so the existing `Refresh`
/// command carries it — `replace_song_in_place`'s identity check passes.
fn replay_step_patch(
    app: &mut App,
    patch: &StepCellsPatch,
    mode: ApplyMode,
) -> Result<MutationEffects, EditError> {
    let (track, effects) = replay_step_patch_cells(app, patch, mode)?;
    invalidate_song_rows_for_edit(app, track, patch.target.pattern);
    Ok(effects)
}

/// The cells half alone, returning the resolved track index. A geometry patch
/// replays this directly so the row refresh happens once, AFTER the pattern
/// length has moved too — a preflight between the two halves would clone a
/// half-applied pattern.
fn replay_step_patch_cells(
    app: &mut App,
    patch: &StepCellsPatch,
    mode: ApplyMode,
) -> Result<(usize, MutationEffects), EditError> {
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
        app.state.publish_scheduler_track(track);
    }
    Ok((track, MutationEffects { publish_scheduler }))
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
    let (_, step_effects) = replay_step_patch_cells(app, &patch.cells, mode)?;
    let geometry_publish = match app
        .state
        .restore_pattern_num_steps_no_publish(track, patch.target.pattern, num_steps)
    {
        Ok(publish) => publish,
        Err(error) => {
            let rollback_mode = match mode {
                ApplyMode::Undo => ApplyMode::Redo,
                ApplyMode::Redo => ApplyMode::Undo,
                ApplyMode::UserEdit | ApplyMode::ProjectLoad => unreachable!(),
            };
            return match replay_step_patch_cells(app, &patch.cells, rollback_mode) {
                Ok(_) => Err(EditError::ReplayFailed(error)),
                Err(rollback_error) => Err(EditError::ReplayFailed(format!(
                    "{error}; restoring pattern cells also failed: {rollback_error:?}"
                ))),
            };
        }
    };
    if geometry_publish && !step_effects.publish_scheduler {
        app.state.publish_scheduler_snapshot();
    }
    // A length change keeps row layout identical too — the song's beat math
    // comes from the arrangement, not the pattern length — so it rides the
    // same content refresh as a note edit (spec 5.1).
    invalidate_song_rows_for_edit(app, track, patch.target.pattern);
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
    if let Err(error) = app.state
        .restore_pattern_instrument_base_note_offset_no_publish(
            track,
            patch.target.pattern,
            f32::from_bits(base_note_bits),
        )
    {
        let rollback = app
            .state
            .restore_pattern_track_params_no_publish(track, patch.target.pattern, &before)
            .and_then(|_| {
                app.state.restore_pattern_instrument_base_note_offset_no_publish(
                    track,
                    patch.target.pattern,
                    f32::from_bits(base_note_before),
                )
            });
        return match rollback {
            Ok(_) => Err(EditError::ReplayFailed(error)),
            Err(rollback_error) => Err(EditError::ReplayFailed(format!(
                "{error}; restoring track parameters also failed: {rollback_error}"
            ))),
        };
    }
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
    } else {
        // A pool target (the pinned loop-bar resize, clip-edit-target spec
        // 5): a playing song whose rows cloned this pattern at preflight
        // must re-preflight, exactly like the do-path.
        invalidate_song_rows_for_edit(app, track, patch.target.pattern);
        // The restore wrote the pattern's entities (§17.3), which its take
        // siblings share structurally — no mirroring needed.
        // The live mirror may be borrowing this pattern (a bound take or
        // clip): drop the loan so the next binding sync re-borrows the
        // restored values and re-pushes them where audible.
        let borrowing = app
            .loaded_sound_binding
            .get(track)
            .copied()
            .flatten()
            .is_some_and(|source| {
                app.bound_source_patterns(source, track)
                    .contains(&patch.target.pattern)
            });
        if borrowing {
            app.loaded_sound_binding[track] = None;
            app.state.release_bound_track_device_state(track);
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
    let rollback_mode = match mode {
        ApplyMode::Undo => ApplyMode::Redo,
        ApplyMode::Redo => ApplyMode::Undo,
        ApplyMode::UserEdit | ApplyMode::ProjectLoad => {
            return Err(EditError::ReplayFailed(
                "track-parameter batch replay requires undo or redo mode".to_string(),
            ));
        }
    };
    let mut publish_scheduler = false;
    let mut applied = Vec::new();
    for (index, track_patch) in patch.tracks.iter().enumerate() {
        match replay_track_params_patch(app, track_patch, mode, false) {
            Ok(effects) => {
                publish_scheduler |= effects.publish_scheduler;
                applied.push(index);
            }
            Err(error) => {
                for applied_index in applied.into_iter().rev() {
                    if let Err(rollback_error) = replay_track_params_patch(
                        app,
                        &patch.tracks[applied_index],
                        rollback_mode,
                        false,
                    ) {
                        return Err(EditError::ReplayFailed(format!(
                            "track-parameter batch replay failed ({error:?}); rollback also failed ({rollback_error:?})"
                        )));
                    }
                }
                return Err(error);
            }
        }
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
    let scene_idx = app.state.scene_index(patch.scene)
        .ok_or_else(|| EditError::ReplayFailed(
            "bus-effect scene no longer exists".to_string(),
        ))?;
    let scene = repository.get_mut(scene_idx)
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

    if app.state.current_scene_index() == scene_idx {
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
        if let (Some(reference), Some(table)) = (&values.table, &values.prepared_table) {
            app.restore_prepared_bus_filter_table(bus_idx, slot, reference, table.clone())
                .map_err(EditError::ReplayFailed)?;
        }
        app.push_bus_effect_slot_defaults(bus_idx, slot);
        app.publish_bus_effect_runtime();
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
                if let (Some(reference), Some(table)) = (&values.table, &values.prepared_table) {
                    app.restore_prepared_track_filter_table(
                        target.track,
                        slot_idx,
                        reference,
                        table.clone(),
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
                    if let (Some(reference), Some(table)) =
                        (&effect.table, &effect.prepared_table)
                    {
                        app.restore_prepared_rack_filter_table(
                            target.track,
                            slot_idx,
                            effect_slot,
                            reference,
                            table.clone(),
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
    // One entity write reaches every referent (§17.3); the playing song's
    // prebuilt rows still need a re-preflight (16.7).
    invalidate_song_rows_for_edit(app, target.track, target.pattern);
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
    if before.reverb_size_bits != target.reverb_size_bits {
        app.set_reverb_param_unrecorded(0, f32::from_bits(target.reverb_size_bits));
    }
    if before.reverb_brightness_bits != target.reverb_brightness_bits {
        app.set_reverb_param_unrecorded(1, f32::from_bits(target.reverb_brightness_bits));
    }
    if before.reverb_replace_bits != target.reverb_replace_bits {
        app.set_reverb_param_unrecorded(2, f32::from_bits(target.reverb_replace_bits));
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
        EditPatch::Composite(patches) => replay_composite_patch(app, patches, mode),
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
                app.device_registry.clear_track(patch.track);
                app.graph_controller().delete_track(track)
                    .map_err(EditError::ReplayFailed)?;
                // Same group bookkeeping as the track-deletion redo arm: drop
                // the member (and the rack pad it backed), shift the member
                // indices behind it, and republish the choke runtime table.
                app.remap_groups_after_track_delete(track);
                Ok(())
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
        EditPatch::Arrangement(patch) => {
            let target = match mode {
                ApplyMode::Undo => &patch.before,
                ApplyMode::Redo => &patch.after,
                ApplyMode::UserEdit | ApplyMode::ProjectLoad => {
                    return Err(EditError::ReplayFailed(
                        "arrangement replay requires undo or redo mode".to_string(),
                    ));
                }
            };
            app.restore_committed_arrangement_state(target)
                .map_err(EditError::ReplayFailed)
        }
        EditPatch::BusGroupStructure(patch) => {
            let target = match mode {
                ApplyMode::Undo => &patch.before,
                ApplyMode::Redo => &patch.after,
                ApplyMode::UserEdit | ApplyMode::ProjectLoad => {
                    return Err(EditError::ReplayFailed(
                        "bus/group-structure replay requires undo or redo mode".to_string(),
                    ));
                }
            };
            app.restore_bus_group_structure_state(target)
                .map_err(EditError::ReplayFailed)
        }
        EditPatch::MacroConfiguration(patch) => {
            let target = match mode {
                ApplyMode::Undo => &patch.before,
                ApplyMode::Redo => &patch.after,
                ApplyMode::UserEdit | ApplyMode::ProjectLoad => {
                    return Err(EditError::ReplayFailed(
                        "macro-configuration replay requires undo or redo mode".to_string(),
                    ));
                }
            };
            app.macro_engine.restore_configuration(target);
            app.state.publish_macro_overrides(app.macro_engine.override_snapshot());
            app.push_all_restored_defaults();
            app.state.publish_scheduler_snapshot();
            Ok(())
        }
        EditPatch::TransportParams(patch) => {
            replay_transport_params_patch(app, patch, mode, true).map(|_| ())
        }
    }
}

fn replay_composite_patch(
    app: &mut App,
    patches: &[EditPatch],
    mode: ApplyMode,
) -> Result<(), EditError> {
    let indices: Vec<usize> = match mode {
        ApplyMode::Undo => (0..patches.len()).rev().collect(),
        ApplyMode::Redo => (0..patches.len()).collect(),
        ApplyMode::UserEdit | ApplyMode::ProjectLoad => {
            return Err(EditError::ReplayFailed(
                "compound replay requires undo or redo mode".to_string(),
            ));
        }
    };
    let rollback_mode = match mode {
        ApplyMode::Undo => ApplyMode::Redo,
        ApplyMode::Redo => ApplyMode::Undo,
        ApplyMode::UserEdit | ApplyMode::ProjectLoad => unreachable!(),
    };
    let mut applied = Vec::new();
    for index in indices {
        if let Err(error) = replay_patch(app, &patches[index], mode) {
            for applied_index in applied.into_iter().rev() {
                if let Err(rollback_error) = replay_patch(app, &patches[applied_index], rollback_mode) {
                    return Err(EditError::ReplayFailed(format!(
                        "compound replay failed ({error:?}); rollback also failed ({rollback_error:?})"
                    )));
                }
            }
            return Err(error);
        }
        applied.push(index);
    }
    Ok(())
}

fn pending_gesture_publishes_scheduler(patch: &EditPatch) -> bool {
    match patch {
        EditPatch::Composite(patches) => patches.iter().any(pending_gesture_publishes_scheduler),
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
        // The arrangement's compiled song has no scheduler runtime.
        EditPatch::Arrangement(_) => false,
        EditPatch::BusGroupStructure(_) => true,
        EditPatch::MacroConfiguration(_) => true,
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
    if finished {
        if let Some((track, pattern)) = app.pending_song_row_invalidation.take() {
            app.invalidate_song_rows_for_pattern(track, pattern);
        }
    }
    finished
}

pub fn squash_history_since(
    app: &mut App,
    checkpoint: usize,
    label: impl Into<String>,
) -> Option<HistoryMove> {
    finish_active_gesture(app);
    let count = app.history.undo_len().checked_sub(checkpoint)?;
    if count < 2 {
        return None;
    }
    let patches = app.history.recent_undo_patches(count)?
        .into_iter().cloned().collect::<Vec<_>>();
    let retained_bytes = std::mem::size_of::<Vec<EditPatch>>()
        + patches.iter().map(edit_patch_retained_bytes).sum::<usize>();
    app.history.squash_recent(
        count,
        label.into(),
        EditPatch::Composite(patches),
        retained_bytes,
    )
}

pub fn rollback_history_to(
    app: &mut App,
    checkpoint: super::history::UndoManager<EditPatch>,
) -> Result<(), EditError> {
    finish_active_gesture(app);
    let target_len = checkpoint.undo_len();
    while app.history.undo_len() > target_len {
        match undo(app) {
            HistoryReplay::Applied(_) => {}
            HistoryReplay::Unavailable => {
                return Err(EditError::ReplayFailed(
                    "authoring transaction history disappeared during rollback".to_string(),
                ));
            }
            HistoryReplay::Failed(error) => return Err(error),
        }
    }
    app.history = checkpoint;
    Ok(())
}

fn edit_patch_retained_bytes(patch: &EditPatch) -> usize {
    match patch {
        EditPatch::Composite(patches) => std::mem::size_of::<Vec<EditPatch>>()
            + patches.iter().map(edit_patch_retained_bytes).sum::<usize>(),
        EditPatch::StepCells(patch) => patch.retained_bytes(),
        EditPatch::PatternGeometry(patch) => patch.retained_bytes(),
        EditPatch::TrackParams(patch) => patch.retained_bytes(),
        EditPatch::TrackParamsBatch(patch) => patch.retained_bytes(),
        EditPatch::BusMixer(patch) => patch.retained_bytes(),
        EditPatch::DeviceValues(patch) => patch.retained_bytes(),
        EditPatch::InstrumentBinding(patch) => patch.retained_bytes(),
        EditPatch::EffectChain(patch) => patch.retained_bytes(),
        EditPatch::BusEffectChain(patch) => patch.retained_bytes(),
        EditPatch::BusEffectValues(patch) => patch.retained_bytes(),
        EditPatch::RackEffectChain(patch) => patch.retained_bytes(),
        EditPatch::MidiFxChain(patch) => patch.retained_bytes(),
        EditPatch::RackSlotStructure(patch) => patch.retained_bytes(),
        EditPatch::TrackCreation(patch) => patch.retained_bytes(),
        EditPatch::TrackDeletion(patch) => patch.retained_bytes(),
        EditPatch::TrackPresentation(patch) => patch.retained_bytes(),
        EditPatch::SceneStructure(patch) => patch.retained_bytes(),
        EditPatch::Arrangement(patch) => patch.retained_bytes(),
        EditPatch::BusGroupStructure(patch) => patch.retained_bytes(),
        EditPatch::MacroConfiguration(patch) => patch.retained_bytes(),
        EditPatch::TransportParams(patch) => patch.retained_bytes(),
    }
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
    if app.recording_history.is_some() {
        app.ui.recording = false;
        if let Err(error) = app.finish_recording_take_history() {
            return HistoryReplay::Failed(EditError::ReplayFailed(error));
        }
    }
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
    if app.recording_history.is_some() {
        app.ui.recording = false;
        if let Err(error) = app.finish_recording_take_history() {
            return HistoryReplay::Failed(EditError::ReplayFailed(error));
        }
    }
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
    matches!(
        patch,
        EditPatch::TrackCreation(_)
            | EditPatch::TrackDeletion(_)
            | EditPatch::BusGroupStructure(_)
    ) || matches!(patch, EditPatch::Composite(patches) if patches.iter().any(structural_track_patch))
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
        EditPatch::Composite(_) => {
            replay_patch(app, &patch, ApplyMode::Undo)?;
        }
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
        EditPatch::Arrangement(_) => {
            replay_patch(app, &patch, ApplyMode::Undo)?;
        }
        EditPatch::BusGroupStructure(_) => {
            replay_patch(app, &patch, ApplyMode::Undo)?;
        }
        EditPatch::MacroConfiguration(_) => {
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
    use std::ffi::CString;
    use std::sync::{Arc, Mutex};

    use super::*;
    use crate::audiograph::LiveGraphPtr;
    use crate::recorder::MasterRecorder;
    use crate::sequencer::{
        default_empty_effect_chain, InstrumentType, PatternSnapshot, SequencerState,
        SwingResolution, Timebase, DRUM_RACK_TOTAL_PAD_NOTES,
    };
    use crate::app::AudioBuses;

    #[test]
    fn group_conversion_pad_order_starts_at_c4_and_wraps_at_the_maximum() {
        let notes = group_conversion_pad_notes().collect::<Vec<_>>();
        assert_eq!(notes.len(), DRUM_RACK_TOTAL_PAD_NOTES);
        assert_eq!(notes[0], 0, "the first adopted member lands on C4");
        assert_eq!(notes[DRUM_RACK_LAST_PAD_NOTE as usize], DRUM_RACK_LAST_PAD_NOTE);
        assert_eq!(
            notes[DRUM_RACK_LAST_PAD_NOTE as usize + 1],
            DRUM_RACK_FIRST_PAD_NOTE,
            "the note after the top of the domain wraps to C1",
        );
        assert_eq!(notes.last(), Some(&-1));
    }

    struct TestLiveGraph(LiveGraphPtr);

    impl TestLiveGraph {
        fn new(label: &str) -> Self {
            crate::audiograph::initialize_engine_for_test(64, 44_100);
            let label = CString::new(label).unwrap();
            let ptr = unsafe {
                crate::audiograph::create_live_graph(32, 64, label.as_ptr(), 2)
            };
            assert!(!ptr.is_null());
            Self(LiveGraphPtr(ptr))
        }
    }

    impl Drop for TestLiveGraph {
        fn drop(&mut self) {
            unsafe { crate::audiograph::destroy_live_graph(self.0.0) };
        }
    }

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
                bus_effect_runtime: Arc::new(Mutex::new(Arc::new(Vec::new()))),
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

    /// Regression: a newly added track has a pattern only in the scene it
    /// was born in (takes spec 11.1). Switching to another scene must show
    /// an EMPTY step grid (not the previous scene's notes), and the first
    /// step edit there must materialize the scene's pattern, lift the
    /// empty-cell silencing, and leave the original scene untouched.
    #[test]
    fn bare_scene_presents_empty_grid_and_first_edit_materializes() {
        let state = SequencerState::new(
            2,
            vec![default_empty_effect_chain(), default_empty_effect_chain()],
        );
        state.replace_pattern_repository(
            vec![
                PatternSnapshot::new_default(2, &[]),
                PatternSnapshot::new_default(2, &[]),
            ],
            0,
        );
        // Shape of a track added while on scene 0: scene 1's cell is bare.
        state.with_scenes_mut(|scenes| {
            if let Some(id) = scenes.scenes[1].cells[1].take() {
                scenes.track_pools[1].remove(id);
            }
        });
        let mut app = test_app(state);
        app.tracks = vec!["Track 1".to_string(), "Track 2".to_string()];
        app.track_registry =
            crate::sequencer::TrackRegistry::for_legacy_track_count(2).unwrap();
        app.graph.track_buffer_ids = vec![-1, -1];
        app.graph.track_sample_rates = vec![44_100, 44_100];
        app.graph.track_instrument_types = vec![InstrumentType::Sampler, InstrumentType::Sampler];

        // Author a step on track 1 while on scene 0.
        try_apply_command(&mut app, AppCommand::ToggleStep { track: 1, step: 0 })
            .expect("edit in the born scene");
        assert!(app.state.capture_step_snapshot(1, 0).active);

        // Switch to scene 1 (bare for track 1): empty grid, silenced lane,
        // no effective pattern — scene 0's notes must not leak through.
        app.state
            .launch_scene(
                1,
                2,
                &app.graph.track_buffer_ids,
                &app.graph.track_sample_rates,
                &app.tracks,
                &app.graph.track_instrument_types,
            )
            .expect("launch scene 1");
        assert!(!app.state.capture_step_snapshot(1, 0).active, "no leaked notes");
        assert!(app.state.is_scene_silenced(1));
        assert!(app.state.effective_track_pattern_id(1).is_none());

        // First step edit in the bare scene materializes the pattern into
        // scene 1's cell and lifts the silencing.
        try_apply_command(&mut app, AppCommand::ToggleStep { track: 1, step: 4 })
            .expect("first edit in the bare scene");
        let materialized = app
            .state
            .effective_track_pattern_id(1)
            .expect("pattern materialized");
        app.state.with_scenes_mut(|scenes| {
            assert_eq!(scenes.scenes[1].cells[1], Some(materialized));
            assert_ne!(scenes.scenes[0].cells[1], Some(materialized));
        });
        assert!(!app.state.is_scene_silenced(1));
        assert!(app.state.capture_step_snapshot(1, 4).active);
        assert!(!app.state.capture_step_snapshot(1, 0).active);

        // Back to scene 0: the original pattern is intact and scene 1's
        // edit stayed in scene 1.
        app.state
            .launch_scene(
                0,
                2,
                &app.graph.track_buffer_ids,
                &app.graph.track_sample_rates,
                &app.tracks,
                &app.graph.track_instrument_types,
            )
            .expect("launch scene 0");
        assert!(app.state.capture_step_snapshot(1, 0).active);
        assert!(!app.state.capture_step_snapshot(1, 4).active);
    }

    #[test]
    fn track_bus_send_plock_command_records_live_and_pattern_step_state() {
        let state = SequencerState::new(1, vec![default_empty_effect_chain()]);
        let mut app = test_app(state);
        app.tracks = vec!["Track 1".to_string()];
        app.track_registry = crate::sequencer::TrackRegistry::for_legacy_track_count(1).unwrap();
        let destination = BusId::DEFAULT_A;

        let outcome = try_apply_command(&mut app, AppCommand::SetTrackBusSendPlock {
            track: 0,
            step: 5,
            destination,
            value: Some(0.72),
        }).expect("record bus-send p-lock command");

        assert!(matches!(outcome, EditOutcome::Applied(_)));
        assert_eq!(app.state.pattern.track_send_plocks[0].get(5, destination), Some(0.72));
        assert_eq!(app.state.with_scene_track_pattern(0, 0, |pattern| {
            pattern.track_send_plock_snapshot[5][0].amount
        }), Some(0.72));
    }

    #[test]
    fn track_bus_send_edits_are_scoped_to_the_active_scene_pattern() {
        let state = SequencerState::new(1, vec![default_empty_effect_chain()]);
        state.replace_pattern_repository(
            vec![PatternSnapshot::new_default(1, &[]), PatternSnapshot::new_default(1, &[])],
            0,
        );
        PatternSnapshot::new_default(1, &[]).restore(&state);
        let mut app = test_app(state);
        app.tracks = vec!["Track 1".to_string()];
        app.track_registry = crate::sequencer::TrackRegistry::for_legacy_track_count(1).unwrap();
        app.graph.track_buffer_ids = vec![-1];
        app.graph.track_sample_rates = vec![44_100];
        app.graph.track_instrument_types = vec![InstrumentType::Sampler];
        let destination = BusId::DEFAULT_A;

        try_apply_command(&mut app, AppCommand::SetTrackSends {
            track: 0,
            sends: vec![crate::sequencer::TrackSendSnapshot { destination, amount: 0.15 }],
        }).expect("edit scene 1 send");
        app.state.launch_scene(
            1, 1, &app.graph.track_buffer_ids, &app.graph.track_sample_rates,
            &app.tracks, &app.graph.track_instrument_types,
        ).expect("launch scene 2");
        try_apply_command(&mut app, AppCommand::SetTrackSends {
            track: 0,
            sends: vec![crate::sequencer::TrackSendSnapshot { destination, amount: 0.9 }],
        }).expect("edit scene 2 send");
        app.state.launch_scene(
            0, 1, &app.graph.track_buffer_ids, &app.graph.track_sample_rates,
            &app.tracks, &app.graph.track_instrument_types,
        ).expect("return to scene 1");

        assert_eq!(app.state.pattern.track_params[0].sends()[0].amount, 0.15);
        assert_eq!(app.state.with_scene_track_pattern(1, 0, |pattern| {
            pattern.track_params.sends[0].amount
        }), Some(0.9));
    }

    /// Track-sound spec §2.2: loading an instrument preset on a bare lane
    /// resolves the TRACK SOUND — it neither fails with
    /// `MissingTrackPattern` nor mints a scene cell (the old lazy
    /// materialization, spec §1.1).
    #[test]
    fn instrument_preset_load_on_a_bare_lane_edits_the_track_sound_without_minting() {
        let state = SequencerState::new(
            2,
            vec![default_empty_effect_chain(), default_empty_effect_chain()],
        );
        state.replace_pattern_repository(
            vec![
                PatternSnapshot::new_default(2, &[]),
                PatternSnapshot::new_default(2, &[]),
            ],
            0,
        );
        // Shape of a track added while on scene 0: scene 1's cell is bare.
        state.with_scenes_mut(|scenes| {
            if let Some(id) = scenes.scenes[1].cells[1].take() {
                scenes.track_pools[1].remove(id);
            }
        });
        let mut app = test_app(state);
        app.tracks = vec!["Track 1".to_string(), "Track 2".to_string()];
        app.track_registry =
            crate::sequencer::TrackRegistry::for_legacy_track_count(2).unwrap();
        app.graph.track_buffer_ids = vec![-1, -1];
        app.graph.track_sample_rates = vec![44_100, 44_100];
        app.graph.track_instrument_types = vec![InstrumentType::Sampler, InstrumentType::Sampler];

        app.state
            .launch_scene(
                1,
                2,
                &app.graph.track_buffer_ids,
                &app.graph.track_sample_rates,
                &app.tracks,
                &app.graph.track_instrument_types,
            )
            .expect("launch scene 1");
        assert!(app.state.effective_track_pattern_id(1).is_none());

        apply_recorded_instrument_values_mutation(&mut app, 1, "Load preset", |_app| Ok(()))
            .expect("the preset load resolves the track sound on a bare lane");
        assert!(
            app.state.effective_track_pattern_id(1).is_none(),
            "a device edit never materializes a scene cell (track-sound spec §2.2)"
        );
        assert!(
            app.state.track_sound_pattern_id(1).is_some(),
            "the track sound is the resolved write target"
        );
    }

    fn configure_test_sampler_project(app: &mut App, sample_path: &str) {
        app.tracks[0].clear();
        let sample_path = std::path::PathBuf::from(sample_path);
        app.sampler_paths = vec![Some(sample_path.clone())];
        app.register_loaded_sample_path("", -1, sample_path);
        app.graph.track_buffer_ids = vec![-1];
        app.graph.track_sample_rates = vec![44_100];
        app.graph.track_instrument_types = vec![InstrumentType::Sampler];
        app.graph.track_instrument_run_modes = vec![
            crate::sequencer::CustomInstrumentRunMode::Instrument,
        ];
        app.graph.track_engine_ids = vec![None];
        app.graph.effect_descriptors = vec![
            crate::effects::EffectDescriptor::default_full_chain(),
        ];
        app.graph.instrument_descriptors = vec![
            crate::effects::EffectDescriptor::builtin_sampler(),
        ];
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
    fn scene_cell_assignment_keeps_restored_pattern_sample_identity() {
        let state = SequencerState::new(1, vec![default_empty_effect_chain()]);
        let mut first = PatternSnapshot::new_default(1, &[]);
        first.sample_ids[0] = (7, "sample-one".to_string(), 44_100);
        let mut second = PatternSnapshot::new_default(1, &[]);
        second.sample_ids[0] = (9, "sample-two".to_string(), 48_000);
        state.replace_pattern_repository(vec![first, second], 0);
        state.restore_current_pattern_from_repository().unwrap();
        let shared = state.scene_track_pattern_id(1, 0).unwrap();

        let mut app = test_app(state);
        configure_test_sampler_project(&mut app, "/tmp/sample-one.wav");
        app.graph.track_buffer_ids = vec![7];
        app.tracks[0] = "sample-one".to_string();

        // Mirrors the "set-scene-cell" handler: the live sample arrays must be
        // updated inside the mutation, before the wrapper re-snapshots live
        // state into the newly effective pattern.
        app.apply_recorded_scene_structure_mutation("Assign scene cell", |app| {
            if !app.state.set_scene_cell(
                0,
                0,
                shared,
                1,
                &app.graph.track_buffer_ids,
                &app.graph.track_sample_rates,
                &app.tracks,
                &app.graph.track_instrument_types,
            ) {
                return Err("set_scene_cell failed".to_string());
            }
            let sample_ids = app.state.effective_pattern_sample_ids(1);
            app.graph_controller().apply_sample_ids(&sample_ids);
            Ok(())
        })
        .unwrap();

        assert_eq!(
            app.state.effective_pattern_sample_ids(1)[0],
            (9, "sample-two".to_string(), 48_000),
            "assigning a pattern into the current scene must not clobber its sample"
        );
        assert_eq!(app.graph.track_buffer_ids[0], 9);
        assert_eq!(app.tracks[0], "sample-two");
    }

    #[test]
    fn recorded_track_rename_trims_rejects_empty_and_round_trips_authorship() {
        let mut app = test_app(SequencerState::new(
            1,
            vec![default_empty_effect_chain()],
        ));
        app.normalize_track_name_authorship();
        let original = app.tracks[0].clone();

        assert!(matches!(
            app.apply_recorded_track_name(0, "  Aurora Layers  "),
            Ok(EditOutcome::Applied(_))
        ));
        assert_eq!(app.tracks[0], "Aurora Layers");
        assert!(app.track_name_user_authored[0]);
        assert_eq!(app.apply_recorded_track_name(0, "   "), Ok(EditOutcome::NoOp));
        assert_eq!(app.tracks[0], "Aurora Layers");

        assert!(matches!(undo(&mut app), HistoryReplay::Applied(_)));
        assert_eq!(app.tracks[0], original);
        assert!(!app.track_name_user_authored[0]);
        assert!(matches!(redo(&mut app), HistoryReplay::Applied(_)));
        assert_eq!(app.tracks[0], "Aurora Layers");
        assert!(app.track_name_user_authored[0]);
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
    fn macro_configuration_history_restores_ids_without_reusing_them_or_live_values() {
        let mut app = test_app(SequencerState::new(
            1,
            vec![default_empty_effect_chain()],
        ));
        assert!(matches!(
            try_apply_command(
                &mut app,
                AppCommand::MacroCreate { name: "Depth".to_string() },
            ),
            Ok(EditOutcome::Applied(_))
        ));
        let macro_id = app.macro_engine.macros()[0].id;
        assert_eq!(macro_id, 1);
        app.set_macro_value(macro_id, 0.72);

        assert!(matches!(undo(&mut app), HistoryReplay::Applied(_)));
        assert!(app.macro_engine.macros().is_empty());
        assert_eq!(app.macro_engine.next_id(), 2);
        assert!(matches!(redo(&mut app), HistoryReplay::Applied(_)));
        assert_eq!(app.macro_engine.macros()[0].id, macro_id);

        app.set_macro_value(macro_id, 0.41);
        assert!(matches!(
            try_apply_command(
                &mut app,
                AppCommand::MacroRename {
                    id: macro_id,
                    name: "Renamed".to_string(),
                },
            ),
            Ok(EditOutcome::Applied(_))
        ));
        assert!(matches!(undo(&mut app), HistoryReplay::Applied(_)));
        assert_eq!(app.macro_engine.macros()[0].name, "Depth");
        assert_eq!(app.macro_engine.macros()[0].value, 0.41);
        assert!(matches!(redo(&mut app), HistoryReplay::Applied(_)));
        assert_eq!(app.macro_engine.macros()[0].name, "Renamed");
        assert_eq!(app.macro_engine.macros()[0].value, 0.41);
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

    // eseq-ur4: the effect-slot twin of
    // `sampler_range_edit_survives_stale_pattern_pool_descriptor`. An older
    // project's pool copy of an effect slot can hold fewer params than the live
    // descriptor. Device-value edits used to fail forever with "device scalar
    // descriptor changed while replaying history", leaving the stale layout in
    // the pool to misroute params into device state slots later.
    #[test]
    fn effect_param_edit_survives_stale_pattern_pool_descriptor() {
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

        // Simulate the old-project pool copy: fewer params and no layout.
        app.state.with_scenes_mut(|scenes| {
            assert!(scenes.track_pools[0].edit(pattern, |data| {
                let stale_params = (data.effect_slots[0].num_params as usize).saturating_sub(2);
                data.effect_slots[0].num_params = stale_params as u32;
                data.effect_slots[0].defaults.truncate(stale_params);
                data.effect_slots[0].param_node_indices.clear();
            }));
        });
        app.state.publish_scheduler_snapshot();

        let outcome = apply_coalesced_device_value_batch(
            &mut app,
            &[AppCommand::SetEffectParam {
                track: 0,
                slot_idx: 0,
                param_idx: 2,
                value: 900.0,
            }],
            "effect-cutoff",
            "Set effect cutoff",
        );
        assert!(matches!(outcome, Ok(EditOutcome::Applied(_))), "{outcome:?}");
        finish_active_gesture(&mut app);

        // The pool copy must be healed back to the live layout.
        let live_num_params = app.state.pattern.effect_chains[0][0]
            .num_params
            .load(Ordering::Relaxed);
        app.state.with_scenes_mut(|scenes| {
            let data = scenes.track_pools[0].get(pattern).unwrap();
            assert_eq!(data.effect_slots[0].num_params, live_num_params);
            assert_eq!(
                data.effect_slots[0].param_node_indices.len(),
                live_num_params as usize
            );
            assert_eq!(data.effect_slots[0].defaults.get(2).copied(), Some(900.0));
        });
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
    fn sampler_range_updates_coalesce_into_one_two_parameter_gesture() {
        let state = SequencerState::new(1, vec![default_empty_effect_chain()]);
        let descriptor = EffectDescriptor::builtin_sampler();
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

        for (start, end) in [(0.01, 0.99), (0.02, 0.98), (0.03, 0.97)] {
            let outcome = apply_coalesced_device_value_batch(
                &mut app,
                &[
                    AppCommand::SetInstrumentParam {
                        track: 0,
                        param_idx: 2,
                        value: start,
                    },
                    AppCommand::SetInstrumentParam {
                        track: 0,
                        param_idx: 3,
                        value: end,
                    },
                ],
                "sampler-range",
                "Set sampler range",
            );
            assert!(matches!(outcome, Ok(EditOutcome::Applied(_))), "{outcome:?}");
        }
        assert_eq!(app.history.undo_len(), 0);
        finish_active_gesture(&mut app);
        assert_eq!(app.history.undo_len(), 1);

        assert!(matches!(undo(&mut app), HistoryReplay::Applied(_)));
        assert!(app
            .state
            .capture_pattern_instrument_device_values(0, pattern)
            .unwrap()
            .bit_exact_eq(&before));
    }

    #[test]
    fn sampler_range_drag_reaches_scheduler_snapshot() {
        let state = SequencerState::new(1, vec![default_empty_effect_chain()]);
        let descriptor = EffectDescriptor::builtin_sampler();
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
        app.state.publish_scheduler_snapshot();

        for (start, end) in [(0.1, 0.9), (0.2, 0.8), (0.25, 0.75)] {
            let outcome = apply_coalesced_device_value_batch(
                &mut app,
                &[
                    AppCommand::SetInstrumentParam {
                        track: 0,
                        param_idx: 2,
                        value: start,
                    },
                    AppCommand::SetInstrumentParam {
                        track: 0,
                        param_idx: 3,
                        value: end,
                    },
                ],
                "sampler-range",
                "Set sampler range",
            );
            assert!(matches!(outcome, Ok(EditOutcome::Applied(_))), "{outcome:?}");
        }
        finish_active_gesture(&mut app);
        let snapshot = app.state.latest_scheduler_snapshot();
        let slot = &snapshot.tracks[0].instrument_slot;
        assert_eq!(slot.defaults.get(2).copied(), Some(0.25));
        assert_eq!(slot.defaults.get(3).copied(), Some(0.75));
    }

    #[test]
    fn sampler_range_edit_survives_stale_pattern_pool_descriptor() {
        // Older saved projects can hold a pool copy of the instrument slot
        // whose param layout predates the current descriptor (the sampler
        // grew params). Device-value edits used to fail with "device scalar
        // descriptor changed while replaying history" and skip the scheduler
        // publish, leaving audio on stale start/end until transport restart.
        let state = SequencerState::new(1, vec![default_empty_effect_chain()]);
        let descriptor = EffectDescriptor::builtin_sampler();
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

        // Simulate the old-project pool copy: fewer params than the live slot.
        app.state.with_scenes_mut(|scenes| {
            assert!(scenes.track_pools[0].edit(pattern, |data| {
                let stale_params =
                    (data.instrument_slot.num_params as usize).saturating_sub(3);
                data.instrument_slot.num_params = stale_params as u32;
                data.instrument_slot.defaults.truncate(stale_params);
            }));
        });
        app.state.publish_scheduler_snapshot();

        let outcome = apply_coalesced_device_value_batch(
            &mut app,
            &[
                AppCommand::SetInstrumentParam {
                    track: 0,
                    param_idx: 2,
                    value: 0.25,
                },
                AppCommand::SetInstrumentParam {
                    track: 0,
                    param_idx: 3,
                    value: 0.75,
                },
            ],
            "sampler-range",
            "Set sampler range",
        );
        assert!(matches!(outcome, Ok(EditOutcome::Applied(_))), "{outcome:?}");
        finish_active_gesture(&mut app);

        let snapshot = app.state.latest_scheduler_snapshot();
        let slot = &snapshot.tracks[0].instrument_slot;
        assert_eq!(slot.defaults.get(2).copied(), Some(0.25));
        assert_eq!(slot.defaults.get(3).copied(), Some(0.75));

        // The pool copy must be healed to the live layout.
        let live_num_params = app.state.pattern.instrument_slots[0]
            .num_params
            .load(Ordering::Relaxed);
        app.state.with_scenes_mut(|scenes| {
            let data = scenes.track_pools[0].get(pattern).unwrap();
            assert_eq!(data.instrument_slot.num_params, live_num_params);
            assert_eq!(data.instrument_slot.defaults.get(2).copied(), Some(0.25));
        });
    }

    #[test]
    fn sampler_range_key_lock_updates_coalesce_into_one_gesture() {
        let state = SequencerState::new(1, vec![default_empty_effect_chain()]);
        let descriptor = EffectDescriptor::builtin_sampler();
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

        for (start, end) in [(0.01, 0.99), (0.02, 0.98), (0.03, 0.97)] {
            let outcome = apply_coalesced_device_value_batch(
                &mut app,
                &[
                    AppCommand::SetInstrumentKeyLockMulti {
                        track: 0,
                        notes: vec![60, 64],
                        param_idx: 2,
                        value: start,
                    },
                    AppCommand::SetInstrumentKeyLockMulti {
                        track: 0,
                        notes: vec![60, 64],
                        param_idx: 3,
                        value: end,
                    },
                ],
                "sampler-range",
                "Set sampler range",
            );
            assert!(matches!(outcome, Ok(EditOutcome::Applied(_))), "{outcome:?}");
        }
        finish_active_gesture(&mut app);
        assert_eq!(app.history.undo_len(), 1);

        assert!(matches!(undo(&mut app), HistoryReplay::Applied(_)));
        assert!(app
            .state
            .capture_pattern_instrument_device_values(0, pattern)
            .unwrap()
            .bit_exact_eq(&before));
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
    fn selected_step_plock_on_custom_effect_slot_undoes_as_one_entry() {
        let state = SequencerState::new(1, vec![default_empty_effect_chain()]);
        let slot_idx = BUILTIN_SLOT_COUNT;
        let descriptor = crate::effects::EffectDescriptor::builtin_filter();
        state.pattern.effect_chains[0][slot_idx].apply_descriptor(&descriptor, 0);
        assert!(state.save_current_pattern_snapshot(
            1,
            &[-1],
            &[44_100],
            &["Track 1".to_string()],
            &[InstrumentType::Sampler],
        ));
        let mut app = test_app(state);
        app.graph.effect_descriptors = vec![EffectDescriptor::default_full_chain()];
        app.graph.effect_descriptors[0][slot_idx] = descriptor;
        let steps = vec![2, 6];
        let before = steps
            .iter()
            .map(|step| app.state.capture_step_snapshot(0, *step))
            .collect::<Vec<_>>();

        let outcome = apply_coalesced_device_plock_batch(
            &mut app,
            &[AppCommand::SetEffectPlockMulti {
                track: 0,
                steps: steps.clone(),
                slot_idx,
                param_idx: 2,
                value: 1_200.0,
            }],
            "custom-effect-control",
            "Set custom effect p-lock",
        );
        assert!(matches!(outcome, Ok(EditOutcome::Applied(_))), "{outcome:?}");
        finish_active_gesture(&mut app);
        assert_eq!(app.history.undo_len(), 1);
        for step in &steps {
            assert_eq!(
                app.state.pattern.effect_chains[0][slot_idx].plocks.get(*step, 2),
                Some(1_200.0),
            );
        }

        assert!(matches!(undo(&mut app), HistoryReplay::Applied(_)));
        for (step, expected) in steps.iter().zip(before) {
            assert!(step_snapshot_bit_exact_eq(
                &app.state.capture_step_snapshot(0, *step),
                &expected,
            ));
        }
    }

    #[test]
    fn rack_strip_drag_and_solo_round_trip_with_derived_state() {
        let state = SequencerState::new(1, vec![default_empty_effect_chain()]);
        let sampler = crate::effects::EffectDescriptor::builtin_sampler();
        state.set_rack_track_for_all_pattern_snapshots(
            0,
            crate::sequencer::RackTrackSnapshot::new(
                vec![crate::sequencer::RackSlotSnapshot {
                    instrument_type: InstrumentType::Sampler,
                    instrument_run_mode:
                        crate::sequencer::CustomInstrumentRunMode::Instrument,
                    instrument_base_note_offset: 0.0,
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
                vec![crate::sequencer::RackSlotSnapshot {
                    instrument_type: InstrumentType::Sampler,
                    instrument_run_mode: crate::sequencer::CustomInstrumentRunMode::Instrument,
                    instrument_base_note_offset: 0.0,
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
    fn key_lock_merge_identity_includes_the_edited_notes() {
        let first = AppCommand::SetInstrumentKeyLock {
            track: 0,
            note: 60,
            param_idx: 2,
            value: 0.25,
        };
        let second = AppCommand::SetInstrumentKeyLock {
            track: 0,
            note: 61,
            param_idx: 2,
            value: 0.25,
        };
        assert_ne!(
            device_value_merge_suffix(&first),
            device_value_merge_suffix(&second),
            "key-lock edits for different notes must not share a history merge identity"
        );

        let multi_a = AppCommand::SetInstrumentKeyLockMulti {
            track: 0,
            notes: vec![64, 60, 64],
            param_idx: 2,
            value: 0.5,
        };
        let multi_b = AppCommand::SetInstrumentKeyLockMulti {
            track: 0,
            notes: vec![60, 64],
            param_idx: 2,
            value: 0.75,
        };
        assert_eq!(
            device_value_merge_suffix(&multi_a),
            device_value_merge_suffix(&multi_b),
            "note-set identity should be order-independent and deduplicated"
        );
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
        // Play now starts unified song playback (row apply + silent-start
        // auto-latch), which publishes more than one snapshot; the invariant
        // is that it published and recorded nothing, not the exact count.
        assert!(app.state.scheduler_snapshot_version() > version);

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
    fn drum_rack_pattern_resize_uses_member_lengths_and_one_undo_entry() {
        let mut app = test_app(SequencerState::new(
            3,
            vec![
                default_empty_effect_chain(),
                default_empty_effect_chain(),
                default_empty_effect_chain(),
            ],
        ));
        app.tracks = vec![
            "Kick".to_string(),
            "Snare".to_string(),
            "Hat".to_string(),
        ];
        app.track_registry =
            crate::sequencer::TrackRegistry::for_legacy_track_count(3).unwrap();
        app.groups.push(crate::project::ProjectTrackGroup {
            id: 41,
            name: "Kit".to_string(),
            color: [0.5; 3],
            collapsed: false,
            members: vec![0, 1, 2],
            bus_id: 7,
            rack: Some(crate::project::ProjectRackConfig::default()),
            rack_members: Vec::new(),
        });
        for (track, length) in [8, 1, MAX_STEPS].into_iter().enumerate() {
            app.state.pattern.track_params[track].set_num_steps(length);
        }
        app.state.pattern.patterns[0].set_step_active(3, true);

        let doubled = app
            .resize_drum_rack_patterns_recorded(41, PatternLengthChange::Double)
            .expect("double rack member patterns");
        assert!(matches!(doubled, EditOutcome::Applied(_)));
        fn lengths(app: &App) -> Vec<usize> {
            (0..3)
                .map(|track| app.state.pattern.track_params[track].get_num_steps())
                .collect()
        }
        assert_eq!(lengths(&app), vec![16, 2, MAX_STEPS]);
        assert!(app.state.pattern.patterns[0].is_active(11));
        assert_eq!(app.history.undo_len(), 1, "rack resize is one undo step");

        assert!(matches!(undo(&mut app), HistoryReplay::Applied(_)));
        assert_eq!(lengths(&app), vec![8, 1, MAX_STEPS]);
        assert!(!app.state.pattern.patterns[0].is_active(11));
        assert!(matches!(redo(&mut app), HistoryReplay::Applied(_)));

        let halved = app
            .resize_drum_rack_patterns_recorded(41, PatternLengthChange::Halve)
            .expect("halve rack member patterns");
        assert!(matches!(halved, EditOutcome::Applied(_)));
        assert_eq!(lengths(&app), vec![8, 1, MAX_STEPS / 2]);
        assert_eq!(app.history.undo_len(), 2, "halve adds one undo step");
        assert!(matches!(undo(&mut app), HistoryReplay::Applied(_)));
        assert_eq!(lengths(&app), vec![16, 2, MAX_STEPS]);
    }

    /// Regression: deleting the last loose track of a plain group that still
    /// holds a child rack must NOT drop the group. Its bus is the rack bus's
    /// output, so dropping it orphans that bus in-session and — because the
    /// load path keeps such a group alive (`projects.rs`, the group filter on
    /// `!group.rack_members.is_empty()`) — the rack would silently reroute to
    /// the master mix on reload, losing the parent's fader and FX.
    #[test]
    fn deleting_the_last_loose_track_keeps_a_group_that_still_holds_a_rack() {
        let mut app = test_app(SequencerState::new(
            2,
            vec![default_empty_effect_chain(), default_empty_effect_chain()],
        ));
        app.tracks = vec!["Bass".to_string(), "Kick".to_string()];
        app.track_registry =
            crate::sequencer::TrackRegistry::for_legacy_track_count(2).unwrap();
        let parent_bus = BusId(70);
        let rack_bus = BusId(71);
        app.buses.push(crate::app::BusChannelState::new(parent_bus, "Group"));
        app.buses.push({
            let mut bus = crate::app::BusChannelState::new(rack_bus, "Rack");
            bus.output = crate::project::BusOutput::Bus(parent_bus.0);
            bus
        });
        // A plain group: one loose track (0) plus a child rack holding track 1.
        app.groups.push(crate::project::ProjectTrackGroup {
            id: 80,
            name: "Group".to_string(),
            color: [0.5; 3],
            collapsed: false,
            members: vec![0],
            bus_id: parent_bus.0,
            rack: None,
            rack_members: vec![81],
        });
        app.groups.push(crate::project::ProjectTrackGroup {
            id: 81,
            name: "Rack".to_string(),
            color: [0.5; 3],
            collapsed: false,
            members: vec![1],
            bus_id: rack_bus.0,
            rack: Some(crate::project::ProjectRackConfig::default()),
            rack_members: Vec::new(),
        });

        app.remap_groups_after_track_delete(0);

        let parent = app
            .groups
            .iter()
            .find(|group| group.id == 80)
            .expect("a memberless plain group survives while it holds a rack");
        assert!(parent.members.is_empty(), "the loose track is gone");
        assert_eq!(parent.rack_members, vec![81], "the child rack remains");
        assert_eq!(
            app.buses
                .iter()
                .find(|bus| bus.id == rack_bus)
                .expect("rack bus")
                .output,
            crate::project::BusOutput::Bus(parent_bus.0),
            "the rack bus still chains into the surviving group bus",
        );
        let rack = app
            .groups
            .iter()
            .find(|group| group.id == 81)
            .expect("the rack itself survives");
        assert_eq!(rack.members, vec![0], "the rack member reindexed down");
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
    fn transport_and_reverb_params_round_trip_bit_exactly() {
        let graph = TestLiveGraph::new("transport-reverb-history-test");
        let mut app = test_app(SequencerState::new(
            1,
            vec![default_empty_effect_chain()],
        ));
        app.graph.lg = graph.0;
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
        try_apply_command(
            &mut app,
            AppCommand::SetReverbParam {
                param_idx: 1,
                value: 0.375,
            },
        )
        .unwrap();
        finish_active_gesture(&mut app);
        let after = capture_transport_authoring(&app);

        assert!(matches!(undo(&mut app), HistoryReplay::Applied(_)));
        assert_eq!(app.state.scheduler_snapshot_version(), scheduler_version + 1);
        assert!(matches!(undo(&mut app), HistoryReplay::Applied(_)));
        assert_eq!(app.state.scheduler_snapshot_version(), scheduler_version + 1);
        assert!(matches!(undo(&mut app), HistoryReplay::Applied(_)));
        assert_eq!(app.state.scheduler_snapshot_version(), scheduler_version + 2);
        assert_eq!(capture_transport_authoring(&app), before);
        assert!(matches!(redo(&mut app), HistoryReplay::Applied(_)));
        assert!(matches!(redo(&mut app), HistoryReplay::Applied(_)));
        assert!(matches!(redo(&mut app), HistoryReplay::Applied(_)));
        assert_eq!(capture_transport_authoring(&app), after);
    }

    #[test]
    fn authoring_state_oracle_proves_mixed_command_round_trips() {
        let graph = TestLiveGraph::new("authoring-state-oracle-test");
        let mut app = test_app(SequencerState::new(
            1,
            vec![default_empty_effect_chain()],
        ));
        app.graph.lg = graph.0;
        configure_test_sampler_project(&mut app, "/tmp/undo-authoring-oracle.wav");
        let before = app.capture_authoring_state_snapshot().unwrap();

        try_apply_command(&mut app, AppCommand::ToggleStep { track: 0, step: 3 }).unwrap();
        try_apply_command(
            &mut app,
            AppCommand::SetTrackPan { track: 0, value: -0.625 },
        )
        .unwrap();
        finish_active_gesture(&mut app);
        try_apply_command(&mut app, AppCommand::SetBpm { bpm: 147 }).unwrap();
        finish_active_gesture(&mut app);
        try_apply_command(
            &mut app,
            AppCommand::SetReverbParam {
                param_idx: 2,
                value: 0.75,
            },
        )
        .unwrap();
        finish_active_gesture(&mut app);
        let after = app.capture_authoring_state_snapshot().unwrap();
        assert_ne!(after, before);

        let entries = app.history.undo_len();
        for _ in 0..entries {
            assert!(matches!(undo(&mut app), HistoryReplay::Applied(_)));
        }
        assert_eq!(app.capture_authoring_state_snapshot().unwrap(), before);
        for _ in 0..entries {
            assert!(matches!(redo(&mut app), HistoryReplay::Applied(_)));
        }
        assert_eq!(app.capture_authoring_state_snapshot().unwrap(), after);
    }

    #[test]
    fn scalar_and_multi_step_history_patches_are_target_scoped() {
        let mut app = test_app(SequencerState::new(
            2,
            vec![default_empty_effect_chain(), default_empty_effect_chain()],
        ));
        app.tracks.push("Track 2".to_string());
        app.track_registry = crate::sequencer::TrackRegistry::for_legacy_track_count(2).unwrap();
        for step in [2, 7, 11] {
            app.state.pattern.patterns[0].set_step_active(step, true);
        }
        try_apply_command(
            &mut app,
            AppCommand::ClearSteps {
                track: 0,
                steps: vec![2, 7, 11],
            },
        ).unwrap();
        let EditPatch::StepCells(step_patch) = app.history.next_undo_patch().unwrap() else {
            panic!("multi-step edit must retain a cell patch, not a project snapshot");
        };
        assert_eq!(step_patch.cells.len(), 3);

        try_apply_command(
            &mut app,
            AppCommand::SetTrackAttack {
                track: 1,
                ms: 23.5,
            },
        ).unwrap();
        finish_active_gesture(&mut app);
        let EditPatch::TrackParams(track_patch) = app.history.next_undo_patch().unwrap() else {
            panic!("scalar edit must retain one stable track patch");
        };
        assert_eq!(track_patch.target.track, app.track_registry.id_at(1).unwrap());
    }

    #[test]
    fn deterministic_mixed_history_stress_round_trips_with_scene_switches_and_no_ops() {
        const SEED: u64 = 0x5eed_8bad_f00d_cafe;
        let state = SequencerState::new(1, vec![default_empty_effect_chain()]);
        state.replace_pattern_repository(
            vec![
                PatternSnapshot::new_default(
                    1,
                    &[crate::effects::EffectDescriptor::default_full_chain()],
                ),
                PatternSnapshot::new_default(
                    1,
                    &[crate::effects::EffectDescriptor::default_full_chain()],
                ),
            ],
            0,
        );
        state.restore_current_pattern_from_repository().unwrap();
        let graph = TestLiveGraph::new("deterministic-history-stress-test");
        let mut app = test_app(state);
        app.graph.lg = graph.0;
        configure_test_sampler_project(&mut app, "/tmp/undo-history-stress.wav");
        app.state.launch_scene(
            1,
            1,
            &[-1],
            &[44_100],
            &[String::new()],
            &[InstrumentType::Sampler],
        ).unwrap();
        let _ = app.capture_authoring_state_snapshot().unwrap();
        app.state.launch_scene(
            0,
            1,
            &[-1],
            &[44_100],
            &[String::new()],
            &[InstrumentType::Sampler],
        ).unwrap();
        let initial = app.capture_authoring_state_snapshot().unwrap();
        let mut rng = SEED;
        let mut operations = Vec::new();
        for index in 0..96 {
            rng = rng.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1);
            let choice = (rng >> 32) % 7;
            let description = match choice {
                0 => {
                    let step = (rng as usize) % 16;
                    try_apply_command(&mut app, AppCommand::ToggleStep { track: 0, step })
                        .unwrap();
                    format!("toggle-step {step}")
                }
                1 => {
                    let value = ((rng >> 16) as u16) as f32 / u16::MAX as f32 * 2.0 - 1.0;
                    try_apply_command(&mut app, AppCommand::SetTrackPan { track: 0, value })
                        .unwrap();
                    finish_active_gesture(&mut app);
                    format!("set-pan {:08x}", value.to_bits())
                }
                2 => {
                    let bpm = 40 + (rng % 220) as u32;
                    try_apply_command(&mut app, AppCommand::SetBpm { bpm }).unwrap();
                    finish_active_gesture(&mut app);
                    format!("set-bpm {bpm}")
                }
                3 => {
                    let result = undo(&mut app);
                    format!("undo {result:?}")
                }
                4 => {
                    let result = redo(&mut app);
                    format!("redo {result:?}")
                }
                5 => {
                    let scene = index % 2;
                    app.state.launch_scene(
                        scene,
                        1,
                        &[-1],
                        &[44_100],
                        &[String::new()],
                        &[InstrumentType::Sampler],
                    ).unwrap();
                    format!("launch-scene {scene}")
                }
                _ => {
                    let current = f32::from_bits(
                        app.state.pattern.track_params[0].pan.load(Ordering::Relaxed),
                    );
                    let outcome = try_apply_command(
                        &mut app,
                        AppCommand::SetTrackPan { track: 0, value: current },
                    ).unwrap();
                    finish_active_gesture(&mut app);
                    format!("no-op {outcome:?}")
                }
            };
            operations.push(format!("{index}: {description}"));
        }
        finish_active_gesture(&mut app);
        let final_scene = app.state.current_scene_index();
        let final_state = app.capture_authoring_state_snapshot().unwrap();
        let applied_entry_count = app.history.undo_len();
        for _ in 0..applied_entry_count {
            assert!(matches!(undo(&mut app), HistoryReplay::Applied(_)));
        }
        app.state.launch_scene(
            0,
            1,
            &[-1],
            &[44_100],
            &[String::new()],
            &[InstrumentType::Sampler],
        ).unwrap();
        let restored_initial = app.capture_authoring_state_snapshot().unwrap();
        assert_eq!(
            restored_initial,
            initial,
            "seed={SEED:#x} difference={:?}\n{}",
            restored_initial.first_difference(&initial),
            operations.join("\n"),
        );
        for _ in 0..applied_entry_count {
            assert!(matches!(redo(&mut app), HistoryReplay::Applied(_)));
        }
        app.state.launch_scene(
            final_scene,
            1,
            &[-1],
            &[44_100],
            &[String::new()],
            &[InstrumentType::Sampler],
        ).unwrap();
        let restored_final = app.capture_authoring_state_snapshot().unwrap();
        assert_eq!(
            restored_final,
            final_state,
            "seed={SEED:#x} difference={:?}\n{}",
            restored_final.first_difference(&final_state),
            operations.join("\n"),
        );
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
    fn compound_authoring_request_undoes_as_one_entry() {
        let mut app = test_app(SequencerState::new(
            1,
            vec![default_empty_effect_chain()],
        ));
        let before_1 = app.state.capture_step_snapshot(0, 1);
        let before_6 = app.state.capture_step_snapshot(0, 6);
        let checkpoint = app.history.undo_len();
        assert!(matches!(
            try_apply_command(&mut app, AppCommand::ToggleStep { track: 0, step: 1 }),
            Ok(EditOutcome::Applied(_))
        ));
        assert!(matches!(
            try_apply_command(&mut app, AppCommand::ToggleStep { track: 0, step: 6 }),
            Ok(EditOutcome::Applied(_))
        ));
        squash_history_since(&mut app, checkpoint, "Generated pattern")
            .expect("two edits should become one compound entry");
        assert_eq!(app.history.undo_len(), 1);

        assert!(matches!(undo(&mut app), HistoryReplay::Applied(_)));
        assert!(step_snapshot_bit_exact_eq(
            &before_1,
            &app.state.capture_step_snapshot(0, 1),
        ));
        assert!(step_snapshot_bit_exact_eq(
            &before_6,
            &app.state.capture_step_snapshot(0, 6),
        ));
        assert!(matches!(redo(&mut app), HistoryReplay::Applied(_)));
        assert!(app.state.pattern.patterns[0].is_active(1));
        assert!(app.state.pattern.patterns[0].is_active(6));
    }

    #[test]
    fn failed_compound_replay_restores_applied_children_and_preserves_history() {
        let mut app = test_app(SequencerState::new(
            1,
            vec![default_empty_effect_chain()],
        ));
        let checkpoint = app.history.undo_len();
        assert!(matches!(
            try_apply_command(&mut app, AppCommand::ToggleStep { track: 0, step: 4 }),
            Ok(EditOutcome::Applied(_))
        ));
        assert!(matches!(
            try_apply_command(&mut app, AppCommand::SetBpm { bpm: 177 }),
            Ok(EditOutcome::Applied(_))
        ));
        finish_active_gesture(&mut app);
        squash_history_since(&mut app, checkpoint, "Step and tempo")
            .expect("two edits should become one compound entry");
        let after = capture_transport_authoring(&app);
        let revision = app.history.current_revision();

        let track = app.track_registry.id_at(0).expect("stable track target");
        assert_eq!(app.track_registry.remove(track), Some(0));

        assert!(matches!(undo(&mut app), HistoryReplay::Failed(_)));
        assert!(app.state.pattern.patterns[0].is_active(4));
        assert_eq!(capture_transport_authoring(&app), after);
        assert_eq!((app.history.undo_len(), app.history.redo_len()), (1, 0));
        assert_eq!(app.history.current_revision(), revision);
    }

    #[test]
    fn failed_authoring_request_rolls_back_and_preserves_prior_history() {
        let mut app = test_app(SequencerState::new(
            1,
            vec![default_empty_effect_chain()],
        ));
        assert!(matches!(
            try_apply_command(&mut app, AppCommand::ToggleStep { track: 0, step: 9 }),
            Ok(EditOutcome::Applied(_))
        ));
        let checkpoint = app.history.clone();
        let checkpoint_revision = checkpoint.current_revision();
        assert!(matches!(
            try_apply_command(&mut app, AppCommand::ToggleStep { track: 0, step: 2 }),
            Ok(EditOutcome::Applied(_))
        ));
        assert!(matches!(
            try_apply_command(&mut app, AppCommand::ToggleStep { track: 0, step: 5 }),
            Ok(EditOutcome::Applied(_))
        ));

        rollback_history_to(&mut app, checkpoint).expect("rollback generated request");
        assert_eq!(app.history.undo_len(), 1);
        assert_eq!(app.history.current_revision(), checkpoint_revision);
        assert!(app.state.pattern.patterns[0].is_active(9));
        assert!(!app.state.pattern.patterns[0].is_active(2));
        assert!(!app.state.pattern.patterns[0].is_active(5));
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
        // Solo left the persisted model (takes spec 17.8): it flips the live
        // atomic but is deliberately outside the round-trip law — no snapshot
        // diff, no history entry.
        assert!(try_apply_command(&mut app, AppCommand::ToggleTrackSolo { track: 0 }).is_ok());
        assert!(app.state.pattern.track_params[0].is_solo());
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
