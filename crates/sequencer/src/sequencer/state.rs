use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::effects::{
    EffectDescriptor, EffectSlotSnapshot, EffectSlotState, EffectSlotValuesSnapshot, HostControl,
    MAX_SLOT_PARAMS,
};
use crate::graph::{GraphVisualizationSnapshot, ProjectGraphOverrides};
use crate::neural::{
    remap_neural_network_routes_after_track_delete, NeuralVisualizationSnapshot,
    ProjectNeuralNetwork,
};
use crate::plock_variants::{
    live_track_key_lock_variant_key, live_track_key_lock_variant_keys, live_track_variant_key,
    live_track_variant_keys, PlockVariantAssignment, PlockVariantDomain, PlockVariantKey,
    PlockVariantRegistry,
};
use crate::voice::MAX_VOICES;

use super::data::{
    sync_beats, ChordData, ChordSnapshot, CustomInstrumentRunMode, InstrumentType, ModConnection,
    RackRouting, StepData, StepParam, SwingPLockData, SwingResolution, SwingResolutionPLockData,
    Timebase, TimebasePLockData, TrackParams, TrackParamsSnapshot, TrackPattern, TrackSoundState,
    DEFAULT_BPM, EXT_MOD_INPUT_COUNT, MAX_INSTRUMENT_ENGINES, MAX_RACK_SLOTS, MAX_SAMPLER_POOLS,
    MAX_STEPS, MAX_TRACKS, NUM_PARAMS, TRACK_PATTERN_WORDS,
};
use super::snapshot::{SequencerSnapshot, SequencerTransportSnapshot};
use super::{BusId, TrackOutput};

/// Stable logical identity for a sequencer track.
///
/// Dense track indices remain the runtime addressing scheme, but authoring
/// references that must survive reordering use this id instead.
#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct TrackId(pub u64);

impl TrackId {
    pub const MIN: Self = Self(1);

    pub fn new(value: u64) -> Option<Self> {
        (value != 0).then_some(Self(value))
    }
}

#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct EffectInstanceId(pub u64);

#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct MidiFxInstanceId(pub u64);

#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct RackSlotId(pub u64);

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TrackRegistryError {
    InvalidId(TrackId),
    DuplicateId(TrackId),
    IndexOutOfRange { index: usize, len: usize },
    IdExhausted,
}

/// Bidirectional mapping between stable track ids and dense runtime indices.
#[derive(Clone, Debug)]
pub struct TrackRegistry {
    order: Vec<TrackId>,
    index_by_id: HashMap<TrackId, usize>,
    next_id: u64,
}

impl Default for TrackRegistry {
    fn default() -> Self {
        Self {
            order: Vec::new(),
            index_by_id: HashMap::new(),
            next_id: TrackId::MIN.0,
        }
    }
}

impl TrackRegistry {
    pub fn for_legacy_track_count(track_count: usize) -> Result<Self, TrackRegistryError> {
        let count = u64::try_from(track_count).map_err(|_| TrackRegistryError::IdExhausted)?;
        if count == u64::MAX {
            return Err(TrackRegistryError::IdExhausted);
        }
        Self::from_ids((1..=count).map(TrackId))
    }

    pub fn from_ids(ids: impl IntoIterator<Item = TrackId>) -> Result<Self, TrackRegistryError> {
        let order = ids.into_iter().collect::<Vec<_>>();
        let mut index_by_id = HashMap::with_capacity(order.len());
        let mut max_id = 0u64;
        for (index, id) in order.iter().copied().enumerate() {
            if id.0 == 0 {
                return Err(TrackRegistryError::InvalidId(id));
            }
            if index_by_id.insert(id, index).is_some() {
                return Err(TrackRegistryError::DuplicateId(id));
            }
            max_id = max_id.max(id.0);
        }
        let next_id = max_id.checked_add(1).unwrap_or(0);
        Ok(Self {
            order,
            index_by_id,
            next_id,
        })
    }

    pub fn len(&self) -> usize {
        self.order.len()
    }

    pub fn is_empty(&self) -> bool {
        self.order.is_empty()
    }

    pub fn ids(&self) -> &[TrackId] {
        &self.order
    }

    pub fn id_at(&self, index: usize) -> Option<TrackId> {
        self.order.get(index).copied()
    }

    pub fn index_of(&self, id: TrackId) -> Option<usize> {
        self.index_by_id.get(&id).copied()
    }

    pub fn can_allocate(&self) -> bool {
        self.next_id != 0
    }

    pub fn allocate_at(&mut self, index: usize) -> Result<TrackId, TrackRegistryError> {
        if index > self.order.len() {
            return Err(TrackRegistryError::IndexOutOfRange {
                index,
                len: self.order.len(),
            });
        }
        let id = TrackId::new(self.next_id).ok_or(TrackRegistryError::IdExhausted)?;
        self.next_id = self.next_id.checked_add(1).unwrap_or(0);
        self.order.insert(index, id);
        self.reindex_from(index);
        Ok(id)
    }

    pub fn allocate(&mut self) -> Result<TrackId, TrackRegistryError> {
        self.allocate_at(self.order.len())
    }

    pub fn remove(&mut self, id: TrackId) -> Option<usize> {
        let index = self.index_by_id.remove(&id)?;
        self.order.remove(index);
        self.reindex_from(index);
        Some(index)
    }

    pub fn replace_at(
        &mut self,
        index: usize,
        replacement: TrackId,
    ) -> Result<TrackId, TrackRegistryError> {
        if replacement.0 == 0 {
            return Err(TrackRegistryError::InvalidId(replacement));
        }
        let Some(current) = self.order.get(index).copied() else {
            return Err(TrackRegistryError::IndexOutOfRange {
                index,
                len: self.order.len(),
            });
        };
        if current == replacement {
            return Ok(current);
        }
        if self.index_by_id.contains_key(&replacement) {
            return Err(TrackRegistryError::DuplicateId(replacement));
        }
        self.order[index] = replacement;
        self.index_by_id.remove(&current);
        self.index_by_id.insert(replacement, index);
        if replacement.0 >= self.next_id && self.next_id != 0 {
            self.next_id = replacement.0.checked_add(1).unwrap_or(0);
        }
        Ok(current)
    }

    pub fn move_to(&mut self, id: TrackId, target: usize) -> Result<(), TrackRegistryError> {
        if target >= self.order.len() {
            return Err(TrackRegistryError::IndexOutOfRange {
                index: target,
                len: self.order.len(),
            });
        }
        let source = self
            .index_of(id)
            .ok_or(TrackRegistryError::InvalidId(id))?;
        if source == target {
            return Ok(());
        }
        self.order.remove(source);
        self.order.insert(target, id);
        self.reindex_from(source.min(target));
        Ok(())
    }

    fn reindex_from(&mut self, start: usize) {
        for (index, id) in self.order.iter().copied().enumerate().skip(start) {
            self.index_by_id.insert(id, index);
        }
    }
}

#[cfg(test)]
mod track_registry_tests {
    use super::{TrackId, TrackRegistry, TrackRegistryError};

    #[test]
    fn stable_track_ids_resolve_after_dense_reordering_and_deletion() {
        let mut registry = TrackRegistry::default();
        let first = registry.allocate().expect("allocate first track id");
        let second = registry.allocate().expect("allocate second track id");
        let third = registry.allocate().expect("allocate third track id");

        registry.move_to(third, 0).expect("move third track");
        assert_eq!(registry.ids(), &[third, first, second]);
        assert_eq!(registry.index_of(first), Some(1));
        assert_eq!(registry.index_of(third), Some(0));

        assert_eq!(registry.remove(first), Some(1));
        assert_eq!(registry.ids(), &[third, second]);
        assert_eq!(registry.index_of(second), Some(1));
        assert_eq!(registry.index_of(first), None);
    }

    #[test]
    fn imported_track_ids_are_validated_and_new_ids_never_reuse_existing_values() {
        assert!(matches!(
            TrackRegistry::from_ids([TrackId(1), TrackId(1)]),
            Err(TrackRegistryError::DuplicateId(TrackId(1)))
        ));
        assert!(matches!(
            TrackRegistry::from_ids([TrackId(0)]),
            Err(TrackRegistryError::InvalidId(TrackId(0)))
        ));

        let mut registry = TrackRegistry::from_ids([TrackId(8), TrackId(3)])
            .expect("import unique nonzero ids");
        assert_eq!(registry.allocate().expect("allocate after import"), TrackId(9));

        let legacy = TrackRegistry::for_legacy_track_count(3)
            .expect("assign deterministic ids to legacy tracks");
        assert_eq!(legacy.ids(), &[TrackId(1), TrackId(2), TrackId(3)]);
    }
}

#[derive(Clone, Debug)]
pub struct StepSlotPlocks {
    pub params: Vec<Option<f32>>,
    pub tensor_params: Vec<Option<Vec<f32>>>,
}

impl StepSlotPlocks {
    fn clear(&mut self) {
        self.params.fill(None);
        self.tensor_params.fill(None);
    }
}

pub const RACK_SLOT_PARAM_COUNT: usize = 6;
pub const RACK_MACRO_COUNT: usize = 8;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct RackMacroId(u8);

impl RackMacroId {
    pub const ALL: [Self; RACK_MACRO_COUNT] = [
        Self(0),
        Self(1),
        Self(2),
        Self(3),
        Self(4),
        Self(5),
        Self(6),
        Self(7),
    ];

    pub fn from_index(index: usize) -> Option<Self> {
        (index < RACK_MACRO_COUNT).then_some(Self(index as u8))
    }

    pub fn index(self) -> usize {
        self.0 as usize
    }

    pub fn stable_key(self) -> String {
        format!("macro_{}", self.index() + 1)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum RackMacroCurve {
    #[default]
    Linear,
    Exp,
    Log,
}

#[derive(Clone, Debug, PartialEq)]
pub enum RackMacroTarget {
    SlotParam {
        slot: usize,
        param: String,
    },
    SlotInstrumentParam {
        slot: usize,
        param: String,
        param_index: usize,
    },
    SlotEffectParam {
        slot: usize,
        effect_slot: usize,
        param: String,
        param_index: usize,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub struct RackMacroMapping {
    pub target: RackMacroTarget,
    pub range_min: f32,
    pub range_max: f32,
    pub curve: RackMacroCurve,
}

#[derive(Clone, Debug, PartialEq)]
pub struct RackMacro {
    pub id: RackMacroId,
    pub name: String,
    pub value: f32,
    pub mappings: Vec<RackMacroMapping>,
    pub plocks: Vec<Option<f32>>,
}

impl RackMacro {
    fn default_for(id: RackMacroId) -> Self {
        Self {
            id,
            name: format!("Macro {}", id.index() + 1),
            value: 0.0,
            mappings: Vec::new(),
            plocks: vec![None; MAX_STEPS],
        }
    }

    pub fn value_at(&self, step: usize) -> f32 {
        self.plocks
            .get(step)
            .and_then(|value| *value)
            .unwrap_or(self.value)
            .clamp(0.0, 1.0)
    }
}

pub fn default_rack_macros() -> Vec<RackMacro> {
    RackMacroId::ALL
        .into_iter()
        .map(RackMacro::default_for)
        .collect()
}

fn remove_rack_macro_slot_targets(macros: &mut [RackMacro], removed_slot: usize) {
    for rack_macro in macros {
        rack_macro.mappings.retain(|mapping| match mapping.target {
            RackMacroTarget::SlotParam { slot, .. }
            | RackMacroTarget::SlotInstrumentParam { slot, .. }
            | RackMacroTarget::SlotEffectParam { slot, .. } => slot != removed_slot,
        });
        for mapping in &mut rack_macro.mappings {
            let slot = match &mut mapping.target {
                RackMacroTarget::SlotParam { slot, .. }
                | RackMacroTarget::SlotInstrumentParam { slot, .. }
                | RackMacroTarget::SlotEffectParam { slot, .. } => slot,
            };
            if *slot > removed_slot {
                *slot -= 1;
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RackSlotParam {
    BaseNote,
    Gain,
    Pan,
    MaxPolyphony,
    Mute,
    Solo,
}

impl RackSlotParam {
    pub const ALL: [Self; RACK_SLOT_PARAM_COUNT] = [
        Self::BaseNote,
        Self::Gain,
        Self::Pan,
        Self::MaxPolyphony,
        Self::Mute,
        Self::Solo,
    ];

    pub fn index(self) -> usize {
        match self {
            Self::BaseNote => 0,
            Self::Gain => 1,
            Self::Pan => 2,
            Self::MaxPolyphony => 3,
            Self::Mute => 4,
            Self::Solo => 5,
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "base-note" => Some(Self::BaseNote),
            "gain" => Some(Self::Gain),
            "pan" => Some(Self::Pan),
            "max-polyphony" => Some(Self::MaxPolyphony),
            "mute" => Some(Self::Mute),
            "solo" => Some(Self::Solo),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::BaseNote => "base-note",
            Self::Gain => "gain",
            Self::Pan => "pan",
            Self::MaxPolyphony => "max-polyphony",
            Self::Mute => "mute",
            Self::Solo => "solo",
        }
    }

    pub fn clamp(self, value: f32) -> f32 {
        match self {
            Self::BaseNote => value.clamp(-48.0, 48.0),
            Self::Gain => value.clamp(0.0, 2.0),
            Self::Pan => value.clamp(-1.0, 1.0),
            Self::MaxPolyphony => value.round().clamp(1.0, crate::voice::MAX_VOICES as f32),
            Self::Mute | Self::Solo => {
                if value > 0.5 {
                    1.0
                } else {
                    0.0
                }
            }
        }
    }
}

#[derive(Clone, Debug)]
pub struct RackSlotParamPlocks {
    pub rows: Vec<Vec<Option<f32>>>,
}

impl RackSlotParamPlocks {
    pub fn new() -> Self {
        Self {
            rows: (0..MAX_STEPS)
                .map(|_| vec![None; RACK_SLOT_PARAM_COUNT])
                .collect(),
        }
    }

    pub fn from_rows(mut rows: Vec<Vec<Option<f32>>>) -> Self {
        rows.truncate(MAX_STEPS);
        while rows.len() < MAX_STEPS {
            rows.push(Vec::new());
        }
        for row in &mut rows {
            row.truncate(RACK_SLOT_PARAM_COUNT);
            if row.len() < RACK_SLOT_PARAM_COUNT {
                row.resize(RACK_SLOT_PARAM_COUNT, None);
            }
            for param in RackSlotParam::ALL {
                if let Some(Some(value)) = row.get_mut(param.index()) {
                    *value = param.clamp(*value);
                }
            }
        }
        Self { rows }
    }

    fn ensure_step(&mut self, step: usize) -> bool {
        if step >= MAX_STEPS {
            return false;
        }
        while self.rows.len() <= step {
            self.rows.push(vec![None; RACK_SLOT_PARAM_COUNT]);
        }
        if self.rows[step].len() < RACK_SLOT_PARAM_COUNT {
            self.rows[step].resize(RACK_SLOT_PARAM_COUNT, None);
        }
        true
    }

    pub fn get(&self, step: usize, param: RackSlotParam) -> Option<f32> {
        self.rows
            .get(step)
            .and_then(|row| row.get(param.index()))
            .copied()
            .flatten()
    }

    pub fn set(&mut self, step: usize, param: RackSlotParam, value: f32) -> bool {
        if !self.ensure_step(step) {
            return false;
        }
        self.rows[step][param.index()] = Some(param.clamp(value));
        true
    }

    pub fn clear(&mut self, step: usize, param: RackSlotParam) -> bool {
        if step >= MAX_STEPS {
            return false;
        }
        if let Some(row) = self.rows.get_mut(step) {
            if let Some(value) = row.get_mut(param.index()) {
                *value = None;
            }
        }
        true
    }

    pub fn clear_step(&mut self, step: usize) {
        if let Some(row) = self.rows.get_mut(step) {
            for value in row.iter_mut().take(RACK_SLOT_PARAM_COUNT) {
                *value = None;
            }
        }
    }

    pub fn step_has_plock(&self, step: usize) -> bool {
        self.rows
            .get(step)
            .is_some_and(|row| row.iter().take(RACK_SLOT_PARAM_COUNT).any(Option::is_some))
    }
}

impl Default for RackSlotParamPlocks {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Clone, Debug)]
pub struct BusPatternSnapshot {
    pub id: BusId,
    pub gate_sequence: BusGateSequence,
    pub effect_plocks: Vec<Vec<Vec<Option<f32>>>>,
    /// Per-scene base (non-plocked) effect parameter values, indexed
    /// `[slot][param]`. Recalled on scene switch so a bus effect knob can
    /// hold different values per scene. Empty for legacy snapshots.
    pub effect_defaults: Vec<Vec<f32>>,
}

impl BusPatternSnapshot {
    fn remap_effect_slots(&mut self, new_to_old: &[Option<usize>]) {
        let old_plocks = std::mem::take(&mut self.effect_plocks);
        let old_defaults = std::mem::take(&mut self.effect_defaults);
        let slot_count = crate::lisp_host::MAX_CUSTOM_FX;

        self.effect_plocks = new_to_old
            .iter()
            .copied()
            .take(slot_count)
            .map(|source| {
                source
                    .and_then(|slot| old_plocks.get(slot).cloned())
                    .unwrap_or_default()
            })
            .collect();
        self.effect_defaults = new_to_old
            .iter()
            .copied()
            .take(slot_count)
            .map(|source| {
                source
                    .and_then(|slot| old_defaults.get(slot).cloned())
                    .unwrap_or_default()
            })
            .collect();

        self.effect_plocks.resize_with(slot_count, Vec::new);
        self.effect_defaults.resize_with(slot_count, Vec::new);
    }

    fn replace_effect_slot(
        &mut self,
        slot_idx: usize,
        defaults: Vec<f32>,
        plocks: Vec<Vec<Option<f32>>>,
    ) {
        if slot_idx >= crate::lisp_host::MAX_CUSTOM_FX {
            return;
        }
        self.effect_plocks.resize_with(slot_idx + 1, Vec::new);
        self.effect_defaults.resize_with(slot_idx + 1, Vec::new);
        self.effect_plocks[slot_idx] = plocks;
        self.effect_defaults[slot_idx] = defaults;
    }
}

#[derive(Clone, Debug)]
pub struct BusGateSequence {
    pub steps: [bool; MAX_STEPS],
    pub velocities: [f32; MAX_STEPS],
    pub durations: [f32; MAX_STEPS],
    pub syncs: [f32; MAX_STEPS],
    pub num_steps: usize,
    pub timebase: Timebase,
    pub swing: f32,
    pub swing_resolution: SwingResolution,
    pub timebase_plocks: [Option<Timebase>; MAX_STEPS],
    pub swing_plocks: [Option<f32>; MAX_STEPS],
    pub swing_resolution_plocks: [Option<SwingResolution>; MAX_STEPS],
}

impl Default for BusGateSequence {
    fn default() -> Self {
        Self {
            steps: [true; MAX_STEPS],
            velocities: [1.0; MAX_STEPS],
            durations: [1.0; MAX_STEPS],
            syncs: [0.0; MAX_STEPS],
            num_steps: 16,
            timebase: Timebase::Sixteenth,
            swing: 50.0,
            swing_resolution: SwingResolution::Sixteenth,
            timebase_plocks: [None; MAX_STEPS],
            swing_plocks: [None; MAX_STEPS],
            swing_resolution_plocks: [None; MAX_STEPS],
        }
    }
}

impl BusGateSequence {
    pub fn toggle_step(&mut self, step: usize) -> Option<bool> {
        let value = self.steps.get_mut(step)?;
        *value = !*value;
        Some(*value)
    }

    pub fn set_step_velocity(&mut self, step: usize, value: f32) -> Option<f32> {
        let slot = self.velocities.get_mut(step)?;
        *slot = value.clamp(0.0, 1.0);
        Some(*slot)
    }

    pub fn set_step_duration(&mut self, step: usize, value: f32) -> Option<f32> {
        let slot = self.durations.get_mut(step)?;
        *slot = value.clamp(0.1, 2.0);
        Some(*slot)
    }

    pub fn set_step_sync(&mut self, step: usize, value: f32) -> Option<f32> {
        let slot = self.syncs.get_mut(step)?;
        *slot = value
            .round()
            .clamp(0.0, (crate::sequencer::SYNC_COUNT - 1) as f32);
        Some(*slot)
    }

    pub fn set_num_steps(&mut self, value: usize) {
        self.num_steps = value.clamp(1, MAX_STEPS);
    }

    pub fn has_step_plock(&self, step: usize) -> bool {
        step < MAX_STEPS
            && (self.timebase_plocks[step].is_some()
                || self.swing_plocks[step].is_some()
                || self.swing_resolution_plocks[step].is_some())
    }
}

#[derive(Clone, Debug)]
pub struct StepSnapshot {
    pub active: bool,
    pub neural_reset: bool,
    pub params: [f32; NUM_PARAMS],
    pub chord: Vec<f32>,
    pub chord_durations: Vec<f32>,
    pub chord_delays: Vec<f32>,
    pub timebase: Option<Timebase>,
    pub swing: Option<f32>,
    pub swing_resolution: Option<SwingResolution>,
    pub midi_fx_plocks: Vec<StepSlotPlocks>,
    pub effect_plocks: Vec<StepSlotPlocks>,
    pub instrument_plocks: StepSlotPlocks,
    pub rack_macro_plocks: Vec<Option<f32>>,
    pub rack_slot_param_plocks: Vec<StepSlotPlocks>,
    pub rack_slot_instrument_plocks: Vec<StepSlotPlocks>,
    pub rack_slot_effect_plocks: Vec<Vec<StepSlotPlocks>>,
}

pub type StepCellSnapshot = StepSnapshot;

impl StepSnapshot {
    pub fn without_audio_plocks(&self) -> Self {
        let mut snapshot = self.clone();
        for plocks in &mut snapshot.midi_fx_plocks {
            plocks.clear();
        }
        for plocks in &mut snapshot.effect_plocks {
            plocks.clear();
        }
        snapshot.instrument_plocks.clear();
        snapshot.rack_macro_plocks.fill(None);
        for plocks in &mut snapshot.rack_slot_param_plocks {
            plocks.clear();
        }
        for plocks in &mut snapshot.rack_slot_instrument_plocks {
            plocks.clear();
        }
        for slot in &mut snapshot.rack_slot_effect_plocks {
            for plocks in slot {
                plocks.clear();
            }
        }
        snapshot
    }
}

fn capture_live_slot_step_plocks(slot: &EffectSlotState, step: usize) -> StepSlotPlocks {
    let num_params = slot.num_params.load(Ordering::Relaxed) as usize;
    let params = (0..num_params)
        .map(|param_idx| slot.plocks.get(step, param_idx))
        .collect();
    let tensor_params = (0..slot.tensor_params.num_params())
        .map(|tensor_idx| slot.tensor_params.plock_values(step, tensor_idx))
        .collect();
    StepSlotPlocks {
        params,
        tensor_params,
    }
}

fn capture_snapshot_slot_step_plocks(
    slot: &EffectSlotSnapshot,
    step: usize,
) -> StepSlotPlocks {
    let params = (0..slot.num_params as usize)
        .map(|param_idx| {
            slot.plocks
                .get(step)
                .and_then(|row| row.get(param_idx))
                .copied()
                .flatten()
        })
        .collect();
    let tensor_params = (0..slot.tensor_params.len())
        .map(|tensor_idx| slot.tensor_plock_values(step, tensor_idx).map(<[f32]>::to_vec))
        .collect();
    StepSlotPlocks {
        params,
        tensor_params,
    }
}

fn restore_live_slot_step_plocks(
    slot: &EffectSlotState,
    step: usize,
    saved: Option<&StepSlotPlocks>,
) {
    let num_params = slot.num_params.load(Ordering::Relaxed) as usize;
    for param_idx in 0..num_params {
        match saved
            .and_then(|plocks| plocks.params.get(param_idx))
            .copied()
            .flatten()
        {
            Some(value) => slot.set_plock(step, param_idx, value),
            None => slot.plocks.clear_param(step, param_idx),
        }
    }
    for tensor_idx in 0..slot.tensor_params.num_params() {
        let values = saved
            .and_then(|plocks| plocks.tensor_params.get(tensor_idx))
            .cloned()
            .flatten();
        if values.as_deref().is_none_or(|values| {
            !slot.tensor_params.set_plock(step, tensor_idx, values)
        }) {
            slot.tensor_params.clear_plock(step, tensor_idx);
        }
    }
}

fn restore_snapshot_slot_step_plocks(
    slot: &mut EffectSlotSnapshot,
    step: usize,
    saved: Option<&StepSlotPlocks>,
) {
    for param_idx in 0..slot.num_params as usize {
        match saved
            .and_then(|plocks| plocks.params.get(param_idx))
            .copied()
            .flatten()
        {
            Some(value) => {
                slot.set_plock(step, param_idx, value);
            }
            None => {
                slot.clear_plock(step, param_idx);
            }
        }
    }
    for tensor_idx in 0..slot.tensor_params.len() {
        let values = saved
            .and_then(|plocks| plocks.tensor_params.get(tensor_idx))
            .cloned()
            .flatten();
        let restored = values
            .map(|values| slot.set_tensor_plock(step, tensor_idx, values))
            .unwrap_or(false);
        if !restored {
            slot.clear_tensor_plock(step, tensor_idx);
        }
    }
}

#[derive(Clone)]
pub struct PatternSnapshot {
    pub track_bits: Vec<[u64; TRACK_PATTERN_WORDS]>,
    pub neural_reset_bits: Vec<[u64; TRACK_PATTERN_WORDS]>,
    pub step_data: Vec<Vec<[f32; NUM_PARAMS]>>,
    pub track_params: Vec<TrackParamsSnapshot>,
    pub effect_slots: Vec<Vec<EffectSlotSnapshot>>,
    pub midi_fx_slots: Vec<Vec<EffectSlotSnapshot>>,
    pub instrument_slots: Vec<EffectSlotSnapshot>,
    pub instrument_base_note_offsets: Vec<f32>,
    pub track_sound_states: Vec<TrackSoundState>,
    pub sample_ids: Vec<(i32, String, u32)>,
    pub chord_snapshots: Vec<ChordSnapshot>,
    pub timebase_plock_snapshots: Vec<[Option<u32>; MAX_STEPS]>,
    pub swing_plock_snapshots: Vec<[Option<u32>; MAX_STEPS]>,
    pub swing_resolution_plock_snapshots: Vec<[Option<u32>; MAX_STEPS]>,
    pub instrument_types: Vec<InstrumentType>,
    pub instrument_run_modes: Vec<CustomInstrumentRunMode>,
    pub mod_connections: Vec<ModConnection>,
    pub neural_networks: Vec<ProjectNeuralNetwork>,
    pub graph_overrides: Vec<ProjectGraphOverrides>,
    pub rack_tracks: Vec<Option<RackTrackSnapshot>>,
    pub process_chains: Vec<crate::process::TrackProcessChain>,
    pub project_process_lane_overrides: Vec<crate::process::ProjectLaneOverrides>,
    pub project_process_chain: crate::process::TrackProcessChain,
    pub plock_variant_registries: Vec<PlockVariantRegistry>,
    pub key_lock_variant_registries: Vec<PlockVariantRegistry>,
}

#[derive(Clone, Debug)]
pub struct RackTrackSnapshot {
    pub routing: RackRouting,
    pub slots: Vec<RackSlotSnapshot>,
    pub macros: Vec<RackMacro>,
    pub(crate) runtime_macro_values: Option<Arc<RackMacroRuntimeValues>>,
    pub(crate) runtime_macro_track: usize,
}

impl RackTrackSnapshot {
    pub fn new(routing: RackRouting, slots: Vec<RackSlotSnapshot>, macros: Vec<RackMacro>) -> Self {
        Self {
            routing,
            slots,
            macros,
            runtime_macro_values: None,
            runtime_macro_track: 0,
        }
    }

    pub fn normalize_macros(&mut self) {
        let mut normalized = default_rack_macros();
        for rack_macro in std::mem::take(&mut self.macros) {
            if let Some(target) = normalized.get_mut(rack_macro.id.index()) {
                *target = rack_macro;
                target.value = target.value.clamp(0.0, 1.0);
                target.plocks.resize(MAX_STEPS, None);
                target.plocks.truncate(MAX_STEPS);
            }
        }
        self.macros = normalized;
    }

    pub(crate) fn attach_runtime_macro_values(
        &mut self,
        values: Arc<RackMacroRuntimeValues>,
        track: usize,
    ) {
        self.runtime_macro_values = Some(values);
        self.runtime_macro_track = track;
    }

    pub(crate) fn runtime_macro_value_at(&self, id: RackMacroId, step: usize) -> Option<f32> {
        self.runtime_macro_values
            .as_ref()
            .and_then(|values| values.value_at(self.runtime_macro_track, id, step))
    }
}

/// Pointer-rate rack macro values shared by immutable scheduler snapshots.
///
/// Rack topology and mappings remain snapshot-owned, but default values and
/// per-step locks are fixed-size scalar control data. Keeping those scalars in
/// atomics lets the command thread publish a knob drag immediately without
/// deep-cloning every rack slot, effect descriptor, and p-lock grid.
#[derive(Debug)]
pub(crate) struct RackMacroRuntimeValues {
    defaults: Vec<[AtomicU32; RACK_MACRO_COUNT]>,
    plocks: Vec<Box<[AtomicU64]>>,
}

impl RackMacroRuntimeValues {
    fn new() -> Self {
        Self {
            defaults: (0..MAX_TRACKS)
                .map(|_| std::array::from_fn(|_| AtomicU32::new(0.0_f32.to_bits())))
                .collect(),
            plocks: (0..MAX_TRACKS)
                .map(|_| {
                    (0..RACK_MACRO_COUNT * MAX_STEPS)
                        .map(|_| AtomicU64::new(0))
                        .collect::<Vec<_>>()
                        .into_boxed_slice()
                })
                .collect(),
        }
    }

    fn encode_plock(value: Option<f32>) -> u64 {
        value.map_or(0, |value| (1_u64 << 32) | u64::from(value.to_bits()))
    }

    fn decode_plock(value: u64) -> Option<f32> {
        (value >> 32 != 0).then(|| f32::from_bits(value as u32))
    }

    fn set_default(&self, track: usize, id: RackMacroId, value: f32) {
        if let Some(defaults) = self.defaults.get(track) {
            defaults[id.index()].store(value.clamp(0.0, 1.0).to_bits(), Ordering::Relaxed);
        }
    }

    fn set_plock(&self, track: usize, id: RackMacroId, step: usize, value: Option<f32>) {
        let Some(plocks) = self.plocks.get(track) else {
            return;
        };
        let Some(cell) = plocks.get(id.index() * MAX_STEPS + step) else {
            return;
        };
        cell.store(Self::encode_plock(value), Ordering::Relaxed);
    }

    fn value_at(&self, track: usize, id: RackMacroId, step: usize) -> Option<f32> {
        let defaults = self.defaults.get(track)?;
        let plocks = self.plocks.get(track)?;
        let plock = plocks
            .get(id.index() * MAX_STEPS + step)
            .and_then(|cell| Self::decode_plock(cell.load(Ordering::Relaxed)));
        Some(plock.unwrap_or_else(|| f32::from_bits(defaults[id.index()].load(Ordering::Relaxed))))
    }

    fn sync_track(&self, track: usize, rack: Option<&RackTrackSnapshot>) {
        for index in 0..RACK_MACRO_COUNT {
            let Some(id) = RackMacroId::from_index(index) else {
                continue;
            };
            let rack_macro = rack.and_then(|rack| rack.macros.get(index));
            self.set_default(track, id, rack_macro.map_or(0.0, |item| item.value));
            for step in 0..MAX_STEPS {
                self.set_plock(
                    track,
                    id,
                    step,
                    rack_macro.and_then(|item| item.plocks.get(step).copied().flatten()),
                );
            }
        }
    }
}

#[derive(Clone, Debug)]
pub struct RackSlotSnapshot {
    pub instrument_type: InstrumentType,
    pub instrument_run_mode: CustomInstrumentRunMode,
    pub instrument_base_note_offset: f32,
    pub pad_note: Option<i32>,
    pub choke_group: Option<u8>,
    pub gain: f32,
    pub pan: f32,
    pub mute: bool,
    pub solo: bool,
    pub max_polyphony: usize,
    pub param_plocks: RackSlotParamPlocks,
    pub instrument_slot: EffectSlotSnapshot,
    pub effect_slots: Vec<EffectSlotSnapshot>,
    pub effect_descriptors: Vec<EffectDescriptor>,
    pub custom_effect_names: Vec<Option<String>>,
    pub track_sound_state: TrackSoundState,
    pub sample_id: Option<(i32, String, u32)>,
}

#[derive(Clone, Debug)]
pub struct InstrumentDeviceValuesSnapshot {
    pub slot: EffectSlotValuesSnapshot,
    pub base_note_offset_bits: u32,
    pub sound_state: TrackSoundState,
    pub key_lock_variant_registry: PlockVariantRegistry,
}

impl InstrumentDeviceValuesSnapshot {
    pub fn bit_exact_eq(&self, other: &Self) -> bool {
        self.slot.bit_exact_eq(&other.slot)
            && self.base_note_offset_bits == other.base_note_offset_bits
            && self.sound_state.engine_id == other.sound_state.engine_id
            && self.sound_state.loaded_preset == other.sound_state.loaded_preset
            && self.sound_state.dirty == other.sound_state.dirty
            && self.key_lock_variant_registry == other.key_lock_variant_registry
    }

    pub fn retained_bytes(&self) -> usize {
        std::mem::size_of::<Self>()
            + self.slot.retained_bytes()
            + self
                .sound_state
                .loaded_preset
                .as_ref()
                .map_or(0, String::capacity)
    }
}

#[derive(Clone, Debug)]
pub struct RackSlotValuesSnapshot {
    pub base_note_offset_bits: u32,
    pub choke_group: Option<u8>,
    pub gain_bits: u32,
    pub pan_bits: u32,
    pub mute: bool,
    pub solo: bool,
    pub max_polyphony: usize,
    pub param_plocks: RackSlotParamPlocks,
    pub instrument_slot: EffectSlotValuesSnapshot,
    pub effect_slots: Vec<EffectSlotValuesSnapshot>,
    pub sound_state: TrackSoundState,
}

impl RackSlotValuesSnapshot {
    pub fn bit_exact_eq(&self, other: &Self) -> bool {
        self.base_note_offset_bits == other.base_note_offset_bits
            && self.choke_group == other.choke_group
            && self.gain_bits == other.gain_bits
            && self.pan_bits == other.pan_bits
            && self.mute == other.mute
            && self.solo == other.solo
            && self.max_polyphony == other.max_polyphony
            && optional_f32_rows_bit_exact_eq(
                &self.param_plocks.rows,
                &other.param_plocks.rows,
            )
            && self.instrument_slot.bit_exact_eq(&other.instrument_slot)
            && self.effect_slots.len() == other.effect_slots.len()
            && self
                .effect_slots
                .iter()
                .zip(&other.effect_slots)
                .all(|(left, right)| left.bit_exact_eq(right))
            && self.sound_state.engine_id == other.sound_state.engine_id
            && self.sound_state.loaded_preset == other.sound_state.loaded_preset
            && self.sound_state.dirty == other.sound_state.dirty
    }

    pub fn retained_bytes(&self) -> usize {
        std::mem::size_of::<Self>()
            + self.param_plocks.rows.capacity() * std::mem::size_of::<Vec<Option<f32>>>()
            + self
                .param_plocks
                .rows
                .iter()
                .map(|row| row.capacity() * std::mem::size_of::<Option<f32>>())
                .sum::<usize>()
            + self.instrument_slot.retained_bytes()
            + self
                .effect_slots
                .iter()
                .map(EffectSlotValuesSnapshot::retained_bytes)
                .sum::<usize>()
            + self
                .sound_state
                .loaded_preset
                .as_ref()
                .map_or(0, String::capacity)
    }
}

fn optional_f32_rows_bit_exact_eq(
    left: &[Vec<Option<f32>>],
    right: &[Vec<Option<f32>>],
) -> bool {
    left.len() == right.len()
        && left.iter().zip(right).all(|(left, right)| {
            left.len() == right.len()
                && left.iter().zip(right).all(|(left, right)| match (left, right) {
                    (Some(left), Some(right)) => left.to_bits() == right.to_bits(),
                    (None, None) => true,
                    _ => false,
                })
        })
}

impl RackSlotSnapshot {
    pub fn authoring_values(&self) -> RackSlotValuesSnapshot {
        RackSlotValuesSnapshot {
            base_note_offset_bits: self.instrument_base_note_offset.to_bits(),
            choke_group: self.choke_group,
            gain_bits: self.gain.to_bits(),
            pan_bits: self.pan.to_bits(),
            mute: self.mute,
            solo: self.solo,
            max_polyphony: self.max_polyphony,
            param_plocks: self.param_plocks.clone(),
            instrument_slot: self.instrument_slot.authoring_values(),
            effect_slots: self
                .effect_slots
                .iter()
                .map(EffectSlotSnapshot::authoring_values)
                .collect(),
            sound_state: self.track_sound_state.clone(),
        }
    }

    pub fn apply_authoring_values(
        &mut self,
        values: &RackSlotValuesSnapshot,
    ) -> Result<(), String> {
        let mut instrument_slot = self.instrument_slot.clone();
        instrument_slot.apply_authoring_values(&values.instrument_slot)?;
        if self.effect_slots.len() != values.effect_slots.len() {
            return Err("rack effect chain changed while replaying history".to_string());
        }
        let mut effect_slots = self.effect_slots.clone();
        for (slot, values) in effect_slots.iter_mut().zip(&values.effect_slots) {
            slot.apply_authoring_values(values)?;
        }
        self.instrument_base_note_offset = f32::from_bits(values.base_note_offset_bits);
        self.choke_group = values.choke_group;
        self.gain = f32::from_bits(values.gain_bits);
        self.pan = f32::from_bits(values.pan_bits);
        self.mute = values.mute;
        self.solo = values.solo;
        self.max_polyphony = values.max_polyphony;
        self.param_plocks = values.param_plocks.clone();
        self.instrument_slot = instrument_slot;
        self.effect_slots = effect_slots;
        self.track_sound_state = values.sound_state.clone();
        Ok(())
    }

    pub fn empty_effect_slots() -> Vec<EffectSlotSnapshot> {
        (0..crate::lisp_host::MAX_CUSTOM_FX)
            .map(|_| EffectSlotSnapshot::new_empty())
            .collect()
    }

    pub fn empty_effect_names() -> Vec<Option<String>> {
        vec![None; crate::lisp_host::MAX_CUSTOM_FX]
    }

    pub fn normalize_effect_chain(&mut self) {
        self.effect_slots.resize_with(
            crate::lisp_host::MAX_CUSTOM_FX,
            EffectSlotSnapshot::new_empty,
        );
        self.effect_slots.truncate(crate::lisp_host::MAX_CUSTOM_FX);
        self.effect_descriptors.resize_with(
            crate::lisp_host::MAX_CUSTOM_FX,
            EffectDescriptor::empty_custom_slot,
        );
        self.effect_descriptors
            .truncate(crate::lisp_host::MAX_CUSTOM_FX);
        self.custom_effect_names
            .resize(crate::lisp_host::MAX_CUSTOM_FX, None);
        self.custom_effect_names
            .truncate(crate::lisp_host::MAX_CUSTOM_FX);
    }

    pub fn param_default(&self, param: RackSlotParam) -> f32 {
        match param {
            RackSlotParam::BaseNote => self.instrument_base_note_offset,
            RackSlotParam::Gain => self.gain,
            RackSlotParam::Pan => self.pan,
            RackSlotParam::MaxPolyphony => self.max_polyphony as f32,
            RackSlotParam::Mute => {
                if self.mute {
                    1.0
                } else {
                    0.0
                }
            }
            RackSlotParam::Solo => {
                if self.solo {
                    1.0
                } else {
                    0.0
                }
            }
        }
    }

    pub fn param_value_at_step(&self, param: RackSlotParam, step: usize) -> f32 {
        self.param_plocks
            .get(step, param)
            .unwrap_or_else(|| self.param_default(param))
    }

    pub fn set_param_plock(&mut self, step: usize, param: RackSlotParam, value: f32) -> bool {
        self.param_plocks.set(step, param, value)
    }
}

fn prepare_track_pattern_data_for_rack(data: &mut TrackPatternData) {
    data.instrument_type = InstrumentType::Rack;
    data.instrument_run_mode = CustomInstrumentRunMode::Instrument;
    data.instrument_slot = EffectSlotSnapshot::new_empty();
    data.instrument_base_note_offset = 0.0;
    data.track_sound_state.engine_id = None;
}

fn replace_rack_slot_source_preserving_controls(
    slot: &mut RackSlotSnapshot,
    replacement: &RackSlotSnapshot,
) {
    let pad_note = slot.pad_note;
    let choke_group = slot.choke_group;
    let instrument_base_note_offset = slot.instrument_base_note_offset;
    let gain = slot.gain;
    let pan = slot.pan;
    let mute = slot.mute;
    let solo = slot.solo;
    let max_polyphony = slot.max_polyphony;
    let param_plocks = slot.param_plocks.clone();
    let effect_slots = slot.effect_slots.clone();
    let effect_descriptors = slot.effect_descriptors.clone();
    let custom_effect_names = slot.custom_effect_names.clone();

    *slot = replacement.clone();
    slot.pad_note = pad_note;
    slot.choke_group = choke_group;
    slot.instrument_base_note_offset = instrument_base_note_offset;
    slot.gain = gain;
    slot.pan = pan;
    slot.mute = mute;
    slot.solo = solo;
    slot.max_polyphony = max_polyphony;
    slot.param_plocks = param_plocks;
    slot.effect_slots = effect_slots;
    slot.effect_descriptors = effect_descriptors;
    slot.custom_effect_names = custom_effect_names;
}

#[derive(Clone, Debug)]
pub struct TrackPatternData {
    pub track_bits: [u64; TRACK_PATTERN_WORDS],
    pub neural_reset_bits: [u64; TRACK_PATTERN_WORDS],
    pub step_data: Vec<[f32; NUM_PARAMS]>,
    pub track_params: TrackParamsSnapshot,
    pub effect_slots: Vec<EffectSlotSnapshot>,
    pub midi_fx_slots: Vec<EffectSlotSnapshot>,
    pub instrument_slot: EffectSlotSnapshot,
    pub instrument_base_note_offset: f32,
    pub track_sound_state: TrackSoundState,
    pub sample_id: (i32, String, u32),
    pub chord_snapshot: ChordSnapshot,
    pub timebase_plock_snapshot: [Option<u32>; MAX_STEPS],
    pub swing_plock_snapshot: [Option<u32>; MAX_STEPS],
    pub swing_resolution_plock_snapshot: [Option<u32>; MAX_STEPS],
    pub instrument_type: InstrumentType,
    pub instrument_run_mode: CustomInstrumentRunMode,
    pub rack_track: Option<RackTrackSnapshot>,
    pub process_chain: crate::process::TrackProcessChain,
    pub project_process_lane_overrides: crate::process::ProjectLaneOverrides,
    pub plock_variant_registry: PlockVariantRegistry,
    pub key_lock_variant_registry: PlockVariantRegistry,
}

/// Instrument-owned authoring state for one track pattern.  Structural
/// instrument replacement deliberately resets these fields; keeping them in a
/// separate snapshot lets undo restore the binding without overwriting notes,
/// timing, mixer values, or effect state edited by other operations.
#[derive(Clone, Debug)]
pub struct TrackInstrumentPatternState {
    pub instrument_slot: EffectSlotSnapshot,
    pub instrument_base_note_offset: f32,
    pub track_sound_state: TrackSoundState,
    pub sample_id: (i32, String, u32),
    pub instrument_type: InstrumentType,
    pub instrument_run_mode: CustomInstrumentRunMode,
    pub rack_track: Option<RackTrackSnapshot>,
    pub process_chain: crate::process::TrackProcessChain,
    pub project_process_lane_overrides: crate::process::ProjectLaneOverrides,
    pub plock_variant_registry: PlockVariantRegistry,
    pub key_lock_variant_registry: PlockVariantRegistry,
}

#[derive(Clone, Debug)]
pub struct NeuralInstrumentOverrideState {
    pub scene: usize,
    pub network: usize,
    pub neuron: usize,
    pub entries: Vec<(usize, crate::neural::ProjectParamOverride)>,
}

#[derive(Clone, Debug)]
pub struct NeuralEffectOverrideState {
    pub scene: usize,
    pub network: usize,
    pub neuron: usize,
    pub entries: Vec<(usize, crate::neural::ProjectEffectParamOverride)>,
}

#[derive(Clone, Debug)]
pub struct TrackEffectBindingStateSnapshot {
    pub process_chains: Vec<(PatternId, crate::process::TrackProcessChain)>,
    pub project_process_lane_overrides:
        Vec<(PatternId, crate::process::ProjectLaneOverrides)>,
    pub neural_overrides: Vec<NeuralEffectOverrideState>,
}

#[derive(Clone, Debug)]
pub struct TrackInstrumentPatternStateSnapshot {
    pub live: TrackInstrumentPatternState,
    pub patterns: Vec<(PatternId, TrackInstrumentPatternState)>,
    pub neural_overrides: Vec<NeuralInstrumentOverrideState>,
}

#[derive(Clone, Debug)]
pub struct RackSlotPatternStateSnapshot {
    pub slot_index: usize,
    pub live: RackSlotSnapshot,
    pub patterns: Vec<(PatternId, RackSlotSnapshot)>,
}

#[derive(Clone, Debug)]
pub struct RackMacroPatternStateSnapshot {
    pub live: Vec<RackMacro>,
    pub patterns: Vec<(PatternId, Vec<RackMacro>)>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct InstrumentSlotResetSummary {
    pub patterns_reset: usize,
    pub patterns_with_cleared_locks: usize,
    pub process_bindings_dropped: usize,
    pub neural_overrides_dropped: usize,
}

enum InstrumentSourceReset {
    Custom {
        engine_id: usize,
        run_mode: CustomInstrumentRunMode,
    },
    Sampler {
        sample_id: (i32, String, u32),
    },
}

impl InstrumentSourceReset {
    fn instrument_type(&self) -> InstrumentType {
        match self {
            Self::Custom { .. } => InstrumentType::Custom,
            Self::Sampler { .. } => InstrumentType::Sampler,
        }
    }

    fn run_mode(&self) -> CustomInstrumentRunMode {
        match self {
            Self::Custom { run_mode, .. } => *run_mode,
            Self::Sampler { .. } => CustomInstrumentRunMode::Instrument,
        }
    }

    fn engine_id(&self) -> Option<usize> {
        match self {
            Self::Custom { engine_id, .. } => Some(*engine_id),
            Self::Sampler { .. } => None,
        }
    }

    fn sample_id(&self) -> (i32, String, u32) {
        match self {
            Self::Custom { .. } => (-1, String::new(), 44_100),
            Self::Sampler { sample_id } => sample_id.clone(),
        }
    }
}

const INSTRUMENT_PLOCK_VARIANT_DOMAINS: &[PlockVariantDomain] = &[
    PlockVariantDomain::Instrument,
    PlockVariantDomain::InstrumentTensor,
    PlockVariantDomain::InstrumentKeyLock,
];

fn instrument_slot_has_locks(slot: &EffectSlotSnapshot) -> bool {
    slot.plocks
        .iter()
        .any(|row| row.iter().any(Option::is_some))
        || slot
            .key_locks
            .values()
            .any(|row| row.iter().any(Option::is_some))
        || slot
            .tensor_params
            .iter()
            .any(|tensor| tensor.plocks.iter().any(Option::is_some))
}

impl TrackPatternData {
    fn instrument_state(&self) -> TrackInstrumentPatternState {
        TrackInstrumentPatternState {
            instrument_slot: self.instrument_slot.clone(),
            instrument_base_note_offset: self.instrument_base_note_offset,
            track_sound_state: self.track_sound_state.clone(),
            sample_id: self.sample_id.clone(),
            instrument_type: self.instrument_type,
            instrument_run_mode: self.instrument_run_mode,
            rack_track: self.rack_track.clone(),
            process_chain: self.process_chain.clone(),
            project_process_lane_overrides: self.project_process_lane_overrides.clone(),
            plock_variant_registry: self.plock_variant_registry.clone(),
            key_lock_variant_registry: self.key_lock_variant_registry.clone(),
        }
    }

    fn restore_instrument_state(
        &mut self,
        state: &TrackInstrumentPatternState,
        descriptor: &EffectDescriptor,
        node_id: u32,
        modulator_node_id: u32,
    ) {
        let mut instrument_slot = state.instrument_slot.clone();
        instrument_slot.sync_to_descriptor_with_modulator(
            descriptor,
            node_id,
            modulator_node_id,
        );
        self.instrument_slot = instrument_slot;
        self.instrument_base_note_offset = state.instrument_base_note_offset;
        self.track_sound_state = state.track_sound_state.clone();
        self.sample_id = state.sample_id.clone();
        self.instrument_type = state.instrument_type;
        self.instrument_run_mode = state.instrument_run_mode;
        self.rack_track = state.rack_track.clone();
        self.process_chain = state.process_chain.clone();
        crate::process::rebind_track_process_chain_instrument_param_ids(
            &mut self.process_chain,
            descriptor,
            &self.instrument_slot,
        );
        self.project_process_lane_overrides = state.project_process_lane_overrides.clone();
        self.plock_variant_registry = state.plock_variant_registry.clone();
        self.key_lock_variant_registry = state.key_lock_variant_registry.clone();
    }

    fn capture_step_snapshot(&self, step: usize) -> Option<StepSnapshot> {
        if step >= MAX_STEPS {
            return None;
        }
        let word = step / 64;
        let bit = step % 64;
        let params = *self.step_data.get(step)?;
        let rack = self.rack_track.as_ref();

        Some(StepSnapshot {
            active: (self.track_bits[word] >> bit) & 1 == 1,
            neural_reset: (self.neural_reset_bits[word] >> bit) & 1 == 1,
            params,
            chord: self.chord_snapshot.steps.get(step)?.clone(),
            chord_durations: self.chord_snapshot.durations.get(step)?.clone(),
            chord_delays: self.chord_snapshot.delays.get(step)?.clone(),
            timebase: self.timebase_plock_snapshot[step].map(Timebase::from_index),
            swing: self.swing_plock_snapshot[step].map(f32::from_bits),
            swing_resolution: self.swing_resolution_plock_snapshot[step]
                .map(SwingResolution::from_index),
            midi_fx_plocks: self
                .midi_fx_slots
                .iter()
                .map(|slot| capture_snapshot_slot_step_plocks(slot, step))
                .collect(),
            effect_plocks: self
                .effect_slots
                .iter()
                .map(|slot| capture_snapshot_slot_step_plocks(slot, step))
                .collect(),
            instrument_plocks: capture_snapshot_slot_step_plocks(
                &self.instrument_slot,
                step,
            ),
            rack_macro_plocks: rack
                .map(|rack| {
                    rack.macros
                        .iter()
                        .map(|rack_macro| rack_macro.plocks[step])
                        .collect()
                })
                .unwrap_or_default(),
            rack_slot_param_plocks: rack
                .map(|rack| {
                    rack.slots
                        .iter()
                        .map(|slot| StepSlotPlocks {
                            params: RackSlotParam::ALL
                                .iter()
                                .map(|param| slot.param_plocks.get(step, *param))
                                .collect(),
                            tensor_params: Vec::new(),
                        })
                        .collect()
                })
                .unwrap_or_default(),
            rack_slot_instrument_plocks: rack
                .map(|rack| {
                    rack.slots
                        .iter()
                        .map(|slot| {
                            capture_snapshot_slot_step_plocks(&slot.instrument_slot, step)
                        })
                        .collect()
                })
                .unwrap_or_default(),
            rack_slot_effect_plocks: rack
                .map(|rack| {
                    rack.slots
                        .iter()
                        .map(|slot| {
                            slot.effect_slots
                                .iter()
                                .map(|effect| {
                                    capture_snapshot_slot_step_plocks(effect, step)
                                })
                                .collect()
                        })
                        .collect()
                })
                .unwrap_or_default(),
        })
    }

    fn restore_step_snapshot(&mut self, step: usize, snapshot: &StepSnapshot) -> bool {
        if step >= MAX_STEPS
            || step >= self.step_data.len()
            || step >= self.chord_snapshot.steps.len()
            || step >= self.chord_snapshot.durations.len()
            || step >= self.chord_snapshot.delays.len()
        {
            return false;
        }
        let word = step / 64;
        let mask = 1u64 << (step % 64);
        if snapshot.active {
            self.track_bits[word] |= mask;
        } else {
            self.track_bits[word] &= !mask;
        }
        if snapshot.neural_reset {
            self.neural_reset_bits[word] |= mask;
        } else {
            self.neural_reset_bits[word] &= !mask;
        }
        self.step_data[step] = snapshot.params;

        self.chord_snapshot.steps[step] = snapshot.chord.clone();
        self.chord_snapshot.durations[step] = snapshot.chord_durations.clone();
        self.chord_snapshot.delays[step] = snapshot.chord_delays.clone();
        self.timebase_plock_snapshot[step] = snapshot.timebase.map(|value| value as u32);
        self.swing_plock_snapshot[step] = snapshot.swing.map(f32::to_bits);
        self.swing_resolution_plock_snapshot[step] =
            snapshot.swing_resolution.map(|value| value as u32);

        for (slot_idx, slot) in self.midi_fx_slots.iter_mut().enumerate() {
            restore_snapshot_slot_step_plocks(slot, step, snapshot.midi_fx_plocks.get(slot_idx));
        }
        for (slot_idx, slot) in self.effect_slots.iter_mut().enumerate() {
            restore_snapshot_slot_step_plocks(slot, step, snapshot.effect_plocks.get(slot_idx));
        }
        restore_snapshot_slot_step_plocks(
            &mut self.instrument_slot,
            step,
            Some(&snapshot.instrument_plocks),
        );

        if let Some(rack) = self.rack_track.as_mut() {
            for (macro_idx, rack_macro) in rack.macros.iter_mut().enumerate() {
                rack_macro.plocks[step] = snapshot
                    .rack_macro_plocks
                    .get(macro_idx)
                    .copied()
                    .flatten();
            }
            for (slot_idx, slot) in rack.slots.iter_mut().enumerate() {
                let saved_params = snapshot.rack_slot_param_plocks.get(slot_idx);
                for param in RackSlotParam::ALL {
                    match saved_params
                        .and_then(|plocks| plocks.params.get(param.index()))
                        .copied()
                        .flatten()
                    {
                        Some(value) => slot.param_plocks.set(step, param, value),
                        None => slot.param_plocks.clear(step, param),
                    };
                }
                restore_snapshot_slot_step_plocks(
                    &mut slot.instrument_slot,
                    step,
                    snapshot.rack_slot_instrument_plocks.get(slot_idx),
                );
                for (effect_idx, effect) in slot.effect_slots.iter_mut().enumerate() {
                    restore_snapshot_slot_step_plocks(
                        effect,
                        step,
                        snapshot
                            .rack_slot_effect_plocks
                            .get(slot_idx)
                            .and_then(|effects| effects.get(effect_idx)),
                    );
                }
            }
        }
        true
    }

    fn reset_instrument_source(
        &mut self,
        descriptor: &EffectDescriptor,
        node_id: u32,
        modulator_node_id: u32,
        source: &InstrumentSourceReset,
    ) -> (bool, usize) {
        let cleared_locks = instrument_slot_has_locks(&self.instrument_slot);
        self.instrument_slot =
            EffectSlotSnapshot::new_default_with_modulator(descriptor, node_id, modulator_node_id);
        self.track_sound_state = TrackSoundState {
            engine_id: source.engine_id(),
            loaded_preset: None,
            dirty: false,
        };
        self.sample_id = source.sample_id();
        self.instrument_type = source.instrument_type();
        self.instrument_run_mode = source.run_mode();
        self.rack_track = None;
        self.plock_variant_registry
            .remove_domains(INSTRUMENT_PLOCK_VARIANT_DOMAINS);
        self.key_lock_variant_registry
            .remove_domains(INSTRUMENT_PLOCK_VARIANT_DOMAINS);
        let dropped_bindings = crate::process::rebind_track_process_chain_instrument_param_ids(
            &mut self.process_chain,
            descriptor,
            &self.instrument_slot,
        );
        (cleared_locks, dropped_bindings)
    }

    fn refresh_process_effect_binding_param_ids_for_slot(
        &mut self,
        slot_idx: usize,
        descriptor: &EffectDescriptor,
    ) {
        let Some(effect_slot) = self.effect_slots.get(slot_idx) else {
            return;
        };
        crate::process::refresh_track_process_chain_effect_binding_param_ids_for_slot(
            &mut self.process_chain,
            slot_idx,
            descriptor,
            effect_slot,
        );
    }

    pub(crate) fn refreshed_process_chain(
        &self,
        instrument_descriptor: Option<&EffectDescriptor>,
        effect_descriptors: &[EffectDescriptor],
    ) -> crate::process::TrackProcessChain {
        let mut process_chain = self.process_chain.clone();
        crate::process::refresh_track_process_chain_binding_param_ids(
            &mut process_chain,
            instrument_descriptor,
            Some(&self.instrument_slot),
            effect_descriptors,
            &self.effect_slots,
        );
        process_chain
    }

    fn restore_to(&self, state: &SequencerState, track: usize) -> bool {
        if track >= state.pattern.patterns.len()
            || track >= state.pattern.neural_reset_patterns.len()
            || track >= state.pattern.step_data.len()
            || track >= state.pattern.track_params.len()
            || track >= state.pattern.effect_chains.len()
            || track >= state.pattern.midi_fx_slots.len()
            || track >= state.pattern.instrument_slots.len()
            || track >= state.pattern.instrument_base_note_offsets.len()
            || track >= state.pattern.instrument_run_modes.len()
            || track >= state.runtime.instrument_run_mode_flags.len()
            || track >= state.pattern.chord_data.len()
            || track >= state.pattern.timebase_plocks.len()
            || track >= state.pattern.swing_plocks.len()
            || track >= state.pattern.swing_resolution_plocks.len()
            || track >= state.pattern.process_chains.lock().unwrap().len()
            || track >= state.pattern.plock_variant_registries.lock().unwrap().len()
            || track
                >= state
                    .pattern
                    .key_lock_variant_registries
                    .lock()
                    .unwrap()
                    .len()
        {
            return false;
        }

        state.pattern.patterns[track].store_bits(self.track_bits);
        state.pattern.neural_reset_patterns[track].store_bits(self.neural_reset_bits);

        state.pattern.step_data[track].store_rows_clamped(&self.step_data);

        let tp = &state.pattern.track_params[track];
        let snap = &self.track_params;
        restore_track_params_snapshot(tp, snap);

        for (slot_idx, slot_snap) in self.effect_slots.iter().enumerate() {
            if slot_idx < state.pattern.effect_chains[track].len() {
                slot_snap.restore(&state.pattern.effect_chains[track][slot_idx]);
            }
        }
        for (slot_idx, slot_snap) in self.midi_fx_slots.iter().enumerate() {
            if slot_idx < state.pattern.midi_fx_slots[track].len() {
                slot_snap.restore(&state.pattern.midi_fx_slots[track][slot_idx]);
            }
        }

        self.instrument_slot
            .restore(&state.pattern.instrument_slots[track]);
        state.pattern.instrument_base_note_offsets[track].store(
            self.instrument_base_note_offset.to_bits(),
            Ordering::Relaxed,
        );
        state.pattern.instrument_run_modes[track]
            .store(self.instrument_run_mode.runtime_flag(), Ordering::Relaxed);
        state.runtime.instrument_run_mode_flags[track]
            .store(self.instrument_run_mode.runtime_flag(), Ordering::Relaxed);

        {
            let mut track_sound_state = state.pattern.track_sound_state.lock().unwrap();
            if track < track_sound_state.len() {
                track_sound_state[track] = self.track_sound_state.clone();
            }
        }
        {
            let mut rack_tracks = state.pattern.rack_tracks.lock().unwrap();
            if track < rack_tracks.len() {
                rack_tracks[track] = self.rack_track.clone();
            }
        }
        let refreshed_process_chain = {
            let effect_descriptors = state.scratch_effect_descriptors.lock().unwrap();
            let instrument_descriptors = state.scratch_instrument_descriptors.lock().unwrap();
            self.refreshed_process_chain(
                instrument_descriptors.get(track),
                effect_descriptors
                    .get(track)
                    .map(Vec::as_slice)
                    .unwrap_or(&[]),
            )
        };
        {
            let mut process_chains = state.pattern.process_chains.lock().unwrap();
            if track < process_chains.len() {
                process_chains[track] = refreshed_process_chain;
            }
        }
        {
            let mut overrides = state.pattern.project_process_lane_overrides.lock().unwrap();
            if track < overrides.len() {
                overrides[track] = self.project_process_lane_overrides.clone();
            }
        }
        {
            let mut registries = state.pattern.plock_variant_registries.lock().unwrap();
            if track < registries.len() {
                registries[track] = self.plock_variant_registry.clone();
            }
        }
        let active_variant_keys = live_track_variant_keys(state, track);
        if let Some(registry) = state
            .pattern
            .plock_variant_registries
            .lock()
            .unwrap()
            .get_mut(track)
        {
            registry.prune_to_keys(&active_variant_keys);
        }
        {
            let mut registries = state.pattern.key_lock_variant_registries.lock().unwrap();
            if track < registries.len() {
                registries[track] = self.key_lock_variant_registry.clone();
            }
        }
        let active_key_lock_variant_keys = live_track_key_lock_variant_keys(state, track);
        if let Some(registry) = state
            .pattern
            .key_lock_variant_registries
            .lock()
            .unwrap()
            .get_mut(track)
        {
            registry.prune_to_keys(&active_key_lock_variant_keys);
        }

        self.chord_snapshot
            .restore(&state.pattern.chord_data[track]);
        state.pattern.timebase_plocks[track].restore(&self.timebase_plock_snapshot);
        state.pattern.swing_plocks[track].restore(&self.swing_plock_snapshot);
        state.pattern.swing_resolution_plocks[track].restore(&self.swing_resolution_plock_snapshot);

        true
    }

    fn remove_effect_slot(&mut self, slot_idx: usize) {
        if slot_idx >= self.effect_slots.len() {
            return;
        }
        for idx in slot_idx..self.effect_slots.len().saturating_sub(1) {
            self.effect_slots[idx] = self.effect_slots[idx + 1].clone();
        }
        if let Some(last) = self.effect_slots.last_mut() {
            last.clear();
        }
    }

    fn insert_empty_effect_slot(&mut self, slot_idx: usize) {
        if slot_idx >= self.effect_slots.len() {
            return;
        }
        for idx in (slot_idx + 1..self.effect_slots.len()).rev() {
            self.effect_slots[idx] = self.effect_slots[idx - 1].clone();
        }
        self.effect_slots[slot_idx].clear();
    }

    fn move_effect_slot_to(&mut self, source_slot: usize, target_slot: usize) {
        if source_slot >= self.effect_slots.len()
            || target_slot >= self.effect_slots.len()
            || source_slot == target_slot
        {
            return;
        }
        let entry = self.effect_slots.remove(source_slot);
        self.effect_slots.insert(target_slot, entry);
        while self.effect_slots.len() <= target_slot.max(source_slot) {
            self.effect_slots.push(EffectSlotSnapshot::new_empty());
        }
    }

    fn sync_effect_slot_with_modulator(
        &mut self,
        slot_idx: usize,
        desc: &EffectDescriptor,
        node_id: u32,
        modulator_node_id: u32,
    ) {
        while self.effect_slots.len() <= slot_idx {
            self.effect_slots.push(EffectSlotSnapshot::new_empty());
        }
        self.effect_slots[slot_idx].sync_to_descriptor_with_modulator(
            desc,
            node_id,
            modulator_node_id,
        );
    }

    fn remap_sidechain_references_after_track_delete(
        &mut self,
        owner_track_old: usize,
        effect_descriptors: &[EffectDescriptor],
        deleted_track: usize,
        old_track_count: usize,
    ) {
        for (slot_idx, slot) in self.effect_slots.iter_mut().enumerate() {
            let Some(desc) = effect_descriptors.get(slot_idx) else {
                continue;
            };
            let num_params = slot.num_params as usize;
            for param_idx in 0..num_params.min(desc.params.len()) {
                if !matches!(
                    desc.params[param_idx].host_control,
                    Some(HostControl::FxSidechain { .. })
                ) {
                    continue;
                }
                if param_idx < slot.defaults.len() {
                    slot.defaults[param_idx] = remap_sidechain_selection_after_track_delete(
                        owner_track_old,
                        slot.defaults[param_idx].round().max(0.0) as usize,
                        deleted_track,
                        old_track_count,
                    ) as f32;
                }
                for step in 0..MAX_STEPS {
                    let selection = slot.plocks.get(step)
                        .and_then(|params| params.get(param_idx))
                        .and_then(|value| *value);
                    if let (Some(selection), Some(value)) = (
                        selection,
                        slot.plocks.get_mut(step).and_then(|params| params.get_mut(param_idx)),
                    ) {
                        *value = Some(remap_sidechain_selection_after_track_delete(
                            owner_track_old,
                            selection.round().max(0.0) as usize,
                            deleted_track,
                            old_track_count,
                        ) as f32);
                    }
                }
            }
        }
    }

    fn remove_midi_fx_slot(&mut self, slot_idx: usize) {
        if slot_idx < self.track_params.midi_fx_chain.len() {
            self.track_params.midi_fx_chain.remove(slot_idx);
        }
        if slot_idx >= self.midi_fx_slots.len() {
            return;
        }
        for idx in slot_idx..self.midi_fx_slots.len().saturating_sub(1) {
            self.midi_fx_slots[idx] = self.midi_fx_slots[idx + 1].clone();
        }
        if let Some(last) = self.midi_fx_slots.last_mut() {
            last.clear();
        }
    }

    fn insert_midi_fx_slot(&mut self, slot_idx: usize, name: String, desc: &EffectDescriptor) {
        let insert_idx = slot_idx.min(self.track_params.midi_fx_chain.len());
        self.track_params.midi_fx_chain.insert(insert_idx, name);
        if insert_idx >= self.midi_fx_slots.len() {
            return;
        }
        for idx in (insert_idx + 1..self.midi_fx_slots.len()).rev() {
            self.midi_fx_slots[idx] = self.midi_fx_slots[idx - 1].clone();
        }
        self.midi_fx_slots[insert_idx].sync_to_descriptor(desc, 0);
    }

    fn move_midi_fx_slot_to(&mut self, source_slot: usize, target_slot: usize) {
        if source_slot >= self.track_params.midi_fx_chain.len() {
            return;
        }
        let target_slot = target_slot.min(self.track_params.midi_fx_chain.len().saturating_sub(1));
        if source_slot == target_slot {
            return;
        }
        let name = self.track_params.midi_fx_chain.remove(source_slot);
        self.track_params.midi_fx_chain.insert(target_slot, name);
        if source_slot >= self.midi_fx_slots.len() || target_slot >= self.midi_fx_slots.len() {
            return;
        }
        let entry = self.midi_fx_slots.remove(source_slot);
        self.midi_fx_slots.insert(target_slot, entry);
        while self.midi_fx_slots.len() <= target_slot.max(source_slot) {
            self.midi_fx_slots.push(EffectSlotSnapshot::new_empty());
        }
    }

    fn clear(
        &mut self,
        track: usize,
        slot_descriptors: &[Vec<EffectDescriptor>],
        instrument_type: InstrumentType,
    ) {
        self.track_bits = [0u64; TRACK_PATTERN_WORDS];
        self.neural_reset_bits = [0u64; TRACK_PATTERN_WORDS];
        self.step_data = PatternSnapshot::default_step_data();
        self.track_params = TrackParamsSnapshot::default();
        self.effect_slots = PatternSnapshot::default_effect_slots(track, slot_descriptors);
        self.midi_fx_slots = PatternSnapshot::default_midi_fx_slots();
        self.instrument_slot = PatternSnapshot::default_instrument_slot();
        self.instrument_base_note_offset = 0.0;
        self.track_sound_state = TrackSoundState::default();
        self.sample_id = (-1, String::new(), 44_100);
        self.chord_snapshot = ChordSnapshot::new_default();
        self.timebase_plock_snapshot = [None; MAX_STEPS];
        self.swing_plock_snapshot = [None; MAX_STEPS];
        self.swing_resolution_plock_snapshot = [None; MAX_STEPS];
        self.instrument_type = instrument_type;
        self.instrument_run_mode = CustomInstrumentRunMode::Instrument;
        self.rack_track = None;
        self.plock_variant_registry = PlockVariantRegistry::default();
    }

    fn default_step_params() -> [f32; NUM_PARAMS] {
        let mut params = [0.0f32; NUM_PARAMS];
        for param in StepParam::ALL {
            params[param.index()] = param.default_value();
        }
        params
    }
}

#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
pub struct PatternId(pub u64);

/// Stable logical identity for a project scene.
///
/// Scene indices are presentation order and can change when scenes are
/// inserted, deleted, or reordered. Long-lived authoring references use this
/// identity instead.
#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq, PartialOrd, Ord)]
pub struct SceneId(pub u64);

#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
pub struct TrackPatternId {
    pub track: TrackId,
    pub pattern: PatternId,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TrackPatternCellView {
    pub pattern_id: PatternId,
    pub assigned_to_current_scene: bool,
    pub active_effective: bool,
    pub overridden: bool,
}

#[derive(Clone, Debug)]
pub struct TrackPatternPool {
    pub patterns: HashMap<PatternId, TrackPatternData>,
    pub next_id: u64,
}

#[derive(Clone, Debug)]
pub struct SceneTrackReferenceState {
    pub mod_connections: Vec<ModConnection>,
    pub neural_networks: Vec<ProjectNeuralNetwork>,
    pub graph_overrides: Vec<ProjectGraphOverrides>,
}

#[derive(Clone, Debug)]
pub struct TrackSidechainPatternState {
    pub owner_track: usize,
    pub pattern: PatternId,
    pub slots: Vec<(usize, EffectSlotSnapshot)>,
}

#[derive(Clone, Debug)]
pub struct TrackPatternLaneState {
    pub pool: TrackPatternPool,
    pub scene_cells: Vec<Option<PatternId>>,
    pub track_override: Option<PatternId>,
    pub scene_references: Vec<SceneTrackReferenceState>,
    pub sidechains: Vec<TrackSidechainPatternState>,
}

impl Default for TrackPatternPool {
    fn default() -> Self {
        Self {
            patterns: HashMap::new(),
            // Reserve 0 for atomic/sentinel uses; real track pattern ids start at 1.
            next_id: 1,
        }
    }
}

impl TrackPatternPool {
    pub fn insert(&mut self, data: TrackPatternData) -> PatternId {
        let id = PatternId(self.next_id.max(1));
        self.next_id = id.0.saturating_add(1).max(1);
        self.patterns.insert(id, data);
        id
    }

    pub fn contains(&self, id: PatternId) -> bool {
        self.patterns.contains_key(&id)
    }

    pub fn get(&self, id: PatternId) -> Option<&TrackPatternData> {
        self.patterns.get(&id)
    }

    pub fn get_mut(&mut self, id: PatternId) -> Option<&mut TrackPatternData> {
        self.patterns.get_mut(&id)
    }

    pub fn remove(&mut self, id: PatternId) -> Option<TrackPatternData> {
        self.patterns.remove(&id)
    }
}

#[derive(Clone, Debug)]
pub struct Scene {
    pub id: SceneId,
    pub name: String,
    pub cells: Vec<Option<PatternId>>,
    pub bus_patterns: Vec<BusPatternSnapshot>,
    // These are scene-level because per-track launches must not swap project-wide
    // modulation, neural, or graph routing state.
    pub mod_connections: Vec<ModConnection>,
    pub neural_networks: Vec<ProjectNeuralNetwork>,
    pub graph_overrides: Vec<ProjectGraphOverrides>,
    /// Project-level default process chain: composed ahead of every track's
    /// own chain at snapshot capture, so present and future tracks inherit it.
    pub project_process_chain: crate::process::TrackProcessChain,
}

#[derive(Clone, Debug)]
pub struct ProjectScenes {
    pub track_pools: Vec<TrackPatternPool>,
    pub scenes: Vec<Scene>,
    pub current_scene: usize,
    pub track_overrides: Vec<Option<PatternId>>,
    next_scene_id: u64,
}

impl ProjectScenes {
    pub fn from_pattern_snapshots(
        snapshots: &[PatternSnapshot],
        current_scene: usize,
    ) -> ProjectScenes {
        let track_count = snapshots
            .iter()
            .map(|snapshot| snapshot.track_bits.len())
            .max()
            .unwrap_or(0);
        let mut track_pools = vec![TrackPatternPool::default(); track_count];
        let mut scenes = Vec::with_capacity(snapshots.len().max(1));

        for (scene_idx, snapshot) in snapshots.iter().enumerate() {
            let mut cells = vec![None; track_count];
            for track in 0..track_count {
                if let Some(data) = snapshot.track_pattern_data(track) {
                    cells[track] = Some(track_pools[track].insert(data));
                }
            }
            scenes.push(Scene {
                id: SceneId(scene_idx as u64 + 1),
                name: format!("Scene {}", scene_idx + 1),
                cells,
                bus_patterns: Vec::new(),
                mod_connections: snapshot.mod_connections.clone(),
                neural_networks: snapshot.neural_networks.clone(),
                graph_overrides: snapshot.graph_overrides.clone(),
                project_process_chain: snapshot.project_process_chain.clone(),
            });
        }

        if scenes.is_empty() {
            scenes.push(Scene {
                id: SceneId(1),
                name: "Scene 1".to_string(),
                cells: vec![None; track_count],
                bus_patterns: Vec::new(),
                mod_connections: Vec::new(),
                neural_networks: Vec::new(),
                graph_overrides: Vec::new(),
                project_process_chain: crate::process::TrackProcessChain::default(),
            });
        }

        Self {
            track_pools,
            scenes,
            current_scene: current_scene.min(snapshots.len().saturating_sub(1)),
            track_overrides: vec![None; track_count],
            next_scene_id: u64::try_from(snapshots.len().max(1))
                .expect("scene count exceeds stable identity space")
                .checked_add(1)
                .expect("scene identity space exhausted"),
        }
    }

    pub fn scene_count(&self) -> usize {
        self.scenes.len().max(1)
    }

    pub fn scene_id(&self, scene_idx: usize) -> Option<SceneId> {
        self.scenes.get(scene_idx).map(|scene| scene.id)
    }

    pub fn scene_index(&self, id: SceneId) -> Option<usize> {
        self.scenes.iter().position(|scene| scene.id == id)
    }

    /// Sample ids for a scene without cloning the full track pattern data.
    pub fn scene_sample_ids(&self, scene_idx: usize) -> Option<Vec<(i32, String, u32)>> {
        let scene = self.scenes.get(scene_idx)?;
        Some(
            (0..self.track_pools.len())
                .map(|track| {
                    scene
                        .cells
                        .get(track)
                        .copied()
                        .flatten()
                        .and_then(|id| self.track_pools[track].get(id))
                        .map(|data| data.sample_id.clone())
                        .unwrap_or((-1, String::new(), 44_100))
                })
                .collect(),
        )
    }

    /// Metadata-only view of a scene (no track pattern data is cloned).
    pub fn scene_metadata(
        &self,
        scene_idx: usize,
    ) -> Option<(
        Vec<ModConnection>,
        Vec<ProjectNeuralNetwork>,
        Vec<ProjectGraphOverrides>,
    )> {
        let scene = self.scenes.get(scene_idx)?;
        Some((
            scene.mod_connections.clone(),
            scene.neural_networks.clone(),
            scene.graph_overrides.clone(),
        ))
    }

    pub fn scene_snapshot(&self, scene_idx: usize) -> Option<PatternSnapshot> {
        let scene = self.scenes.get(scene_idx)?;
        let mut snapshot = PatternSnapshot::new_default(self.track_pools.len(), &[]);
        snapshot.mod_connections = scene.mod_connections.clone();
        snapshot.neural_networks = scene.neural_networks.clone();
        snapshot.graph_overrides = scene.graph_overrides.clone();
        snapshot.project_process_chain = scene.project_process_chain.clone();
        for track in 0..self.track_pools.len() {
            let Some(id) = scene.cells.get(track).copied().flatten() else {
                continue;
            };
            let Some(data) = self.track_pools[track].get(id).cloned() else {
                continue;
            };
            snapshot.set_track_pattern_data(track, data);
        }
        Some(snapshot)
    }

    pub fn snapshots(&self) -> Vec<PatternSnapshot> {
        (0..self.scenes.len())
            .filter_map(|scene_idx| self.scene_snapshot(scene_idx))
            .collect()
    }

    pub fn save_scene_snapshot(&mut self, scene_idx: usize, snapshot: PatternSnapshot) -> bool {
        while self.track_pools.len() < snapshot.track_bits.len() {
            self.track_pools.push(TrackPatternPool::default());
            self.track_overrides.push(None);
            for scene in &mut self.scenes {
                scene.cells.push(None);
            }
        }
        let Some(scene) = self.scenes.get_mut(scene_idx) else {
            return false;
        };
        while scene.cells.len() < snapshot.track_bits.len() {
            scene.cells.push(None);
        }
        scene.mod_connections = snapshot.mod_connections.clone();
        scene.neural_networks = snapshot.neural_networks.clone();
        scene.graph_overrides = snapshot.graph_overrides.clone();
        // Deliberately NOT copied from the snapshot: the scene itself is the
        // live authority for `project_process_chain` (edited in place via
        // edit_current_project_process_chain), and several callers save
        // snapshots that never carried it. Snapshot→scene transfer happens
        // only in from_pattern_snapshots (project load).

        for track in 0..snapshot.track_bits.len() {
            let Some(data) = snapshot.track_pattern_data(track) else {
                continue;
            };
            let Some(id) = self
                .track_overrides
                .get(track)
                .copied()
                .flatten()
                .or_else(|| scene.cells.get(track).copied().flatten())
                .filter(|id| self.track_pools[track].contains(*id))
            else {
                continue;
            };
            if let Some(slot) = self.track_pools[track].get_mut(id) {
                *slot = data;
            }
        }
        true
    }

    pub fn delete_scene(&mut self, scene_idx: usize) -> Option<usize> {
        if self.scenes.len() <= 1 || scene_idx >= self.scenes.len() {
            return None;
        }
        self.scenes.remove(scene_idx);
        let new_idx = scene_idx.min(self.scenes.len() - 1);
        self.current_scene = new_idx;
        self.track_overrides.fill(None);
        Some(new_idx)
    }

    /// Move one scene to another position without modifying any track pattern
    /// pool. Scene cells contain stable pattern ids, so moving the scene itself
    /// preserves every track's pattern identity and data.
    pub fn reorder_scene(&mut self, source: usize, target: usize) -> Option<usize> {
        if source >= self.scenes.len() || target >= self.scenes.len() {
            return None;
        }
        if source == target {
            return Some(self.current_scene);
        }

        let scene = self.scenes.remove(source);
        self.scenes.insert(target, scene);
        self.current_scene = if self.current_scene == source {
            target
        } else if source < self.current_scene && self.current_scene <= target {
            self.current_scene - 1
        } else if target <= self.current_scene && self.current_scene < source {
            self.current_scene + 1
        } else {
            self.current_scene
        };
        Some(self.current_scene)
    }

    pub fn current_scene_metadata(
        &self,
    ) -> (
        Vec<ModConnection>,
        Vec<ProjectNeuralNetwork>,
        Vec<ProjectGraphOverrides>,
    ) {
        self.scenes
            .get(self.current_scene)
            .map(|scene| {
                (
                    scene.mod_connections.clone(),
                    scene.neural_networks.clone(),
                    scene.graph_overrides.clone(),
                )
            })
            .unwrap_or_default()
    }

    pub fn edit_current_mod_connections<F, R>(&mut self, edit: F) -> Result<R, String>
    where
        F: FnOnce(&mut Vec<ModConnection>) -> Result<R, String>,
    {
        let scene = self
            .scenes
            .get_mut(self.current_scene)
            .ok_or_else(|| "current scene out of range".to_string())?;
        edit(&mut scene.mod_connections)
    }

    pub fn current_neural_networks(&self) -> Vec<ProjectNeuralNetwork> {
        self.scenes
            .get(self.current_scene)
            .map(|scene| scene.neural_networks.clone())
            .unwrap_or_default()
    }

    pub fn edit_current_neural_networks<F, R>(&mut self, edit: F) -> Result<R, String>
    where
        F: FnOnce(&mut Vec<ProjectNeuralNetwork>) -> Result<R, String>,
    {
        let scene = self
            .scenes
            .get_mut(self.current_scene)
            .ok_or_else(|| "current scene out of range".to_string())?;
        edit(&mut scene.neural_networks)
    }

    pub fn current_graph_overrides(&self) -> Vec<ProjectGraphOverrides> {
        self.scenes
            .get(self.current_scene)
            .map(|scene| scene.graph_overrides.clone())
            .unwrap_or_default()
    }

    pub fn current_project_process_chain(&self) -> crate::process::TrackProcessChain {
        self.scenes
            .get(self.current_scene)
            .map(|scene| scene.project_process_chain.clone())
            .unwrap_or_default()
    }

    pub fn edit_current_project_process_chain<F, R>(&mut self, edit: F) -> Result<R, String>
    where
        F: FnOnce(&mut crate::process::TrackProcessChain) -> Result<R, String>,
    {
        let scene = self
            .scenes
            .get_mut(self.current_scene)
            .ok_or_else(|| "current scene out of range".to_string())?;
        edit(&mut scene.project_process_chain)
    }

    pub fn edit_current_graph_overrides<F, R>(&mut self, edit: F) -> Result<R, String>
    where
        F: FnOnce(&mut Vec<ProjectGraphOverrides>) -> Result<R, String>,
    {
        let scene = self
            .scenes
            .get_mut(self.current_scene)
            .ok_or_else(|| "current scene out of range".to_string())?;
        edit(&mut scene.graph_overrides)
    }

    pub fn effective_pattern_id(&self, track: usize) -> Option<PatternId> {
        self.track_overrides
            .get(track)
            .copied()
            .flatten()
            .or_else(|| {
                self.scenes
                    .get(self.current_scene)
                    .and_then(|scene| scene.cells.get(track))
                    .copied()
                    .flatten()
            })
    }

    pub fn effective_track_pattern(&self, track: usize) -> Option<&TrackPatternData> {
        let id = self.effective_pattern_id(track)?;
        self.track_pools.get(track)?.get(id)
    }

    pub fn save_effective_track_pattern(&mut self, track: usize, data: TrackPatternData) -> bool {
        let Some(id) = self.effective_pattern_id(track) else {
            return false;
        };
        let Some(slot) = self
            .track_pools
            .get_mut(track)
            .and_then(|pool| pool.get_mut(id))
        else {
            return false;
        };
        *slot = data;
        true
    }

    pub fn launch_scene(&mut self, scene: usize) -> Option<Vec<Option<TrackPatternData>>> {
        let scene_cells = self.scenes.get(scene)?.cells.clone();
        let mut track_patterns = Vec::with_capacity(scene_cells.len());
        for (track, cell) in scene_cells.iter().copied().enumerate() {
            let data = match cell {
                Some(id) => Some(self.track_pools.get(track)?.get(id)?.clone()),
                None => None,
            };
            track_patterns.push(data);
        }

        self.current_scene = scene;
        self.track_overrides.fill(None);
        Some(track_patterns)
    }

    pub fn launch_track_pattern(
        &mut self,
        track: usize,
        id: PatternId,
    ) -> Option<TrackPatternData> {
        let data = self.track_pools.get(track)?.get(id)?.clone();
        *self.track_overrides.get_mut(track)? = Some(id);
        Some(data)
    }

    /// Resolve every selected cell before changing any override. This keeps a
    /// masked scene launch atomic when a scene contains a stale or empty cell.
    pub fn launch_scene_tracks(
        &mut self,
        scene: usize,
        tracks: &[usize],
    ) -> Option<Vec<(usize, TrackPatternData)>> {
        let scene = self.scenes.get(scene)?;
        let resolved = tracks
            .iter()
            .copied()
            .map(|track| {
                let id = scene.cells.get(track).copied().flatten()?;
                let data = self.track_pools.get(track)?.get(id)?.clone();
                Some((track, id, data))
            })
            .collect::<Option<Vec<_>>>()?;

        for (track, id, _) in &resolved {
            *self.track_overrides.get_mut(*track)? = Some(*id);
        }
        Some(
            resolved
                .into_iter()
                .map(|(track, _, data)| (track, data))
                .collect(),
        )
    }

    pub fn track_pattern_cells(&self, track: usize) -> Vec<TrackPatternCellView> {
        let Some(pool) = self.track_pools.get(track) else {
            return Vec::new();
        };
        let assigned = self
            .scenes
            .get(self.current_scene)
            .and_then(|scene| scene.cells.get(track))
            .copied()
            .flatten();
        let override_id = self.track_overrides.get(track).copied().flatten();
        let active = override_id.or(assigned);
        let overridden = override_id.is_some();
        let mut ids = pool.patterns.keys().copied().collect::<Vec<_>>();
        ids.sort_by_key(|id| id.0);
        ids.into_iter()
            .map(|pattern_id| TrackPatternCellView {
                pattern_id,
                assigned_to_current_scene: Some(pattern_id) == assigned,
                active_effective: Some(pattern_id) == active,
                overridden,
            })
            .collect()
    }

    pub fn set_cell(&mut self, scene: usize, track: usize, id: PatternId) -> bool {
        let Some(pool) = self.track_pools.get(track) else {
            return false;
        };
        if !pool.contains(id) {
            return false;
        }
        let Some(scene) = self.scenes.get_mut(scene) else {
            return false;
        };
        if track >= scene.cells.len() {
            return false;
        }

        scene.cells[track] = Some(id);
        true
    }

    pub fn clear_cell(&mut self, scene: usize, track: usize) -> Option<PatternId> {
        let scene = self.scenes.get_mut(scene)?;
        let cell = scene.cells.get_mut(track)?;
        let cleared = cell.take();
        if let Some(id) = cleared {
            if self.track_overrides.get(track).copied().flatten() == Some(id) {
                self.track_overrides[track] = None;
            }
        }
        cleared
    }

    pub fn fork_track_pattern(&mut self, track: usize) -> Option<PatternId> {
        let source = self.effective_track_pattern(track)?.clone();
        let id = self.track_pools.get_mut(track)?.insert(source);
        *self.track_overrides.get_mut(track)? = Some(id);
        Some(id)
    }

    pub fn clone_track_pattern_into_current_scene(&mut self, track: usize) -> Option<PatternId> {
        let source_id = self
            .track_overrides
            .get(track)
            .copied()
            .flatten()
            .or_else(|| {
                self.scenes
                    .get(self.current_scene)
                    .and_then(|scene| scene.cells.get(track))
                    .copied()
                    .flatten()
            })?;
        self.clone_track_pattern_id_into_current_scene(track, source_id)
    }

    pub fn clone_track_pattern_id_into_current_scene(
        &mut self,
        track: usize,
        source_id: PatternId,
    ) -> Option<PatternId> {
        if track >= self.scenes.get(self.current_scene)?.cells.len() {
            return None;
        }
        let source = self.track_pools.get(track)?.get(source_id)?.clone();
        let id = self.track_pools.get_mut(track)?.insert(source);
        let scene = self.scenes.get_mut(self.current_scene)?;
        scene.cells[track] = Some(id);
        *self.track_overrides.get_mut(track)? = None;
        Some(id)
    }

    pub fn delete_track_pattern(&mut self, track: usize, id: PatternId) -> bool {
        let Some(pool) = self.track_pools.get_mut(track) else {
            return false;
        };
        if pool.remove(id).is_none() {
            return false;
        }
        for scene in &mut self.scenes {
            if scene.cells.get(track).copied().flatten() == Some(id) {
                scene.cells[track] = None;
            }
        }
        if self.track_overrides.get(track).copied().flatten() == Some(id) {
            self.track_overrides[track] = None;
        }
        true
    }

    pub fn new_scene(&mut self) -> usize {
        let source_scene = self.scenes.get(self.current_scene).cloned();
        let mut cells = vec![None; self.track_pools.len()];
        for track in 0..self.track_pools.len() {
            if let Some(source) = self.effective_track_pattern(track).cloned() {
                cells[track] = Some(self.track_pools[track].insert(source));
            }
        }

        let scene_idx = self.scenes.len();
        let (
            bus_patterns,
            mod_connections,
            neural_networks,
            graph_overrides,
            project_process_chain,
        ) = source_scene
            .map(|scene| {
                (
                    scene.bus_patterns,
                    scene.mod_connections,
                    scene.neural_networks,
                    scene.graph_overrides,
                    scene.project_process_chain,
                )
            })
            .unwrap_or_default();
        let next_id = self.next_scene_id;
        self.next_scene_id = self
            .next_scene_id
            .checked_add(1)
            .expect("scene identity space exhausted");
        self.scenes.push(Scene {
            id: SceneId(next_id),
            name: format!("Scene {}", scene_idx + 1),
            cells,
            bus_patterns,
            mod_connections,
            neural_networks,
            graph_overrides,
            project_process_chain,
        });
        self.current_scene = scene_idx;
        self.track_overrides.fill(None);
        scene_idx
    }

    pub fn remove_track(&mut self, track: usize) -> bool {
        if track >= self.track_pools.len() {
            return false;
        }

        self.track_pools.remove(track);
        for scene in &mut self.scenes {
            if track < scene.cells.len() {
                scene.cells.remove(track);
            }
        }
        if track < self.track_overrides.len() {
            self.track_overrides.remove(track);
        }
        true
    }

    pub fn purge_unused_track_patterns(&mut self) -> usize {
        let mut removed = 0;
        for track in 0..self.track_pools.len() {
            let mut referenced = HashSet::new();
            for scene in &self.scenes {
                if let Some(id) = scene.cells.get(track).copied().flatten() {
                    referenced.insert(id);
                }
            }
            if let Some(id) = self.track_overrides.get(track).copied().flatten() {
                referenced.insert(id);
            }

            let before = self.track_pools[track].patterns.len();
            self.track_pools[track]
                .patterns
                .retain(|id, _| referenced.contains(id));
            removed += before - self.track_pools[track].patterns.len();
        }
        removed
    }

    fn edit_other_track_patterns<F>(&mut self, track: usize, mut edit: F) -> bool
    where
        F: FnMut(&mut TrackPatternData),
    {
        let current_effective = self.effective_pattern_id(track);
        let Some(pool) = self.track_pools.get_mut(track) else {
            return false;
        };
        for (id, data) in &mut pool.patterns {
            if Some(*id) != current_effective {
                edit(data);
            }
        }
        true
    }
}

impl PatternSnapshot {
    pub fn refresh_process_binding_param_ids(
        &mut self,
        effect_descriptors: &[Vec<EffectDescriptor>],
        instrument_descriptors: &[EffectDescriptor],
    ) {
        for track in 0..self.process_chains.len() {
            crate::process::refresh_track_process_chain_binding_param_ids(
                &mut self.process_chains[track],
                instrument_descriptors.get(track),
                self.instrument_slots.get(track),
                effect_descriptors
                    .get(track)
                    .map(Vec::as_slice)
                    .unwrap_or(&[]),
                self.effect_slots
                    .get(track)
                    .map(Vec::as_slice)
                    .unwrap_or(&[]),
            );
        }
    }

    /// Delete one track lane from this snapshot and compact higher track indices.
    ///
    /// This is the snapshot-side half of track deletion semantics:
    /// deleting a track removes it from every track-indexed lane immediately,
    /// and every track after it shifts down by one. The live delete path will
    /// pair this with graph teardown, live-state compaction, and UI refresh.
    pub fn remove_track(&mut self, track_idx: usize) {
        if track_idx >= self.track_bits.len() {
            return;
        }

        self.track_bits.remove(track_idx);
        remove_track_lane_if_present(&mut self.neural_reset_bits, track_idx);
        remove_track_lane_if_present(&mut self.step_data, track_idx);
        remove_track_lane_if_present(&mut self.track_params, track_idx);
        remove_track_lane_if_present(&mut self.effect_slots, track_idx);
        remove_track_lane_if_present(&mut self.midi_fx_slots, track_idx);
        remove_track_lane_if_present(&mut self.instrument_slots, track_idx);
        remove_track_lane_if_present(&mut self.instrument_base_note_offsets, track_idx);
        remove_track_lane_if_present(&mut self.track_sound_states, track_idx);
        remove_track_lane_if_present(&mut self.sample_ids, track_idx);
        remove_track_lane_if_present(&mut self.chord_snapshots, track_idx);
        remove_track_lane_if_present(&mut self.timebase_plock_snapshots, track_idx);
        remove_track_lane_if_present(&mut self.swing_plock_snapshots, track_idx);
        remove_track_lane_if_present(&mut self.swing_resolution_plock_snapshots, track_idx);
        remove_track_lane_if_present(&mut self.instrument_types, track_idx);
        remove_track_lane_if_present(&mut self.instrument_run_modes, track_idx);
        remove_track_lane_if_present(&mut self.rack_tracks, track_idx);
        remove_track_lane_if_present(&mut self.process_chains, track_idx);
        remove_track_lane_if_present(&mut self.project_process_lane_overrides, track_idx);
        remove_track_lane_if_present(&mut self.plock_variant_registries, track_idx);
        remove_track_lane_if_present(&mut self.key_lock_variant_registries, track_idx);
        self.mod_connections = self
            .mod_connections
            .iter()
            .filter_map(|connection| {
                remap_mod_connection_after_track_delete(*connection, track_idx)
            })
            .collect();
        remap_neural_network_routes_after_track_delete(&mut self.neural_networks, track_idx);
        remap_graph_overrides_after_track_delete(&mut self.graph_overrides, track_idx);
    }

    pub fn remove_effect_slot(&mut self, track: usize, slot_idx: usize) {
        let Some(slots) = self.effect_slots.get_mut(track) else {
            return;
        };
        if slot_idx >= slots.len() {
            return;
        }

        for idx in slot_idx..slots.len().saturating_sub(1) {
            slots[idx] = slots[idx + 1].clone();
        }
        if let Some(last) = slots.last_mut() {
            last.clear();
        }
    }

    pub fn insert_empty_effect_slot(&mut self, track: usize, slot_idx: usize) {
        let Some(slots) = self.effect_slots.get_mut(track) else {
            return;
        };
        if slot_idx >= slots.len() {
            return;
        }
        for idx in (slot_idx + 1..slots.len()).rev() {
            slots[idx] = slots[idx - 1].clone();
        }
        slots[slot_idx].clear();
    }

    pub fn move_effect_slot_to(&mut self, track: usize, source_slot: usize, target_slot: usize) {
        let Some(slots) = self.effect_slots.get_mut(track) else {
            return;
        };
        if source_slot >= slots.len() || target_slot >= slots.len() || source_slot == target_slot {
            return;
        }
        let entry = slots.remove(source_slot);
        slots.insert(target_slot, entry);
        while slots.len() <= target_slot.max(source_slot) {
            slots.push(EffectSlotSnapshot::new_empty());
        }
    }

    pub fn remove_midi_fx_slot(&mut self, track: usize, slot_idx: usize) {
        let Some(params) = self.track_params.get_mut(track) else {
            return;
        };
        if slot_idx < params.midi_fx_chain.len() {
            params.midi_fx_chain.remove(slot_idx);
        }

        let Some(slots) = self.midi_fx_slots.get_mut(track) else {
            return;
        };
        if slot_idx >= slots.len() {
            return;
        }

        for idx in slot_idx..slots.len().saturating_sub(1) {
            slots[idx] = slots[idx + 1].clone();
        }
        if let Some(last) = slots.last_mut() {
            last.clear();
        }
    }

    pub fn insert_midi_fx_slot(
        &mut self,
        track: usize,
        slot_idx: usize,
        name: String,
        desc: &EffectDescriptor,
    ) {
        let Some(params) = self.track_params.get_mut(track) else {
            return;
        };
        let insert_idx = slot_idx.min(params.midi_fx_chain.len());
        params.midi_fx_chain.insert(insert_idx, name);

        let Some(slots) = self.midi_fx_slots.get_mut(track) else {
            return;
        };
        if insert_idx >= slots.len() {
            return;
        }
        for idx in (insert_idx + 1..slots.len()).rev() {
            slots[idx] = slots[idx - 1].clone();
        }
        slots[insert_idx].sync_to_descriptor(desc, 0);
    }

    pub fn move_midi_fx_slot_to(&mut self, track: usize, source_slot: usize, target_slot: usize) {
        let Some(params) = self.track_params.get_mut(track) else {
            return;
        };
        if source_slot >= params.midi_fx_chain.len() {
            return;
        }
        let target_slot = target_slot.min(params.midi_fx_chain.len().saturating_sub(1));
        if source_slot == target_slot {
            return;
        }
        let name = params.midi_fx_chain.remove(source_slot);
        params.midi_fx_chain.insert(target_slot, name);

        let Some(slots) = self.midi_fx_slots.get_mut(track) else {
            return;
        };
        if source_slot >= slots.len() || target_slot >= slots.len() {
            return;
        }
        let entry = slots.remove(source_slot);
        slots.insert(target_slot, entry);
        while slots.len() <= target_slot.max(source_slot) {
            slots.push(EffectSlotSnapshot::new_empty());
        }
    }

    /// Normalize every track-indexed lane to the exact live track count.
    ///
    /// Project files can be older than the current in-memory snapshot shape, or
    /// can legitimately omit lanes for newly-added track-scoped data. Restoring
    /// a partial snapshot is unsafe because any missing lane would leave the
    /// previous live project's lane intact. This method makes the snapshot a
    /// complete replacement before it is committed to the pattern bank.
    pub fn normalize_track_count(
        &mut self,
        track_count: usize,
        slot_descriptors: &[Vec<EffectDescriptor>],
    ) {
        self.truncate_tracks(track_count);
        while self.track_bits.len() < track_count {
            let track = self.track_bits.len();
            self.track_bits.push([0u64; TRACK_PATTERN_WORDS]);
            self.neural_reset_bits.push([0u64; TRACK_PATTERN_WORDS]);
            self.step_data.push(Self::default_step_data());
            self.track_params.push(TrackParamsSnapshot::default());
            self.effect_slots
                .push(Self::default_effect_slots(track, slot_descriptors));
            self.midi_fx_slots.push(Self::default_midi_fx_slots());
            self.instrument_slots.push(Self::default_instrument_slot());
            self.instrument_base_note_offsets.push(0.0);
            self.track_sound_states.push(TrackSoundState::default());
            self.sample_ids.push((-1, String::new(), 44_100));
            self.chord_snapshots.push(ChordSnapshot::new_default());
            self.timebase_plock_snapshots.push([None; MAX_STEPS]);
            self.swing_plock_snapshots.push([None; MAX_STEPS]);
            self.swing_resolution_plock_snapshots
                .push([None; MAX_STEPS]);
            self.instrument_types.push(InstrumentType::Sampler);
            self.instrument_run_modes
                .push(CustomInstrumentRunMode::Instrument);
            self.rack_tracks.push(None);
            self.process_chains
                .push(crate::process::TrackProcessChain::default());
            self.plock_variant_registries
                .push(PlockVariantRegistry::default());
            self.key_lock_variant_registries
                .push(PlockVariantRegistry::default());
        }
        while self.neural_reset_bits.len() < track_count {
            self.neural_reset_bits.push([0u64; TRACK_PATTERN_WORDS]);
        }
        while self.rack_tracks.len() < track_count {
            self.rack_tracks.push(None);
        }
        while self.process_chains.len() < track_count {
            self.process_chains
                .push(crate::process::TrackProcessChain::default());
        }
        while self.project_process_lane_overrides.len() < track_count {
            self.project_process_lane_overrides.push(Default::default());
        }
        while self.plock_variant_registries.len() < track_count {
            self.plock_variant_registries
                .push(PlockVariantRegistry::default());
        }
        while self.key_lock_variant_registries.len() < track_count {
            self.key_lock_variant_registries
                .push(PlockVariantRegistry::default());
        }
        self.mod_connections.retain(|connection| {
            connection.source_track < track_count
                && mod_destination_valid_for_track_count(connection.destination, track_count)
                && connection.dest_input < EXT_MOD_INPUT_COUNT
        });

        for steps in &mut self.step_data {
            steps.truncate(MAX_STEPS);
            while steps.len() < MAX_STEPS {
                let mut params = [0.0f32; NUM_PARAMS];
                for param in StepParam::ALL {
                    params[param.index()] = param.default_value();
                }
                steps.push(params);
            }
        }
    }

    fn truncate_tracks(&mut self, track_count: usize) {
        self.track_bits.truncate(track_count);
        self.neural_reset_bits.truncate(track_count);
        self.step_data.truncate(track_count);
        self.track_params.truncate(track_count);
        self.effect_slots.truncate(track_count);
        self.midi_fx_slots.truncate(track_count);
        self.instrument_slots.truncate(track_count);
        self.instrument_base_note_offsets.truncate(track_count);
        self.track_sound_states.truncate(track_count);
        self.sample_ids.truncate(track_count);
        self.chord_snapshots.truncate(track_count);
        self.timebase_plock_snapshots.truncate(track_count);
        self.swing_plock_snapshots.truncate(track_count);
        self.swing_resolution_plock_snapshots.truncate(track_count);
        self.instrument_types.truncate(track_count);
        self.instrument_run_modes.truncate(track_count);
        self.rack_tracks.truncate(track_count);
        self.process_chains.truncate(track_count);
        self.project_process_lane_overrides.truncate(track_count);
        self.plock_variant_registries.truncate(track_count);
        self.key_lock_variant_registries.truncate(track_count);
        self.mod_connections.retain(|connection| {
            connection.source_track < track_count
                && mod_destination_valid_for_track_count(connection.destination, track_count)
                && connection.dest_input < EXT_MOD_INPUT_COUNT
        });
    }

    pub fn capture(
        state: &SequencerState,
        num_tracks: usize,
        track_buffer_ids: &[i32],
        track_sample_rates: &[u32],
        track_names: &[String],
        instrument_types: &[InstrumentType],
    ) -> Self {
        let mut track_bits = Vec::with_capacity(num_tracks);
        let mut neural_reset_bits = Vec::with_capacity(num_tracks);
        let mut step_data = Vec::with_capacity(num_tracks);
        let mut track_params = Vec::with_capacity(num_tracks);
        let mut effect_slots = Vec::with_capacity(num_tracks);
        let mut midi_fx_slots = Vec::with_capacity(num_tracks);
        let mut instrument_slots = Vec::with_capacity(num_tracks);
        let mut instrument_base_note_offsets = Vec::with_capacity(num_tracks);
        let track_sound_state = state.pattern.track_sound_state.lock().unwrap();
        let mut sound_states = Vec::with_capacity(num_tracks);
        let mut sample_ids = Vec::with_capacity(num_tracks);
        let mut chord_snapshots = Vec::with_capacity(num_tracks);
        let mut timebase_plock_snapshots = Vec::with_capacity(num_tracks);
        let mut swing_plock_snapshots = Vec::with_capacity(num_tracks);
        let mut swing_resolution_plock_snapshots = Vec::with_capacity(num_tracks);
        let mut inst_types = Vec::with_capacity(num_tracks);
        let mut instrument_run_modes = Vec::with_capacity(num_tracks);
        let mut rack_tracks = Vec::with_capacity(num_tracks);
        let mut process_chains = Vec::with_capacity(num_tracks);
        let mut plock_variant_registries = Vec::with_capacity(num_tracks);
        let mut key_lock_variant_registries = Vec::with_capacity(num_tracks);

        let scene_trace = {
            static SCENE_TRACE: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
            *SCENE_TRACE.get_or_init(|| std::env::var("ESEQ_SCENE_TRACE").is_ok_and(|v| v == "1"))
        };
        let mut steps_elapsed = std::time::Duration::ZERO;
        let mut effects_elapsed = std::time::Duration::ZERO;
        let mut midi_elapsed = std::time::Duration::ZERO;
        let mut instrument_elapsed = std::time::Duration::ZERO;
        let mut rest_elapsed = std::time::Duration::ZERO;
        for t in 0..num_tracks {
            track_bits.push(state.pattern.patterns[t].load_bits());
            neural_reset_bits.push(state.pattern.neural_reset_patterns[t].load_bits());

            let started = Instant::now();
            step_data.push(state.pattern.step_data[t].load_rows());
            steps_elapsed += started.elapsed();

            let tp = &state.pattern.track_params[t];
            track_params.push(capture_track_params_snapshot(tp));

            let started = Instant::now();
            let chain: Vec<EffectSlotSnapshot> = state.pattern.effect_chains[t]
                .iter()
                .map(EffectSlotSnapshot::capture)
                .collect();
            effect_slots.push(chain);
            effects_elapsed += started.elapsed();
            let started = Instant::now();
            midi_fx_slots.push(
                state.pattern.midi_fx_slots[t]
                    .iter()
                    .map(EffectSlotSnapshot::capture)
                    .collect(),
            );
            midi_elapsed += started.elapsed();
            let started = Instant::now();
            instrument_slots.push(EffectSlotSnapshot::capture(
                &state.pattern.instrument_slots[t],
            ));
            instrument_elapsed += started.elapsed();
            let rest_started = Instant::now();
            instrument_base_note_offsets.push(f32::from_bits(
                state.pattern.instrument_base_note_offsets[t].load(Ordering::Relaxed),
            ));
            let mut sound = track_sound_state.get(t).cloned().unwrap_or_default();
            let engine_id = state.runtime.track_engine_ids[t].load(Ordering::Relaxed);
            sound.engine_id = if engine_id == u32::MAX {
                None
            } else {
                Some(engine_id as usize)
            };
            sound_states.push(sound);

            let buf_id = if t < track_buffer_ids.len() {
                track_buffer_ids[t]
            } else {
                -1
            };
            let name = if t < track_names.len() {
                track_names[t].clone()
            } else {
                String::new()
            };
            let sample_rate = if t < track_sample_rates.len() {
                track_sample_rates[t]
            } else {
                44_100
            };
            sample_ids.push((buf_id, name, sample_rate));
            chord_snapshots.push(ChordSnapshot::capture(&state.pattern.chord_data[t]));
            timebase_plock_snapshots.push(state.pattern.timebase_plocks[t].snapshot());
            swing_plock_snapshots.push(state.pattern.swing_plocks[t].snapshot());
            swing_resolution_plock_snapshots
                .push(state.pattern.swing_resolution_plocks[t].snapshot());
            inst_types.push(if t < instrument_types.len() {
                instrument_types[t]
            } else {
                InstrumentType::Sampler
            });
            instrument_run_modes.push(CustomInstrumentRunMode::from_runtime_flag(
                state.pattern.instrument_run_modes[t].load(Ordering::Relaxed),
            ));
            rack_tracks.push(
                state
                    .pattern
                    .rack_tracks
                    .lock()
                    .unwrap()
                    .get(t)
                    .cloned()
                    .unwrap_or(None),
            );
            process_chains.push(
                state
                    .pattern
                    .process_chains
                    .lock()
                    .unwrap()
                    .get(t)
                    .cloned()
                    .unwrap_or_default(),
            );
            let mut registry = state
                .pattern
                .plock_variant_registries
                .lock()
                .unwrap()
                .get(t)
                .cloned()
                .unwrap_or_default();
            registry.prune_to_keys(&live_track_variant_keys(state, t));
            plock_variant_registries.push(registry);
            let mut key_registry = state
                .pattern
                .key_lock_variant_registries
                .lock()
                .unwrap()
                .get(t)
                .cloned()
                .unwrap_or_default();
            key_registry.prune_to_keys(&live_track_key_lock_variant_keys(state, t));
            key_lock_variant_registries.push(key_registry);
            rest_elapsed += rest_started.elapsed();
        }
        if scene_trace {
            eprintln!(
                "[capture-trace] steps={:.3}ms effects={:.3}ms midi={:.3}ms instrument={:.3}ms rest={:.3}ms",
                steps_elapsed.as_secs_f64() * 1000.0,
                effects_elapsed.as_secs_f64() * 1000.0,
                midi_elapsed.as_secs_f64() * 1000.0,
                instrument_elapsed.as_secs_f64() * 1000.0,
                rest_elapsed.as_secs_f64() * 1000.0,
            );
        }

        Self {
            track_bits,
            neural_reset_bits,
            step_data,
            track_params,
            effect_slots,
            midi_fx_slots,
            instrument_slots,
            instrument_base_note_offsets,
            track_sound_states: sound_states,
            sample_ids,
            chord_snapshots,
            timebase_plock_snapshots,
            swing_plock_snapshots,
            swing_resolution_plock_snapshots,
            instrument_types: inst_types,
            instrument_run_modes,
            mod_connections: Vec::new(),
            neural_networks: Vec::new(),
            graph_overrides: Vec::new(),
            rack_tracks,
            process_chains,
            project_process_lane_overrides: state
                .pattern
                .project_process_lane_overrides
                .lock()
                .unwrap()
                .iter()
                .take(num_tracks)
                .cloned()
                .collect(),
            project_process_chain: crate::process::TrackProcessChain::default(),
            plock_variant_registries,
            key_lock_variant_registries,
        }
    }

    pub fn capture_with_mod_connections(
        state: &SequencerState,
        num_tracks: usize,
        track_buffer_ids: &[i32],
        track_sample_rates: &[u32],
        track_names: &[String],
        instrument_types: &[InstrumentType],
        mod_connections: Vec<ModConnection>,
        neural_networks: Vec<ProjectNeuralNetwork>,
        graph_overrides: Vec<ProjectGraphOverrides>,
    ) -> Self {
        let mut snapshot = Self::capture(
            state,
            num_tracks,
            track_buffer_ids,
            track_sample_rates,
            track_names,
            instrument_types,
        );
        snapshot.mod_connections = mod_connections;
        snapshot.neural_networks = neural_networks;
        snapshot.graph_overrides = graph_overrides;
        snapshot
    }

    pub fn restore(&self, state: &SequencerState) {
        for track in 0..self.track_bits.len() {
            self.restore_track(state, track);
        }
    }

    pub fn restore_track(&self, state: &SequencerState, track: usize) -> bool {
        let Some(data) = self.track_pattern_data(track) else {
            return false;
        };
        data.restore_to(state, track)
    }

    pub fn track_pattern_data(&self, track: usize) -> Option<TrackPatternData> {
        Some(TrackPatternData {
            track_bits: *self.track_bits.get(track)?,
            neural_reset_bits: self
                .neural_reset_bits
                .get(track)
                .copied()
                .unwrap_or([0u64; TRACK_PATTERN_WORDS]),
            step_data: self.step_data.get(track)?.clone(),
            track_params: self.track_params.get(track)?.clone(),
            effect_slots: self.effect_slots.get(track)?.clone(),
            midi_fx_slots: self.midi_fx_slots.get(track).cloned().unwrap_or_else(|| {
                vec![EffectSlotSnapshot::new_empty(); crate::lisp_host::MAX_MIDI_FX_SLOTS]
            }),
            instrument_slot: self
                .instrument_slots
                .get(track)
                .cloned()
                .unwrap_or_else(EffectSlotSnapshot::new_empty),
            instrument_base_note_offset: self
                .instrument_base_note_offsets
                .get(track)
                .copied()
                .unwrap_or(0.0),
            track_sound_state: self
                .track_sound_states
                .get(track)
                .cloned()
                .unwrap_or_default(),
            sample_id: self
                .sample_ids
                .get(track)
                .cloned()
                .unwrap_or((-1, String::new(), 44_100)),
            chord_snapshot: self
                .chord_snapshots
                .get(track)
                .cloned()
                .unwrap_or_else(ChordSnapshot::new_default),
            timebase_plock_snapshot: self
                .timebase_plock_snapshots
                .get(track)
                .copied()
                .unwrap_or([None; MAX_STEPS]),
            swing_plock_snapshot: self
                .swing_plock_snapshots
                .get(track)
                .copied()
                .unwrap_or([None; MAX_STEPS]),
            swing_resolution_plock_snapshot: self
                .swing_resolution_plock_snapshots
                .get(track)
                .copied()
                .unwrap_or([None; MAX_STEPS]),
            instrument_type: self
                .instrument_types
                .get(track)
                .copied()
                .unwrap_or(InstrumentType::Sampler),
            instrument_run_mode: self
                .instrument_run_modes
                .get(track)
                .copied()
                .unwrap_or(CustomInstrumentRunMode::Instrument),
            rack_track: self.rack_tracks.get(track).cloned().unwrap_or(None),
            process_chain: self.process_chains.get(track).cloned().unwrap_or_default(),
            project_process_lane_overrides: self
                .project_process_lane_overrides
                .get(track)
                .cloned()
                .unwrap_or_default(),
            plock_variant_registry: self
                .plock_variant_registries
                .get(track)
                .cloned()
                .unwrap_or_default(),
            key_lock_variant_registry: self
                .key_lock_variant_registries
                .get(track)
                .cloned()
                .unwrap_or_default(),
        })
    }

    pub fn set_track_pattern_data(&mut self, track: usize, data: TrackPatternData) {
        while self.track_bits.len() <= track {
            let next_track = self.track_bits.len();
            self.push_default_track(next_track, &[]);
        }

        self.track_bits[track] = data.track_bits;
        self.neural_reset_bits[track] = data.neural_reset_bits;
        self.step_data[track] = data.step_data;
        self.track_params[track] = data.track_params;
        self.effect_slots[track] = data.effect_slots;
        self.midi_fx_slots[track] = data.midi_fx_slots;
        self.instrument_slots[track] = data.instrument_slot;
        self.instrument_base_note_offsets[track] = data.instrument_base_note_offset;
        self.track_sound_states[track] = data.track_sound_state;
        self.sample_ids[track] = data.sample_id;
        self.chord_snapshots[track] = data.chord_snapshot;
        self.timebase_plock_snapshots[track] = data.timebase_plock_snapshot;
        self.swing_plock_snapshots[track] = data.swing_plock_snapshot;
        self.swing_resolution_plock_snapshots[track] = data.swing_resolution_plock_snapshot;
        self.instrument_types[track] = data.instrument_type;
        self.instrument_run_modes[track] = data.instrument_run_mode;
        self.rack_tracks[track] = data.rack_track;
        self.process_chains[track] = data.process_chain;
        self.project_process_lane_overrides[track] = data.project_process_lane_overrides;
        self.plock_variant_registries[track] = data.plock_variant_registry;
        self.key_lock_variant_registries[track] = data.key_lock_variant_registry;
    }

    pub fn clone_track_lane_from(&mut self, source: &PatternSnapshot, track: usize) {
        if let Some(data) = source.track_pattern_data(track) {
            self.set_track_pattern_data(track, data);
        }
    }

    pub fn clear_track(
        &mut self,
        track: usize,
        slot_descriptors: &[Vec<EffectDescriptor>],
        instrument_type: InstrumentType,
    ) {
        if track >= self.track_bits.len() {
            return;
        }
        self.track_bits[track] = [0u64; TRACK_PATTERN_WORDS];
        self.neural_reset_bits[track] = [0u64; TRACK_PATTERN_WORDS];
        self.step_data[track] = Self::default_step_data();
        self.track_params[track] = TrackParamsSnapshot::default();
        self.effect_slots[track] = Self::default_effect_slots(track, slot_descriptors);
        self.midi_fx_slots[track] = Self::default_midi_fx_slots();
        self.instrument_slots[track] = Self::default_instrument_slot();
        self.instrument_base_note_offsets[track] = 0.0;
        self.track_sound_states[track] = TrackSoundState::default();
        self.sample_ids[track] = (-1, String::new(), 44_100);
        self.chord_snapshots[track] = ChordSnapshot::new_default();
        self.timebase_plock_snapshots[track] = [None; MAX_STEPS];
        self.swing_plock_snapshots[track] = [None; MAX_STEPS];
        self.swing_resolution_plock_snapshots[track] = [None; MAX_STEPS];
        self.instrument_types[track] = instrument_type;
        self.instrument_run_modes[track] = CustomInstrumentRunMode::Instrument;
        self.rack_tracks[track] = None;
        self.process_chains[track] = crate::process::TrackProcessChain::default();
        self.project_process_lane_overrides[track] = Default::default();
        self.plock_variant_registries[track] = PlockVariantRegistry::default();
        self.key_lock_variant_registries[track] = PlockVariantRegistry::default();
    }

    fn default_step_data() -> Vec<[f32; NUM_PARAMS]> {
        (0..MAX_STEPS)
            .map(|_| {
                let mut params = [0.0f32; NUM_PARAMS];
                for p in StepParam::ALL {
                    params[p.index()] = p.default_value();
                }
                params
            })
            .collect()
    }

    fn default_effect_slots(
        t: usize,
        slot_descriptors: &[Vec<EffectDescriptor>],
    ) -> Vec<EffectSlotSnapshot> {
        if t < slot_descriptors.len() {
            slot_descriptors[t]
                .iter()
                .map(|desc| EffectSlotSnapshot::new_default(desc, 0))
                .collect()
        } else {
            Vec::new()
        }
    }

    fn default_instrument_slot() -> EffectSlotSnapshot {
        EffectSlotSnapshot::new_empty()
    }

    fn default_midi_fx_slots() -> Vec<EffectSlotSnapshot> {
        (0..crate::lisp_host::MAX_MIDI_FX_SLOTS)
            .map(|_| EffectSlotSnapshot::new_empty())
            .collect()
    }

    fn push_default_track(&mut self, t: usize, slot_descriptors: &[Vec<EffectDescriptor>]) {
        self.track_bits.push([0u64; TRACK_PATTERN_WORDS]);
        self.neural_reset_bits.push([0u64; TRACK_PATTERN_WORDS]);
        self.step_data.push(Self::default_step_data());
        self.track_params.push(TrackParamsSnapshot::default());
        self.effect_slots
            .push(Self::default_effect_slots(t, slot_descriptors));
        self.midi_fx_slots.push(Self::default_midi_fx_slots());
        self.instrument_slots.push(Self::default_instrument_slot());
        self.instrument_base_note_offsets.push(0.0);
        self.track_sound_states.push(TrackSoundState::default());
        self.sample_ids.push((-1, String::new(), 44_100));
        self.chord_snapshots.push(ChordSnapshot::new_default());
        self.timebase_plock_snapshots.push([None; MAX_STEPS]);
        self.swing_plock_snapshots.push([None; MAX_STEPS]);
        self.swing_resolution_plock_snapshots
            .push([None; MAX_STEPS]);
        self.instrument_types.push(InstrumentType::Sampler);
        self.instrument_run_modes
            .push(CustomInstrumentRunMode::Instrument);
        self.rack_tracks.push(None);
        self.process_chains
            .push(crate::process::TrackProcessChain::default());
        self.project_process_lane_overrides.push(Default::default());
        self.plock_variant_registries
            .push(PlockVariantRegistry::default());
        self.key_lock_variant_registries
            .push(PlockVariantRegistry::default());
    }

    pub fn new_default(num_tracks: usize, slot_descriptors: &[Vec<EffectDescriptor>]) -> Self {
        let mut snap = Self {
            track_bits: Vec::with_capacity(num_tracks),
            neural_reset_bits: Vec::with_capacity(num_tracks),
            step_data: Vec::with_capacity(num_tracks),
            track_params: Vec::with_capacity(num_tracks),
            effect_slots: Vec::with_capacity(num_tracks),
            midi_fx_slots: Vec::with_capacity(num_tracks),
            instrument_slots: Vec::with_capacity(num_tracks),
            instrument_base_note_offsets: Vec::with_capacity(num_tracks),
            track_sound_states: Vec::with_capacity(num_tracks),
            sample_ids: Vec::with_capacity(num_tracks),
            chord_snapshots: Vec::with_capacity(num_tracks),
            timebase_plock_snapshots: Vec::with_capacity(num_tracks),
            swing_plock_snapshots: Vec::with_capacity(num_tracks),
            swing_resolution_plock_snapshots: Vec::with_capacity(num_tracks),
            instrument_types: Vec::with_capacity(num_tracks),
            instrument_run_modes: Vec::with_capacity(num_tracks),
            mod_connections: Vec::new(),
            neural_networks: Vec::new(),
            graph_overrides: Vec::new(),
            rack_tracks: Vec::with_capacity(num_tracks),
            process_chains: Vec::with_capacity(num_tracks),
            project_process_lane_overrides: Vec::with_capacity(num_tracks),
            project_process_chain: crate::process::TrackProcessChain::default(),
            plock_variant_registries: Vec::with_capacity(num_tracks),
            key_lock_variant_registries: Vec::with_capacity(num_tracks),
        };
        for t in 0..num_tracks {
            snap.push_default_track(t, slot_descriptors);
        }
        snap
    }

    pub fn extend_to_tracks(
        &mut self,
        new_count: usize,
        slot_descriptors: &[Vec<EffectDescriptor>],
    ) {
        if new_count <= self.track_bits.len() {
            return;
        }
        let old_count = self.track_bits.len();
        self.normalize_track_count(new_count, slot_descriptors);
        debug_assert_eq!(self.track_bits.len(), new_count);
        debug_assert!(old_count <= new_count);
    }

    #[cfg(test)]
    fn track_lane_count_is_consistent(&self) -> bool {
        let n = self.track_bits.len();
        self.step_data.len() == n
            && self.neural_reset_bits.len() == n
            && self.track_params.len() == n
            && self.effect_slots.len() == n
            && self.midi_fx_slots.len() == n
            && self.instrument_slots.len() == n
            && self.instrument_base_note_offsets.len() == n
            && self.track_sound_states.len() == n
            && self.sample_ids.len() == n
            && self.chord_snapshots.len() == n
            && self.timebase_plock_snapshots.len() == n
            && self.swing_plock_snapshots.len() == n
            && self.instrument_types.len() == n
            && self.swing_resolution_plock_snapshots.len() == n
            && self.instrument_run_modes.len() == n
            && self.rack_tracks.len() == n
            && self.process_chains.len() == n
            && self.project_process_lane_overrides.len() == n
            && self.plock_variant_registries.len() == n
            && self.key_lock_variant_registries.len() == n
            && self.step_data.iter().all(|steps| steps.len() == MAX_STEPS)
    }

    pub fn sync_effect_slot(
        &mut self,
        track: usize,
        slot_idx: usize,
        desc: &EffectDescriptor,
        node_id: u32,
    ) {
        self.sync_effect_slot_with_modulator(track, slot_idx, desc, node_id, 0);
    }

    pub fn sync_effect_slot_with_modulator(
        &mut self,
        track: usize,
        slot_idx: usize,
        desc: &EffectDescriptor,
        node_id: u32,
        modulator_node_id: u32,
    ) {
        while self.effect_slots.len() <= track {
            self.push_default_track(track, &[]);
        }
        while self.effect_slots[track].len() <= slot_idx {
            self.effect_slots[track].push(EffectSlotSnapshot::new_empty());
        }
        self.effect_slots[track][slot_idx].sync_to_descriptor_with_modulator(
            desc,
            node_id,
            modulator_node_id,
        );
    }

    pub fn sync_midi_fx_slot(&mut self, track: usize, slot_idx: usize, desc: &EffectDescriptor) {
        while self.midi_fx_slots.len() <= track {
            self.push_default_track(track, &[]);
        }
        while self.midi_fx_slots[track].len() <= slot_idx {
            self.midi_fx_slots[track].push(EffectSlotSnapshot::new_empty());
        }
        self.midi_fx_slots[track][slot_idx].sync_to_descriptor(desc, 0);
    }

    pub fn sync_instrument_slot(
        &mut self,
        track: usize,
        desc: &EffectDescriptor,
        node_id: u32,
        modulator_node_id: u32,
        instrument_type: InstrumentType,
    ) {
        while self.instrument_slots.len() <= track {
            self.push_default_track(track, &[]);
        }
        self.instrument_slots[track].sync_to_descriptor_with_modulator(
            desc,
            node_id,
            modulator_node_id,
        );
        if track < self.instrument_types.len() {
            self.instrument_types[track] = instrument_type;
        }
    }
}

fn remove_track_lane_if_present<T>(lanes: &mut Vec<T>, track_idx: usize) {
    if track_idx < lanes.len() {
        lanes.remove(track_idx);
    }
}

fn remap_optional_track_after_delete(track: usize, deleted_track: usize) -> Option<usize> {
    if track == deleted_track {
        None
    } else if track > deleted_track {
        Some(track - 1)
    } else {
        Some(track)
    }
}

fn remap_graph_overrides_after_track_delete(
    overrides: &mut [ProjectGraphOverrides],
    deleted_track: usize,
) {
    for graph in overrides {
        for intrinsic in &mut graph.node_intrinsics {
            if let Some(route) = intrinsic.route.take() {
                intrinsic.route = match route {
                    crate::graph::ProjectGraphRouteOverride::None => {
                        Some(crate::graph::ProjectGraphRouteOverride::None)
                    }
                    crate::graph::ProjectGraphRouteOverride::Track(track) => {
                        remap_optional_track_after_delete(track, deleted_track)
                            .map(crate::graph::ProjectGraphRouteOverride::Track)
                    }
                };
            }
            if let Some(seed_from) = intrinsic.seed_from.take() {
                intrinsic.seed_from = Some(match seed_from {
                    crate::graph::ProjectGraphSeedFrom::Route => {
                        crate::graph::ProjectGraphSeedFrom::Route
                    }
                    crate::graph::ProjectGraphSeedFrom::Tracks(tracks) => {
                        crate::graph::ProjectGraphSeedFrom::Tracks(
                            tracks
                                .into_iter()
                                .filter_map(|track| {
                                    remap_optional_track_after_delete(track, deleted_track)
                                })
                                .collect(),
                        )
                    }
                });
            }
        }
    }
}

fn remap_mod_connection_after_track_delete(
    connection: ModConnection,
    deleted_track: usize,
) -> Option<ModConnection> {
    if connection.source_track == deleted_track {
        return None;
    }
    let destination = match connection.destination {
        crate::sequencer::ModDestination::Track(track) if track == deleted_track => return None,
        crate::sequencer::ModDestination::Track(track) => {
            crate::sequencer::ModDestination::Track(if track > deleted_track {
                track - 1
            } else {
                track
            })
        }
        crate::sequencer::ModDestination::Bus(bus) => crate::sequencer::ModDestination::Bus(bus),
    };
    Some(ModConnection {
        source_track: if connection.source_track > deleted_track {
            connection.source_track - 1
        } else {
            connection.source_track
        },
        destination,
        dest_input: connection.dest_input,
    })
}

fn mod_destination_valid_for_track_count(
    destination: crate::sequencer::ModDestination,
    track_count: usize,
) -> bool {
    match destination {
        crate::sequencer::ModDestination::Track(track) => track < track_count,
        crate::sequencer::ModDestination::Bus(_) => true,
    }
}

fn sidechain_source_track(
    owner_track: usize,
    selection_idx: usize,
    total_tracks: usize,
) -> Option<usize> {
    if selection_idx == 0 {
        return None;
    }
    let mut current_idx = 0usize;
    for source_track in 0..total_tracks {
        if source_track == owner_track {
            continue;
        }
        current_idx += 1;
        if current_idx == selection_idx {
            return Some(source_track);
        }
    }
    None
}

fn sidechain_selection_index(
    owner_track: usize,
    source_track: usize,
    total_tracks: usize,
) -> usize {
    if source_track >= total_tracks || source_track == owner_track {
        return 0;
    }
    let mut selection_idx = 0usize;
    for candidate in 0..total_tracks {
        if candidate == owner_track {
            continue;
        }
        selection_idx += 1;
        if candidate == source_track {
            return selection_idx;
        }
    }
    0
}

fn remap_sidechain_selection_after_track_delete(
    owner_track_old: usize,
    selection_idx: usize,
    deleted_track: usize,
    old_track_count: usize,
) -> usize {
    let Some(source_old) = sidechain_source_track(owner_track_old, selection_idx, old_track_count)
    else {
        return 0;
    };
    if source_old == deleted_track {
        return 0;
    }
    let owner_new = if owner_track_old > deleted_track {
        owner_track_old - 1
    } else {
        owner_track_old
    };
    let source_new = if source_old > deleted_track {
        source_old - 1
    } else {
        source_old
    };
    sidechain_selection_index(owner_new, source_new, old_track_count - 1)
}

fn remap_snapshot_sidechain_references_after_track_delete(
    snapshot: &mut PatternSnapshot,
    effect_descriptors: &[Vec<EffectDescriptor>],
    deleted_track: usize,
    old_track_count: usize,
) {
    for owner_track in 0..old_track_count {
        if owner_track == deleted_track || owner_track >= snapshot.effect_slots.len() {
            continue;
        }
        let Some(track_descs) = effect_descriptors.get(owner_track) else {
            continue;
        };
        for (slot_idx, slot) in snapshot.effect_slots[owner_track].iter_mut().enumerate() {
            let Some(desc) = track_descs.get(slot_idx) else {
                continue;
            };
            let num_params = slot.num_params as usize;
            for param_idx in 0..num_params.min(desc.params.len()) {
                if !matches!(
                    desc.params[param_idx].host_control,
                    Some(HostControl::FxSidechain { .. })
                ) {
                    continue;
                }
                let remapped = remap_sidechain_selection_after_track_delete(
                    owner_track,
                    slot.defaults
                        .get(param_idx)
                        .copied()
                        .unwrap_or(0.0)
                        .round()
                        .max(0.0) as usize,
                    deleted_track,
                    old_track_count,
                ) as f32;
                if param_idx < slot.defaults.len() {
                    slot.defaults[param_idx] = remapped;
                }
                for step in 0..MAX_STEPS {
                    let selection = slot.plocks.get(step)
                        .and_then(|params| params.get(param_idx))
                        .and_then(|value| *value);
                    if let (Some(selection), Some(value)) = (
                        selection,
                        slot.plocks.get_mut(step).and_then(|params| params.get_mut(param_idx)),
                    ) {
                        *value = Some(remap_sidechain_selection_after_track_delete(
                            owner_track,
                            selection.round().max(0.0) as usize,
                            deleted_track,
                            old_track_count,
                        ) as f32);
                    }
                }
            }
        }
    }
}

pub fn default_empty_effect_chain() -> Vec<EffectSlotState> {
    use crate::lisp_host::MAX_CUSTOM_FX;
    (0..MAX_CUSTOM_FX)
        .map(|_| EffectSlotState::empty())
        .collect()
}

pub struct PatternState {
    pub patterns: Vec<TrackPattern>,
    pub neural_reset_patterns: Vec<TrackPattern>,
    scene_silenced: Vec<AtomicBool>,
    pub step_data: Vec<StepData>,
    pub chord_data: Vec<ChordData>,
    pub track_params: Vec<TrackParams>,
    pub effect_chains: Vec<Vec<EffectSlotState>>,
    pub midi_fx_slots: Vec<Vec<EffectSlotState>>,
    scenes: Mutex<ProjectScenes>,
    current_pattern: AtomicU32,
    num_patterns: AtomicU32,
    pub timebase_plocks: Vec<TimebasePLockData>,
    pub swing_plocks: Vec<SwingPLockData>,
    pub swing_resolution_plocks: Vec<SwingResolutionPLockData>,
    pub instrument_slots: Vec<EffectSlotState>,
    pub instrument_base_note_offsets: Vec<AtomicU32>,
    pub instrument_run_modes: Vec<AtomicU32>,
    pub track_sound_state: Mutex<Vec<TrackSoundState>>,
    pub rack_tracks: Mutex<Vec<Option<RackTrackSnapshot>>>,
    pub process_chains: Mutex<Vec<crate::process::TrackProcessChain>>,
    pub project_process_lane_overrides: Mutex<Vec<crate::process::ProjectLaneOverrides>>,
    pub plock_variant_registries: Mutex<Vec<PlockVariantRegistry>>,
    pub key_lock_variant_registries: Mutex<Vec<PlockVariantRegistry>>,
}

pub struct TransportState {
    pub playhead: AtomicU32,
    pub playing: AtomicBool,
    pub bpm: AtomicU32,
    pub master_volume: AtomicU32,
    pub pattern_epoch: AtomicU64,
    pub topology_epoch: AtomicU64,
    pub topology_edit_kind: AtomicU32,
    pub topology_edit_track: AtomicU32,
    pub topology_edit_request_id: AtomicU64,
    pub topology_edit_ready_id: AtomicU64,
    pub topology_edit_applied_id: AtomicU64,
    pub mod_reset_counter: AtomicU32,
    pub pending_mod_resync: AtomicBool,
    pub peak_l: AtomicU32,
    pub peak_r: AtomicU32,
    pub cpu_load_pct: AtomicU32,
    pub trigger_flash: Vec<AtomicU32>,
    pub num_tracks: AtomicU32,
    pub track_playheads: Vec<AtomicU32>,
    /// Per-track phase within the active step, normalized to 0.0..=1.0.
    pub track_playhead_phases: Vec<AtomicU32>,
    /// Per-track sampler playhead as normalized 0.0–1.0 (f32 bits).
    pub sampler_playheads: Vec<AtomicU32>,
    pub active_voice_counts: Vec<AtomicU32>,
    pub playhead_phase: AtomicU32,
    /// The live-keyboard record quantization mode (`RecordQuantize as u8`).
    pub record_quantize: AtomicU32,
    /// Audio output latency compensation used when timestamping keyboard note-ons.
    pub record_latency_seconds: AtomicU32,
    /// Monotonic audio-clock anchor published by the audio callback.
    pub record_clock: RecordClockAnchor,
    pub metronome_enabled: AtomicBool,
    pub record_quantize_thresh: AtomicU32,
}

/// Lock-free snapshot of the render clock for wall-clock interpolation on the
/// UI thread. The sequence counter makes the two payload values atomic as a
/// pair without placing a mutex on the realtime callback.
pub struct RecordClockAnchor {
    sequence: AtomicU64,
    beats_bits: AtomicU64,
    timestamp_nanos: AtomicU64,
}

impl RecordClockAnchor {
    pub fn new() -> Self {
        Self {
            sequence: AtomicU64::new(0),
            beats_bits: AtomicU64::new(0.0_f64.to_bits()),
            timestamp_nanos: AtomicU64::new(0),
        }
    }

    /// Publish an anchor from the audio callback. The odd/even sequence is a
    /// standard seqlock protocol; readers retry rather than observing a mixed
    /// beat/timestamp pair.
    pub fn publish(&self, beats: f64, timestamp: Instant) {
        self.sequence.fetch_add(1, Ordering::Release);
        self.beats_bits
            .store(beats.max(0.0).to_bits(), Ordering::Relaxed);
        self.timestamp_nanos
            .store(record_clock_nanos(timestamp), Ordering::Relaxed);
        self.sequence.fetch_add(1, Ordering::Release);
    }

    pub fn sample(&self, timestamp: Instant) -> Option<(f64, Duration)> {
        for _ in 0..8 {
            let before = self.sequence.load(Ordering::Acquire);
            if before & 1 != 0 {
                std::hint::spin_loop();
                continue;
            }
            let beats = f64::from_bits(self.beats_bits.load(Ordering::Relaxed));
            let anchor_nanos = self.timestamp_nanos.load(Ordering::Relaxed);
            let after = self.sequence.load(Ordering::Acquire);
            if before == after {
                if !beats.is_finite() || anchor_nanos == 0 {
                    return None;
                }
                let now_nanos = record_clock_nanos(timestamp);
                return Some((
                    beats,
                    Duration::from_nanos(now_nanos.saturating_sub(anchor_nanos)),
                ));
            }
        }
        None
    }
}

impl Default for RecordClockAnchor {
    fn default() -> Self {
        Self::new()
    }
}

static RECORD_CLOCK_ORIGIN: OnceLock<Instant> = OnceLock::new();

fn record_clock_nanos(timestamp: Instant) -> u64 {
    timestamp
        .saturating_duration_since(*RECORD_CLOCK_ORIGIN.get_or_init(Instant::now))
        .as_nanos()
        .min(u64::MAX as u128) as u64
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RecordPosition {
    pub step: usize,
    pub phase: f32,
}

pub struct RuntimeBindingState {
    pub sampler_lids: Vec<AtomicU64>,
    pub modulator_lids: Vec<AtomicU64>,
    pub pan_lids: Vec<AtomicU64>,
    pub delay_lids: Vec<AtomicU64>,
    pub send_lids: Vec<AtomicU64>,
    pub rack_slot_pan_lids: Vec<[AtomicU64; MAX_RACK_SLOTS]>,
    pub voice_lids: Vec<[AtomicU64; MAX_VOICES]>,
    pub voice_counts: Vec<AtomicU32>,
    pub instrument_type_flags: Vec<AtomicU32>,
    pub instrument_run_mode_flags: Vec<AtomicU32>,
    pub synth_node_ids: Vec<[AtomicU32; MAX_VOICES]>,
    pub sampler_gatepitch_node_ids: Vec<[AtomicU32; MAX_VOICES]>,
    pub sampler_modulator_node_ids: Vec<[AtomicU32; MAX_VOICES]>,
    pub track_engine_ids: Vec<AtomicU32>,
    pub engine_voice_lids: Vec<[AtomicU64; MAX_VOICES]>,
    pub engine_synth_node_ids: Vec<[AtomicU32; MAX_VOICES]>,
    pub engine_modulator_node_ids: Vec<[AtomicU32; MAX_VOICES]>,
    pub engine_voice_counts: Vec<AtomicU32>,
    pub engine_route_lids: Vec<[[AtomicU64; MAX_TRACKS]; MAX_VOICES]>,
    pub engine_route_lids_r: Vec<[[AtomicU64; MAX_TRACKS]; MAX_VOICES]>,
    pub engine_ext_route_lids: Vec<[[[AtomicU64; EXT_MOD_INPUT_COUNT]; MAX_TRACKS]; MAX_VOICES]>,
    /// Per-rack-slot routes for shared custom engines. Rack slots use the same
    /// stable pool identity as sampler rack slots, so multiple slots on one
    /// track can consume one engine without manufacturing duplicate engines.
    pub rack_engine_route_lids: Vec<[AtomicU64; MAX_VOICES]>,
    pub rack_engine_route_lids_r: Vec<[AtomicU64; MAX_VOICES]>,
    pub rack_engine_route_engine_ids: Vec<AtomicU32>,
    pub rack_engine_ext_route_lids: Vec<[[AtomicU64; EXT_MOD_INPUT_COUNT]; MAX_VOICES]>,
    pub sampler_analysis_buffer_ids: Vec<AtomicU32>,
    pub sampler_analysis_bpm: Vec<AtomicU32>,
    pub sampler_onset_ptr_lo: Vec<AtomicU32>,
    pub sampler_onset_ptr_hi: Vec<AtomicU32>,
    pub sampler_analysis_status: Vec<AtomicU32>,
}

/// A sequencer definition published from the UI/editor runtime to the scheduler VM.
///
/// Two shapes share this channel:
/// - **tick mode** (`graph == None`): `tick_source` is the auto-quoted `:tick` body
///   serialized to re-evaluable lisp (see `lisp_host::sequencer_tick_source`) and
///   `resolution` is a `Timebase` index; the scheduler registers it into its generator
///   runtime.
/// - **graph mode** (`graph == Some(_)`): the whole-body manifest is carried in-process
///   as a [`crate::graph::GraphManifest`]; the scheduler materializes it into a
///   `GraphRuntime`. `tick_source`/`resolution` are unused.
///
/// The scheduler polls [`SequencerState::published_sequencers_version`] and reconciles.
#[derive(Clone, Debug, PartialEq)]
pub struct PublishedSequencer {
    pub id: u64,
    pub name: String,
    pub resolution: u8,
    pub tick_source: String,
    /// Present iff this is a graph-mode sequencer.
    pub graph: Option<crate::graph::GraphManifest>,
}

pub struct SequencerState {
    pub pattern: PatternState,
    pub transport: TransportState,
    pub runtime: RuntimeBindingState,
    scheduler_snapshot: Mutex<Arc<SequencerSnapshot>>,
    scheduler_snapshot_version: AtomicU64,
    /// Command-thread macro values waiting to be folded into the next
    /// immutable scheduler snapshot. The scheduler never reads this lock.
    live_macro_overrides: Mutex<HashMap<crate::macro_engine::MacroParamKey, f32>>,
    rack_macro_runtime_values: Arc<RackMacroRuntimeValues>,
    neural_visualization: Mutex<NeuralVisualizationSnapshot>,
    graph_visualizations: Mutex<Vec<GraphVisualizationSnapshot>>,
    track_output_events: Mutex<Vec<TrackOutputEvent>>,
    track_output_current_beat_bits: AtomicU64,
    active_note_until_samples: Vec<[AtomicU64; 128]>,
    live_note_masks: Vec<[AtomicU64; 2]>,
    audio_rendered_sample: AtomicU64,
    scratch_source: Mutex<String>,
    scratch_source_version: AtomicU64,
    published_sequencers: Mutex<Vec<PublishedSequencer>>,
    published_sequencers_version: AtomicU64,
    published_process_authoring: Mutex<crate::process::PublishedProcessAuthoringSnapshot>,
    published_process_authoring_version: AtomicU64,
    scratch_effect_descriptors: Mutex<Vec<Vec<EffectDescriptor>>>,
    scratch_instrument_descriptors: Mutex<Vec<EffectDescriptor>>,
    process_trace_enabled: AtomicBool,
    pending_accumulator_reset_all: AtomicBool,
    pending_accumulator_reset_tracks: [AtomicBool; MAX_TRACKS],
    quantized_launches: crate::quantized_launch::QuantizedLaunchMailbox,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TrackOutputEvent {
    pub track: usize,
    pub sample_time: u64,
    pub beat: f64,
    pub transpose: f32,
    pub velocity: f32,
}

const TRACK_OUTPUT_EVENT_HISTORY_CAP: usize = 1024;

#[derive(Clone, Debug, Default)]
pub struct PatternSwitchProfile {
    pub total: Duration,
    pub capture_current_snapshot: Duration,
    pub scene_lock_wait: Duration,
    pub save_current_snapshot: Duration,
    pub launch_scene_data: Duration,
    pub restore_tracks: Duration,
    pub collect_sample_ids: Duration,
    pub update_pattern_atoms: Duration,
    pub schedule_mod_resync: Duration,
    pub publish_scheduler_snapshot: Duration,
}

#[derive(Clone, Debug)]
pub struct PatternSwitchResult {
    pub sample_ids: Vec<(i32, String, u32)>,
    pub profile: PatternSwitchProfile,
}

const TOPOLOGY_EDIT_NONE: u32 = 0;
const TOPOLOGY_EDIT_DELETE_TRACK: u32 = 1;

fn capture_track_params_snapshot(track_params: &TrackParams) -> TrackParamsSnapshot {
    TrackParamsSnapshot {
        gate: track_params.is_gate_on(),
        attack_ms: track_params.get_attack_ms(),
        release_ms: track_params.get_release_ms(),
        swing: track_params.get_swing(),
        swing_resolution: track_params.get_swing_resolution(),
        num_steps: track_params.get_num_steps(),
        volume: track_params.get_volume(),
        pan: track_params.get_pan(),
        mute: track_params.is_muted(),
        solo: track_params.is_solo(),
        send: track_params.get_send(),
        output: track_params.output(),
        sends: track_params.sends(),
        polyphonic: track_params.is_polyphonic(),
        max_polyphony: track_params.get_max_polyphony(),
        timebase: track_params.get_timebase(),
        accumulator_idx: track_params.get_accumulator_idx(),
        script_accumulator_name: track_params.script_accumulator_name(),
        midi_fx_chain: track_params.midi_fx_chain(),
        midi_fx_position: track_params.get_midi_fx_position(),
        accum_limit: track_params.get_accum_limit(),
        accum_mode: track_params.get_accum_mode(),
        fts_scale: track_params.get_fts_scale(),
        mute_group: track_params.get_mute_group(),
        global_transpose: track_params.uses_global_transpose(),
    }
}

fn restore_track_params_snapshot(track_params: &TrackParams, snapshot: &TrackParamsSnapshot) {
    track_params.gate.store(snapshot.gate, Ordering::Relaxed);
    track_params.set_attack_ms(snapshot.attack_ms);
    track_params.set_release_ms(snapshot.release_ms);
    track_params.set_swing(snapshot.swing);
    track_params.set_swing_resolution(snapshot.swing_resolution);
    track_params.set_num_steps(snapshot.num_steps);
    track_params.set_volume(snapshot.volume);
    track_params.set_pan(snapshot.pan);
    track_params.set_mute(snapshot.mute);
    track_params.set_solo(snapshot.solo);
    track_params.set_send(snapshot.send);
    track_params.set_output(snapshot.output.clone());
    track_params.set_sends(snapshot.sends.clone());
    track_params.polyphonic.store(snapshot.polyphonic, Ordering::Relaxed);
    track_params.set_max_polyphony(snapshot.max_polyphony);
    track_params.set_timebase(snapshot.timebase);
    track_params.set_accumulator_idx(snapshot.accumulator_idx);
    track_params.set_script_accumulator_name(snapshot.script_accumulator_name.clone());
    track_params.set_midi_fx_chain(snapshot.midi_fx_chain.clone());
    track_params.set_midi_fx_position(snapshot.midi_fx_position);
    track_params.set_accum_limit(snapshot.accum_limit);
    track_params.set_accum_mode(snapshot.accum_mode);
    track_params.set_fts_scale(snapshot.fts_scale);
    track_params.set_mute_group(snapshot.mute_group);
    track_params.set_global_transpose(snapshot.global_transpose);
}

impl SequencerState {
    pub fn capture_current_pattern_snapshot(
        &self,
        num_tracks: usize,
        buffer_ids: &[i32],
        sample_rates: &[u32],
        names: &[String],
        instrument_types: &[InstrumentType],
    ) -> PatternSnapshot {
        let (mod_connections, neural_networks, graph_overrides) = self.current_scene_metadata();
        let mut snapshot = PatternSnapshot::capture_with_mod_connections(
            self,
            num_tracks,
            buffer_ids,
            sample_rates,
            names,
            instrument_types,
            mod_connections,
            neural_networks,
            graph_overrides,
        );
        snapshot.project_process_chain = self.project_process_chain();
        let effect_descriptors = self.scratch_effect_descriptors.lock().unwrap().clone();
        let instrument_descriptors = self.scratch_instrument_descriptors.lock().unwrap().clone();
        snapshot.refresh_process_binding_param_ids(&effect_descriptors, &instrument_descriptors);
        snapshot
    }

    pub fn new(num_tracks: usize, initial_chains: Vec<Vec<EffectSlotState>>) -> Self {
        // Initialize the shared monotonic origin off the audio thread.
        let _ = RECORD_CLOCK_ORIGIN.get_or_init(Instant::now);
        let patterns: Vec<TrackPattern> = (0..MAX_TRACKS).map(|_| TrackPattern::new()).collect();
        let neural_reset_patterns: Vec<TrackPattern> =
            (0..MAX_TRACKS).map(|_| TrackPattern::new()).collect();
        let scene_silenced: Vec<AtomicBool> =
            (0..MAX_TRACKS).map(|_| AtomicBool::new(false)).collect();
        let step_data: Vec<StepData> = (0..MAX_TRACKS).map(|_| StepData::new()).collect();
        let track_params: Vec<TrackParams> = (0..MAX_TRACKS).map(|_| TrackParams::new()).collect();
        let trigger_flash: Vec<AtomicU32> = (0..MAX_TRACKS).map(|_| AtomicU32::new(0)).collect();

        let mut effect_chains = initial_chains;
        for _ in effect_chains.len()..MAX_TRACKS {
            effect_chains.push(default_empty_effect_chain());
        }
        let midi_fx_slots = (0..MAX_TRACKS)
            .map(|_| {
                (0..crate::lisp_host::MAX_MIDI_FX_SLOTS)
                    .map(|_| EffectSlotState::empty())
                    .collect()
            })
            .collect();

        let slot_descriptors: Vec<Vec<EffectDescriptor>> = (0..num_tracks)
            .map(|_| EffectDescriptor::default_full_chain())
            .collect();

        let chord_data: Vec<ChordData> = (0..MAX_TRACKS).map(|_| ChordData::new()).collect();

        let state = Self {
            pattern: PatternState {
                patterns,
                neural_reset_patterns,
                scene_silenced,
                step_data,
                chord_data,
                track_params,
                effect_chains,
                midi_fx_slots,
                scenes: Mutex::new(ProjectScenes::from_pattern_snapshots(
                    &[PatternSnapshot::new_default(num_tracks, &slot_descriptors)],
                    0,
                )),
                current_pattern: AtomicU32::new(0),
                num_patterns: AtomicU32::new(1),
                timebase_plocks: (0..MAX_TRACKS).map(|_| TimebasePLockData::new()).collect(),
                swing_plocks: (0..MAX_TRACKS).map(|_| SwingPLockData::new()).collect(),
                swing_resolution_plocks: (0..MAX_TRACKS)
                    .map(|_| SwingResolutionPLockData::new())
                    .collect(),
                instrument_slots: (0..MAX_TRACKS).map(|_| EffectSlotState::empty()).collect(),
                instrument_base_note_offsets: (0..MAX_TRACKS)
                    .map(|_| AtomicU32::new(0.0_f32.to_bits()))
                    .collect(),
                instrument_run_modes: (0..MAX_TRACKS)
                    .map(|_| AtomicU32::new(CustomInstrumentRunMode::Instrument.runtime_flag()))
                    .collect(),
                track_sound_state: Mutex::new(
                    (0..MAX_TRACKS)
                        .map(|_| TrackSoundState::default())
                        .collect(),
                ),
                rack_tracks: Mutex::new((0..MAX_TRACKS).map(|_| None).collect()),
                process_chains: Mutex::new(
                    (0..MAX_TRACKS)
                        .map(|_| crate::process::TrackProcessChain::default())
                        .collect(),
                ),
                project_process_lane_overrides: Mutex::new(
                    (0..MAX_TRACKS).map(|_| Default::default()).collect(),
                ),
                plock_variant_registries: Mutex::new(
                    (0..MAX_TRACKS)
                        .map(|_| PlockVariantRegistry::default())
                        .collect(),
                ),
                key_lock_variant_registries: Mutex::new(
                    (0..MAX_TRACKS)
                        .map(|_| PlockVariantRegistry::default())
                        .collect(),
                ),
            },
            transport: TransportState {
                playhead: AtomicU32::new(0),
                playing: AtomicBool::new(false),
                bpm: AtomicU32::new(DEFAULT_BPM),
                master_volume: AtomicU32::new(1.0_f32.to_bits()),
                pattern_epoch: AtomicU64::new(0),
                topology_epoch: AtomicU64::new(0),
                topology_edit_kind: AtomicU32::new(TOPOLOGY_EDIT_NONE),
                topology_edit_track: AtomicU32::new(u32::MAX),
                topology_edit_request_id: AtomicU64::new(0),
                topology_edit_ready_id: AtomicU64::new(0),
                topology_edit_applied_id: AtomicU64::new(0),
                mod_reset_counter: AtomicU32::new(0),
                pending_mod_resync: AtomicBool::new(false),
                peak_l: AtomicU32::new(0.0_f32.to_bits()),
                peak_r: AtomicU32::new(0.0_f32.to_bits()),
                cpu_load_pct: AtomicU32::new(0.0_f32.to_bits()),
                trigger_flash,
                num_tracks: AtomicU32::new(num_tracks as u32),
                track_playheads: (0..MAX_TRACKS).map(|_| AtomicU32::new(0)).collect(),
                track_playhead_phases: (0..MAX_TRACKS)
                    .map(|_| AtomicU32::new(0.0_f32.to_bits()))
                    .collect(),
                sampler_playheads: (0..MAX_TRACKS)
                    .map(|_| AtomicU32::new(0.0_f32.to_bits()))
                    .collect(),
                active_voice_counts: (0..MAX_TRACKS).map(|_| AtomicU32::new(0)).collect(),
                playhead_phase: AtomicU32::new(0.0_f32.to_bits()),
                record_quantize: AtomicU32::new(
                    crate::record_quantize::RecordQuantize::DEFAULT as u32,
                ),
                record_latency_seconds: AtomicU32::new(0.0_f32.to_bits()),
                record_clock: RecordClockAnchor::new(),
                metronome_enabled: AtomicBool::new(false),
                record_quantize_thresh: AtomicU32::new(0.5_f32.to_bits()),
            },
            runtime: RuntimeBindingState {
                sampler_lids: (0..MAX_SAMPLER_POOLS).map(|_| AtomicU64::new(0)).collect(),
                modulator_lids: (0..MAX_TRACKS).map(|_| AtomicU64::new(0)).collect(),
                pan_lids: (0..MAX_TRACKS).map(|_| AtomicU64::new(0)).collect(),
                delay_lids: (0..MAX_TRACKS).map(|_| AtomicU64::new(0)).collect(),
                send_lids: (0..MAX_TRACKS).map(|_| AtomicU64::new(0)).collect(),
                rack_slot_pan_lids: (0..MAX_TRACKS)
                    .map(|_| std::array::from_fn(|_| AtomicU64::new(0)))
                    .collect(),
                voice_lids: (0..MAX_SAMPLER_POOLS)
                    .map(|_| std::array::from_fn(|_| AtomicU64::new(0)))
                    .collect(),
                voice_counts: (0..MAX_SAMPLER_POOLS).map(|_| AtomicU32::new(0)).collect(),
                instrument_type_flags: (0..MAX_TRACKS).map(|_| AtomicU32::new(0)).collect(),
                instrument_run_mode_flags: (0..MAX_TRACKS)
                    .map(|_| AtomicU32::new(CustomInstrumentRunMode::Instrument.runtime_flag()))
                    .collect(),
                synth_node_ids: (0..MAX_SAMPLER_POOLS)
                    .map(|_| std::array::from_fn(|_| AtomicU32::new(0)))
                    .collect(),
                sampler_gatepitch_node_ids: (0..MAX_SAMPLER_POOLS)
                    .map(|_| std::array::from_fn(|_| AtomicU32::new(0)))
                    .collect(),
                sampler_modulator_node_ids: (0..MAX_SAMPLER_POOLS)
                    .map(|_| std::array::from_fn(|_| AtomicU32::new(0)))
                    .collect(),
                track_engine_ids: (0..MAX_TRACKS).map(|_| AtomicU32::new(u32::MAX)).collect(),
                engine_voice_lids: (0..MAX_INSTRUMENT_ENGINES)
                    .map(|_| std::array::from_fn(|_| AtomicU64::new(0)))
                    .collect(),
                engine_synth_node_ids: (0..MAX_INSTRUMENT_ENGINES)
                    .map(|_| std::array::from_fn(|_| AtomicU32::new(0)))
                    .collect(),
                engine_modulator_node_ids: (0..MAX_INSTRUMENT_ENGINES)
                    .map(|_| std::array::from_fn(|_| AtomicU32::new(0)))
                    .collect(),
                engine_voice_counts: (0..MAX_INSTRUMENT_ENGINES)
                    .map(|_| AtomicU32::new(0))
                    .collect(),
                engine_route_lids: (0..MAX_INSTRUMENT_ENGINES)
                    .map(|_| std::array::from_fn(|_| std::array::from_fn(|_| AtomicU64::new(0))))
                    .collect(),
                engine_route_lids_r: (0..MAX_INSTRUMENT_ENGINES)
                    .map(|_| std::array::from_fn(|_| std::array::from_fn(|_| AtomicU64::new(0))))
                    .collect(),
                engine_ext_route_lids: (0..MAX_INSTRUMENT_ENGINES)
                    .map(|_| {
                        std::array::from_fn(|_| {
                            std::array::from_fn(|_| std::array::from_fn(|_| AtomicU64::new(0)))
                        })
                    })
                    .collect(),
                rack_engine_route_lids: (0..MAX_SAMPLER_POOLS)
                    .map(|_| std::array::from_fn(|_| AtomicU64::new(0)))
                    .collect(),
                rack_engine_route_lids_r: (0..MAX_SAMPLER_POOLS)
                    .map(|_| std::array::from_fn(|_| AtomicU64::new(0)))
                    .collect(),
                rack_engine_route_engine_ids: (0..MAX_SAMPLER_POOLS)
                    .map(|_| AtomicU32::new(u32::MAX))
                    .collect(),
                rack_engine_ext_route_lids: (0..MAX_SAMPLER_POOLS)
                    .map(|_| std::array::from_fn(|_| std::array::from_fn(|_| AtomicU64::new(0))))
                    .collect(),
                sampler_analysis_buffer_ids: (0..MAX_TRACKS)
                    .map(|_| AtomicU32::new(u32::MAX))
                    .collect(),
                sampler_analysis_bpm: (0..MAX_TRACKS)
                    .map(|_| AtomicU32::new(0.0_f32.to_bits()))
                    .collect(),
                sampler_onset_ptr_lo: (0..MAX_TRACKS).map(|_| AtomicU32::new(0)).collect(),
                sampler_onset_ptr_hi: (0..MAX_TRACKS).map(|_| AtomicU32::new(0)).collect(),
                sampler_analysis_status: (0..MAX_TRACKS).map(|_| AtomicU32::new(0)).collect(),
            },
            scheduler_snapshot: Mutex::new(Arc::new(SequencerSnapshot::empty())),
            scheduler_snapshot_version: AtomicU64::new(0),
            live_macro_overrides: Mutex::new(HashMap::new()),
            rack_macro_runtime_values: Arc::new(RackMacroRuntimeValues::new()),
            neural_visualization: Mutex::new(NeuralVisualizationSnapshot::default()),
            graph_visualizations: Mutex::new(Vec::new()),
            track_output_events: Mutex::new(Vec::new()),
            track_output_current_beat_bits: AtomicU64::new(0.0_f64.to_bits()),
            active_note_until_samples: (0..MAX_TRACKS)
                .map(|_| std::array::from_fn(|_| AtomicU64::new(0)))
                .collect(),
            live_note_masks: (0..MAX_TRACKS)
                .map(|_| std::array::from_fn(|_| AtomicU64::new(0)))
                .collect(),
            audio_rendered_sample: AtomicU64::new(0),
            scratch_source: Mutex::new(String::new()),
            scratch_source_version: AtomicU64::new(0),
            published_sequencers: Mutex::new(Vec::new()),
            published_sequencers_version: AtomicU64::new(0),
            published_process_authoring: Mutex::new(
                crate::process::PublishedProcessAuthoringSnapshot::default(),
            ),
            published_process_authoring_version: AtomicU64::new(0),
            scratch_effect_descriptors: Mutex::new(Vec::new()),
            scratch_instrument_descriptors: Mutex::new(Vec::new()),
            process_trace_enabled: AtomicBool::new(
                std::env::var("ESEQ_PROCESS_TRACE").is_ok_and(|value| value == "1"),
            ),
            pending_accumulator_reset_all: AtomicBool::new(false),
            pending_accumulator_reset_tracks: std::array::from_fn(|_| AtomicBool::new(false)),
            quantized_launches: crate::quantized_launch::QuantizedLaunchMailbox::default(),
        };
        state.publish_scheduler_snapshot();
        state
    }

    pub fn active_track_count(&self) -> usize {
        self.transport.num_tracks.load(Ordering::Acquire) as usize
    }

    pub fn quantized_launches(&self) -> &crate::quantized_launch::QuantizedLaunchMailbox {
        &self.quantized_launches
    }

    pub fn schedule_quantized_pattern_launch(
        &self,
        target: crate::quantized_launch::PatternLaunchTarget,
        quantize: crate::quantized_launch::LaunchQuantize,
        owner: crate::quantized_launch::QuantizedLaunchOwner,
    ) -> Result<
        crate::quantized_launch::QuantizedLaunchToken,
        crate::quantized_launch::QuantizedLaunchSubmitError,
    > {
        self.quantized_launches.schedule(
            target,
            quantize,
            owner,
            self.scene_count(),
            self.active_track_count(),
        )
    }

    pub fn is_scene_silenced(&self, track: usize) -> bool {
        self.pattern
            .scene_silenced
            .get(track)
            .map(|flag| flag.load(Ordering::Acquire))
            .unwrap_or(false)
    }

    fn set_scene_silenced(&self, track: usize, silenced: bool) {
        if let Some(flag) = self.pattern.scene_silenced.get(track) {
            flag.store(silenced, Ordering::Release);
        }
    }
    pub fn scheduler_snapshot_version(&self) -> u64 {
        self.scheduler_snapshot_version.load(Ordering::Acquire)
    }
    pub fn current_pattern_index(&self) -> usize {
        self.pattern.current_pattern.load(Ordering::Relaxed) as usize
    }

    pub fn current_scene_index(&self) -> usize {
        self.current_pattern_index()
    }

    pub(crate) fn current_scene_id(&self) -> Option<SceneId> {
        self.pattern
            .scenes
            .lock()
            .unwrap()
            .scene_id(self.current_scene_index())
    }

    pub(crate) fn scene_index(&self, id: SceneId) -> Option<usize> {
        self.pattern.scenes.lock().unwrap().scene_index(id)
    }

    pub(crate) fn effective_track_pattern_id(&self, track: usize) -> Option<PatternId> {
        self.pattern
            .scenes
            .lock()
            .unwrap()
            .effective_pattern_id(track)
    }

    pub(crate) fn live_track_params_snapshot(&self, track: usize) -> Option<TrackParamsSnapshot> {
        self.pattern
            .track_params
            .get(track)
            .map(capture_track_params_snapshot)
    }

    pub(crate) fn live_rack_track_snapshot(&self, track: usize) -> Option<RackTrackSnapshot> {
        self.pattern
            .rack_tracks
            .lock()
            .unwrap()
            .get(track)
            .cloned()
            .flatten()
    }

    pub fn scene_count(&self) -> usize {
        self.pattern.scenes.lock().unwrap().scene_count()
    }

    /// Reads one scene track in place without cloning the complete pattern.
    pub fn with_scene_track_pattern<R>(
        &self,
        scene: usize,
        track: usize,
        read: impl FnOnce(&TrackPatternData) -> R,
    ) -> Option<R> {
        let scenes = self.pattern.scenes.lock().unwrap();
        let scene = scenes.scenes.get(scene)?;
        let pattern_id = scene.cells.get(track).copied().flatten()?;
        let pattern = scenes.track_pools.get(track)?.get(pattern_id)?;
        Some(read(pattern))
    }

    pub fn pattern_repository_len(&self) -> usize {
        self.scene_count()
    }

    pub fn export_pattern_repository(&self) -> Vec<PatternSnapshot> {
        self.pattern.scenes.lock().unwrap().snapshots()
    }

    pub fn replace_pattern_repository(&self, snapshots: Vec<PatternSnapshot>, current_idx: usize) {
        let _ = self.quantized_launches.cancel_all();
        let len = snapshots.len().max(1);
        {
            let mut scenes = self.pattern.scenes.lock().unwrap();
            let bus_patterns = scenes
                .scenes
                .iter()
                .map(|scene| scene.bus_patterns.clone())
                .collect::<Vec<_>>();
            let mut rebuilt = ProjectScenes::from_pattern_snapshots(&snapshots, current_idx);
            for (scene, bus_patterns) in rebuilt.scenes.iter_mut().zip(bus_patterns) {
                scene.bus_patterns = bus_patterns;
            }
            *scenes = rebuilt;
        }
        self.pattern
            .num_patterns
            .store(len as u32, Ordering::Relaxed);
        self.pattern.current_pattern.store(
            current_idx.min(len.saturating_sub(1)) as u32,
            Ordering::Relaxed,
        );
    }

    pub fn current_pattern_sample_ids(&self) -> Vec<(i32, String, u32)> {
        let current_pattern = self.current_pattern_index();
        self.pattern
            .scenes
            .lock()
            .unwrap()
            .scene_snapshot(current_pattern)
            .map(|snapshot| snapshot.sample_ids.clone())
            .unwrap_or_default()
    }

    pub fn effective_pattern_sample_ids(&self, track_count: usize) -> Vec<(i32, String, u32)> {
        let scenes = self.pattern.scenes.lock().unwrap();
        (0..track_count)
            .map(|track| {
                scenes
                    .effective_track_pattern(track)
                    .map(|data| data.sample_id.clone())
                    .unwrap_or((-1, String::new(), 44_100))
            })
            .collect()
    }

    #[doc(hidden)]
    pub fn capture_project_scenes(&self) -> ProjectScenes {
        self.pattern.scenes.lock().unwrap().clone()
    }

    #[cfg(test)]
    pub(crate) fn with_scenes_mut<R>(&self, f: impl FnOnce(&mut ProjectScenes) -> R) -> R {
        f(&mut self.pattern.scenes.lock().unwrap())
    }

    pub(crate) fn restore_project_scenes(
        &self,
        target: &ProjectScenes,
    ) -> Result<Vec<(i32, String, u32)>, String> {
        if target.scenes.is_empty() {
            return Err("Scene history cannot restore an empty project".to_string());
        }
        if target.track_pools.len() != self.active_track_count()
            || target.track_overrides.len() != self.active_track_count()
            || target.scenes.iter().any(|scene| scene.cells.len() != self.active_track_count())
        {
            return Err("Scene history track topology no longer matches the project".to_string());
        }
        if target.current_scene >= target.scenes.len() {
            return Err("Scene history has an invalid current-scene index".to_string());
        }
        let unique_scene_ids = target
            .scenes
            .iter()
            .map(|scene| scene.id)
            .collect::<HashSet<_>>();
        if unique_scene_ids.len() != target.scenes.len()
            || unique_scene_ids.iter().any(|id| id.0 == 0)
            || target.next_scene_id == 0
            || unique_scene_ids
                .iter()
                .any(|id| id.0 >= target.next_scene_id)
        {
            return Err("Scene history contains invalid or duplicate scene identities".to_string());
        }
        let _ = self.quantized_launches.cancel_all();
        *self.pattern.scenes.lock().unwrap() = target.clone();
        self.pattern.current_pattern.store(target.current_scene as u32, Ordering::Relaxed);
        self.pattern.num_patterns.store(target.scenes.len() as u32, Ordering::Relaxed);
        let sample_ids = self.restore_current_pattern_from_repository()
            .ok_or_else(|| "Scene history could not restore the current scene".to_string())?;
        self.transport.pattern_epoch.fetch_add(1, Ordering::Relaxed);
        self.schedule_mod_resync();
        self.publish_scheduler_snapshot();
        Ok(sample_ids)
    }

    pub fn restore_current_pattern_from_repository(&self) -> Option<Vec<(i32, String, u32)>> {
        let current_pattern = self.current_pattern_index();
        let sample_ids = {
            let mut scenes = self.pattern.scenes.lock().unwrap();
            let launched = scenes.launch_scene(current_pattern)?;
            for (track, data) in launched.into_iter().enumerate() {
                if let Some(data) = data {
                    data.restore_to(self, track);
                    self.set_scene_silenced(track, false);
                } else {
                    self.set_scene_silenced(track, true);
                }
            }
            scenes
                .scene_snapshot(current_pattern)
                .map(|snapshot| snapshot.sample_ids)
                .unwrap_or_default()
        };
        Some(sample_ids)
    }

    pub fn save_current_pattern_snapshot(
        &self,
        num_tracks: usize,
        buffer_ids: &[i32],
        sample_rates: &[u32],
        names: &[String],
        instrument_types: &[InstrumentType],
    ) -> bool {
        let current_pattern = self.current_pattern_index();
        let snapshot = self.capture_current_pattern_snapshot(
            num_tracks,
            buffer_ids,
            sample_rates,
            names,
            instrument_types,
        );
        self.pattern
            .scenes
            .lock()
            .unwrap()
            .save_scene_snapshot(current_pattern, snapshot)
    }

    pub fn save_current_track_midi_fx_snapshot(&self, track: usize) -> bool {
        let current_pattern = self.current_pattern_index();
        let mut scenes = self.pattern.scenes.lock().unwrap();
        let Some(mut snapshot) = scenes.scene_snapshot(current_pattern) else {
            return false;
        };
        if track >= self.pattern.track_params.len()
            || track >= self.pattern.midi_fx_slots.len()
            || track >= snapshot.track_params.len()
            || track >= snapshot.midi_fx_slots.len()
        {
            return false;
        }

        snapshot.track_params[track] =
            capture_track_params_snapshot(&self.pattern.track_params[track]);
        snapshot.midi_fx_slots[track] = self.pattern.midi_fx_slots[track]
            .iter()
            .map(EffectSlotSnapshot::capture)
            .collect();
        scenes.save_scene_snapshot(current_pattern, snapshot)
    }

    pub fn save_current_track_effect_snapshot(&self, track: usize) -> bool {
        let current_pattern = self.current_pattern_index();
        let mut scenes = self.pattern.scenes.lock().unwrap();
        let Some(mut snapshot) = scenes.scene_snapshot(current_pattern) else {
            return false;
        };
        if track >= self.pattern.effect_chains.len() || track >= snapshot.effect_slots.len() {
            return false;
        }

        snapshot.effect_slots[track] = self.pattern.effect_chains[track]
            .iter()
            .map(EffectSlotSnapshot::capture)
            .collect();
        let effect_descriptors = self.scratch_effect_descriptors.lock().unwrap().clone();
        let instrument_descriptors = self.scratch_instrument_descriptors.lock().unwrap().clone();
        snapshot.refresh_process_binding_param_ids(&effect_descriptors, &instrument_descriptors);
        scenes.save_scene_snapshot(current_pattern, snapshot)
    }

    pub fn copy_current_effect_values_to_all_track_patterns(
        &self,
        track: usize,
        slot_idx: usize,
    ) -> usize {
        let Some(source_slot) = self
            .pattern
            .effect_chains
            .get(track)
            .and_then(|slots| slots.get(slot_idx))
            .map(EffectSlotSnapshot::capture)
        else {
            return 0;
        };
        let mut scenes = self.pattern.scenes.lock().unwrap();
        let Some(pool) = scenes.track_pools.get_mut(track) else {
            return 0;
        };
        let mut updated = 0;
        for pattern in pool.patterns.values_mut() {
            if let Some(slot) = pattern.effect_slots.get_mut(slot_idx) {
                slot.copy_base_values_from(&source_slot);
                updated += 1;
            }
        }
        updated
    }

    pub fn copy_current_midi_fx_values_to_all_track_patterns(
        &self,
        track: usize,
        slot_idx: usize,
    ) -> usize {
        let Some(source_slot) = self
            .pattern
            .midi_fx_slots
            .get(track)
            .and_then(|slots| slots.get(slot_idx))
            .map(EffectSlotSnapshot::capture)
        else {
            return 0;
        };
        let mut scenes = self.pattern.scenes.lock().unwrap();
        let Some(pool) = scenes.track_pools.get_mut(track) else {
            return 0;
        };
        let mut updated = 0;
        for pattern in pool.patterns.values_mut() {
            if let Some(slot) = pattern.midi_fx_slots.get_mut(slot_idx) {
                slot.copy_base_values_from(&source_slot);
                updated += 1;
            }
        }
        updated
    }

    pub fn copy_current_instrument_values_to_all_track_patterns(&self, track: usize) -> usize {
        let Some(source_slot) = self
            .pattern
            .instrument_slots
            .get(track)
            .map(EffectSlotSnapshot::capture)
        else {
            return 0;
        };
        let Some(source_base_note) = self.pattern.instrument_base_note_offsets.get(track) else {
            return 0;
        };
        let source_base_note = f32::from_bits(source_base_note.load(Ordering::Relaxed));
        let mut scenes = self.pattern.scenes.lock().unwrap();
        let Some(pool) = scenes.track_pools.get_mut(track) else {
            return 0;
        };
        for pattern in pool.patterns.values_mut() {
            pattern.instrument_slot.copy_base_values_from(&source_slot);
            pattern.instrument_base_note_offset = source_base_note;
        }
        pool.patterns.len()
    }

    /// Validates the shared state lanes needed to replace a track's instrument
    /// source without mutating any state.
    fn validate_instrument_source_reset_target(&self, track: usize) -> Result<(), String> {
        let require_track = |len: usize, collection: &str| {
            if track < len {
                Ok(())
            } else {
                Err(format!(
                    "Track {} is missing from {collection} (length {len})",
                    track + 1
                ))
            }
        };
        require_track(self.pattern.instrument_slots.len(), "live instrument slots")?;
        require_track(
            self.pattern.instrument_run_modes.len(),
            "pattern instrument run modes",
        )?;
        require_track(
            self.runtime.instrument_run_mode_flags.len(),
            "runtime instrument run modes",
        )?;
        require_track(
            self.runtime.instrument_type_flags.len(),
            "runtime instrument types",
        )?;
        require_track(
            self.runtime.track_engine_ids.len(),
            "runtime track engine bindings",
        )?;
        require_track(
            self.pattern.track_sound_state.lock().unwrap().len(),
            "track sound state",
        )?;
        require_track(
            self.pattern.process_chains.lock().unwrap().len(),
            "process chains",
        )?;
        require_track(
            self.pattern.rack_tracks.lock().unwrap().len(),
            "rack track state",
        )?;
        require_track(
            self.pattern.plock_variant_registries.lock().unwrap().len(),
            "p-lock variant registries",
        )?;
        require_track(
            self.pattern
                .key_lock_variant_registries
                .lock()
                .unwrap()
                .len(),
            "key-lock variant registries",
        )?;
        require_track(
            self.pattern.scenes.lock().unwrap().track_pools.len(),
            "stored track pattern pools",
        )?;
        Ok(())
    }

    pub fn validate_instrument_slot_reset_target(
        &self,
        track: usize,
        engine_id: usize,
    ) -> Result<(), String> {
        u32::try_from(engine_id)
            .map_err(|_| format!("Instrument engine id {engine_id} exceeds the runtime format"))?;
        self.validate_instrument_source_reset_target(track)
    }

    pub fn validate_sampler_slot_reset_target(&self, track: usize) -> Result<(), String> {
        self.validate_instrument_source_reset_target(track)
    }

    pub fn reset_instrument_slot_all_patterns(
        &self,
        track: usize,
        descriptor: &EffectDescriptor,
        node_id: u32,
        modulator_node_id: u32,
        engine_id: usize,
        run_mode: CustomInstrumentRunMode,
    ) -> Option<InstrumentSlotResetSummary> {
        self.validate_instrument_slot_reset_target(track, engine_id)
            .ok()?;
        self.reset_instrument_source_all_patterns(
            track,
            descriptor,
            node_id,
            modulator_node_id,
            InstrumentSourceReset::Custom {
                engine_id,
                run_mode,
            },
        )
    }

    pub fn reset_sampler_slot_all_patterns(
        &self,
        track: usize,
        descriptor: &EffectDescriptor,
        node_id: u32,
        modulator_node_id: u32,
        sample_id: (i32, String, u32),
    ) -> Option<InstrumentSlotResetSummary> {
        self.validate_sampler_slot_reset_target(track).ok()?;
        self.reset_instrument_source_all_patterns(
            track,
            descriptor,
            node_id,
            modulator_node_id,
            InstrumentSourceReset::Sampler { sample_id },
        )
    }

    fn reset_instrument_source_all_patterns(
        &self,
        track: usize,
        descriptor: &EffectDescriptor,
        node_id: u32,
        modulator_node_id: u32,
        source: InstrumentSourceReset,
    ) -> Option<InstrumentSlotResetSummary> {
        let engine_id_flag = source
            .engine_id()
            .and_then(|engine_id| u32::try_from(engine_id).ok())
            .unwrap_or(u32::MAX);
        let live_slot = self.pattern.instrument_slots.get(track)?;
        let live_had_locks = instrument_slot_has_locks(&EffectSlotSnapshot::capture(live_slot));

        live_slot.clear();
        live_slot.apply_descriptor_with_modulator(descriptor, node_id, modulator_node_id);
        self.pattern.instrument_run_modes[track]
            .store(source.run_mode().runtime_flag(), Ordering::Relaxed);
        self.runtime.instrument_run_mode_flags[track]
            .store(source.run_mode().runtime_flag(), Ordering::Release);
        self.runtime.instrument_type_flags[track]
            .store(source.instrument_type().runtime_flag(), Ordering::Release);
        self.runtime.track_engine_ids[track].store(engine_id_flag, Ordering::Release);

        self.pattern.track_sound_state.lock().unwrap()[track] = TrackSoundState {
            engine_id: source.engine_id(),
            loaded_preset: None,
            dirty: false,
        };
        self.pattern.rack_tracks.lock().unwrap()[track] = None;

        self.pattern.plock_variant_registries.lock().unwrap()[track]
            .remove_domains(INSTRUMENT_PLOCK_VARIANT_DOMAINS);
        self.pattern.key_lock_variant_registries.lock().unwrap()[track]
            .remove_domains(INSTRUMENT_PLOCK_VARIANT_DOMAINS);

        let live_instrument_slot = EffectSlotSnapshot::capture(live_slot);
        let mut process_bindings_dropped = {
            let mut chains = self.pattern.process_chains.lock().unwrap();
            crate::process::rebind_track_process_chain_instrument_param_ids(
                &mut chains[track],
                descriptor,
                &live_instrument_slot,
            )
        };

        let mut scenes = self.pattern.scenes.lock().unwrap();
        let neural_overrides_dropped = scenes
            .scenes
            .iter_mut()
            .map(|scene| {
                crate::neural::remove_instrument_overrides_for_track(
                    &mut scene.neural_networks,
                    track,
                )
            })
            .sum();
        let effective_pattern_id = scenes.effective_pattern_id(track);
        let pool = &mut scenes.track_pools[track];
        let mut patterns_with_cleared_locks = 0;
        for (pattern_id, data) in &mut pool.patterns {
            let stored_had_locks = instrument_slot_has_locks(&data.instrument_slot);
            let (cleared_locks, dropped_bindings) =
                data.reset_instrument_source(descriptor, node_id, modulator_node_id, &source);
            let cleared_locks = if Some(*pattern_id) == effective_pattern_id {
                live_had_locks || stored_had_locks
            } else {
                cleared_locks
            };
            patterns_with_cleared_locks += usize::from(cleared_locks);
            process_bindings_dropped += dropped_bindings;
        }

        Some(InstrumentSlotResetSummary {
            patterns_reset: pool.patterns.len(),
            patterns_with_cleared_locks,
            process_bindings_dropped,
            neural_overrides_dropped,
        })
    }

    pub fn capture_track_instrument_pattern_state(
        &self,
        track: usize,
    ) -> Result<TrackInstrumentPatternStateSnapshot, String> {
        self.validate_instrument_source_reset_target(track)?;
        let (mut live, patterns, neural_overrides) = {
            let scenes = self.pattern.scenes.lock().unwrap();
            let pool = scenes
                .track_pools
                .get(track)
                .ok_or_else(|| format!("Track {} has no pattern pool", track + 1))?;
            let effective_id = scenes
                .effective_pattern_id(track)
                .ok_or_else(|| format!("Track {} has no effective pattern", track + 1))?;
            let live = pool
                .patterns
                .get(&effective_id)
                .ok_or_else(|| format!("Track {} effective pattern is missing", track + 1))?
                .instrument_state();
            let patterns = pool
                .patterns
                .iter()
                .map(|(id, data)| (*id, data.instrument_state()))
                .collect();
            let mut neural_overrides = Vec::new();
            for (scene_idx, scene) in scenes.scenes.iter().enumerate() {
                for (network_idx, network) in scene.neural_networks.iter().enumerate() {
                    for (neuron_idx, neuron) in network.neurons.iter().enumerate() {
                        let entries = neuron
                            .output_overrides
                            .instrument
                            .iter()
                            .enumerate()
                            .filter(|(_, value)| value.target_track == track)
                            .map(|(idx, value)| (idx, value.clone()))
                            .collect::<Vec<_>>();
                        if !entries.is_empty() {
                            neural_overrides.push(NeuralInstrumentOverrideState {
                                scene: scene_idx,
                                network: network_idx,
                                neuron: neuron_idx,
                                entries,
                            });
                        }
                    }
                }
            }
            (live, patterns, neural_overrides)
        };
        live.instrument_slot = EffectSlotSnapshot::capture(&self.pattern.instrument_slots[track]);
        live.instrument_base_note_offset = f32::from_bits(
            self.pattern.instrument_base_note_offsets[track].load(Ordering::Relaxed),
        );
        live.instrument_run_mode = CustomInstrumentRunMode::from_runtime_flag(
            self.pattern.instrument_run_modes[track].load(Ordering::Relaxed),
        );
        live.track_sound_state = self.pattern.track_sound_state.lock().unwrap()[track].clone();
        live.rack_track = self.pattern.rack_tracks.lock().unwrap()[track].clone();
        live.process_chain = self.pattern.process_chains.lock().unwrap()[track].clone();
        live.project_process_lane_overrides =
            self.pattern.project_process_lane_overrides.lock().unwrap()[track].clone();
        live.plock_variant_registry =
            self.pattern.plock_variant_registries.lock().unwrap()[track].clone();
        live.key_lock_variant_registry = self.pattern.key_lock_variant_registries.lock().unwrap()
            [track]
            .clone();

        Ok(TrackInstrumentPatternStateSnapshot {
            live,
            patterns,
            neural_overrides,
        })
    }

    pub fn restore_track_instrument_pattern_state(
        &self,
        track: usize,
        snapshot: &TrackInstrumentPatternStateSnapshot,
        descriptor: &EffectDescriptor,
        node_id: u32,
        modulator_node_id: u32,
    ) -> Result<(), String> {
        self.validate_track_instrument_pattern_state(track, snapshot, descriptor)?;
        let mut live = snapshot.live.clone();
        live.instrument_slot.sync_to_descriptor_with_modulator(
            descriptor,
            node_id,
            modulator_node_id,
        );
        let mut scenes = self.pattern.scenes.lock().unwrap();
        let pool = scenes
            .track_pools
            .get_mut(track)
            .ok_or_else(|| format!("Track {} has no pattern pool", track + 1))?;
        if pool.patterns.len() != snapshot.patterns.len()
            || snapshot
                .patterns
                .iter()
                .any(|(id, _)| !pool.patterns.contains_key(id))
        {
            return Err(format!(
                "Track {} pattern set changed before instrument history replay",
                track + 1
            ));
        }
        for (id, state) in &snapshot.patterns {
            pool.patterns
                .get_mut(id)
                .expect("pattern set was validated")
                .restore_instrument_state(state, descriptor, node_id, modulator_node_id);
        }
        for scene in &mut scenes.scenes {
            for network in &mut scene.neural_networks {
                for neuron in &mut network.neurons {
                    neuron
                        .output_overrides
                        .instrument
                        .retain(|value| value.target_track != track);
                }
            }
        }
        for saved in &snapshot.neural_overrides {
            let neuron = scenes
                .scenes
                .get_mut(saved.scene)
                .and_then(|scene| scene.neural_networks.get_mut(saved.network))
                .and_then(|network| network.neurons.get_mut(saved.neuron))
                .ok_or_else(|| {
                    format!(
                        "Track {} neural topology changed before instrument history replay",
                        track + 1
                    )
                })?;
            for (index, value) in &saved.entries {
                let index = (*index).min(neuron.output_overrides.instrument.len());
                let mut value = value.clone();
                let raw_idx = live
                    .instrument_slot
                    .param_node_indices
                    .get(value.param_index)
                    .copied()
                    .unwrap_or(value.param_index as u32);
                value.param_id = crate::neural::ParamNodeId::from_slot_param(
                    live.instrument_slot.node_id,
                    live.instrument_slot.modulator_node_id,
                    raw_idx,
                )
                .ok_or_else(|| {
                    format!(
                        "Track {} instrument parameter {} has no live identity",
                        track + 1,
                        value.param_index
                    )
                })?;
                neuron
                    .output_overrides
                    .instrument
                    .insert(index, value);
            }
        }
        drop(scenes);

        crate::process::refresh_track_process_chain_binding_param_ids(
            &mut live.process_chain,
            Some(descriptor),
            Some(&live.instrument_slot),
            &[],
            &[],
        );
        live.instrument_slot.restore(&self.pattern.instrument_slots[track]);
        self.pattern.instrument_base_note_offsets[track]
            .store(live.instrument_base_note_offset.to_bits(), Ordering::Relaxed);
        self.pattern.instrument_run_modes[track]
            .store(live.instrument_run_mode.runtime_flag(), Ordering::Relaxed);
        self.runtime.instrument_run_mode_flags[track]
            .store(live.instrument_run_mode.runtime_flag(), Ordering::Release);
        self.pattern.track_sound_state.lock().unwrap()[track] = live.track_sound_state;
        self.pattern.rack_tracks.lock().unwrap()[track] = live.rack_track;
        self.pattern.process_chains.lock().unwrap()[track] = live.process_chain;
        self.pattern.project_process_lane_overrides.lock().unwrap()[track] =
            live.project_process_lane_overrides;
        self.pattern.plock_variant_registries.lock().unwrap()[track] =
            live.plock_variant_registry;
        self.pattern.key_lock_variant_registries.lock().unwrap()[track] =
            live.key_lock_variant_registry;
        Ok(())
    }

    pub fn validate_track_instrument_pattern_state(
        &self,
        track: usize,
        snapshot: &TrackInstrumentPatternStateSnapshot,
        descriptor: &EffectDescriptor,
    ) -> Result<(), String> {
        self.validate_instrument_source_reset_target(track)?;
        let scenes = self.pattern.scenes.lock().unwrap();
        let pool = scenes
            .track_pools
            .get(track)
            .ok_or_else(|| format!("Track {} has no pattern pool", track + 1))?;
        if pool.patterns.len() != snapshot.patterns.len()
            || snapshot
                .patterns
                .iter()
                .any(|(id, _)| !pool.patterns.contains_key(id))
        {
            return Err(format!(
                "Track {} pattern set changed before instrument history replay",
                track + 1
            ));
        }
        for saved in &snapshot.neural_overrides {
            if scenes
                .scenes
                .get(saved.scene)
                .and_then(|scene| scene.neural_networks.get(saved.network))
                .and_then(|network| network.neurons.get(saved.neuron))
                .is_none()
            {
                return Err(format!(
                    "Track {} neural topology changed before instrument history replay",
                    track + 1
                ));
            }
            if saved.entries.iter().any(|(_, value)| {
                value.param_index >= descriptor.params.len()
                    || value.target_track != track
            }) {
                return Err(format!(
                    "Track {} neural instrument override no longer matches its descriptor",
                    track + 1
                ));
            }
        }
        Ok(())
    }

    pub fn copy_current_rack_slot_instrument_values_to_all_track_patterns(
        &self,
        track: usize,
        rack_slot_idx: usize,
    ) -> usize {
        let source = self
            .pattern
            .rack_tracks
            .lock()
            .unwrap()
            .get(track)
            .and_then(Option::as_ref)
            .and_then(|rack| rack.slots.get(rack_slot_idx))
            .cloned();
        let Some(source) = source else {
            return 0;
        };
        let mut scenes = self.pattern.scenes.lock().unwrap();
        let Some(pool) = scenes.track_pools.get_mut(track) else {
            return 0;
        };
        let mut updated = 0;
        for pattern in pool.patterns.values_mut() {
            let Some(slot) = pattern
                .rack_track
                .as_mut()
                .and_then(|rack| rack.slots.get_mut(rack_slot_idx))
            else {
                continue;
            };
            slot.instrument_slot
                .copy_base_values_from(&source.instrument_slot);
            slot.instrument_base_note_offset = source.instrument_base_note_offset;
            updated += 1;
        }
        updated
    }

    pub fn sync_effect_slot_with_modulator_in_track_patterns(
        &self,
        track: usize,
        slot_idx: usize,
        descriptor: &EffectDescriptor,
        node_id: u32,
        modulator_node_id: u32,
    ) {
        self.save_current_track_effect_snapshot(track);
        let mut scenes = self.pattern.scenes.lock().unwrap();
        scenes.edit_other_track_patterns(track, |data| {
            data.sync_effect_slot_with_modulator(slot_idx, descriptor, node_id, modulator_node_id);
            data.refresh_process_effect_binding_param_ids_for_slot(slot_idx, descriptor);
        });
    }

    pub fn normalize_current_pattern_instrument_run_mode(
        &self,
        track_count: usize,
        slot_descriptors: &[Vec<EffectDescriptor>],
        track: usize,
        run_mode: CustomInstrumentRunMode,
    ) {
        self.extend_all_pattern_snapshots_to_track(
            track_count,
            slot_descriptors,
            track,
            run_mode,
            None,
        );
        let mut scenes = self.pattern.scenes.lock().unwrap();
        let Some(id) = scenes.effective_pattern_id(track) else {
            return;
        };
        if let Some(data) = scenes
            .track_pools
            .get_mut(track)
            .and_then(|pool| pool.get_mut(id))
        {
            data.instrument_run_mode = run_mode;
        }
    }

    fn default_track_pattern_data_for_track(
        track_count: usize,
        slot_descriptors: &[Vec<EffectDescriptor>],
        track: usize,
        run_mode: CustomInstrumentRunMode,
        instrument: Option<(&EffectDescriptor, u32, u32, InstrumentType)>,
    ) -> Option<TrackPatternData> {
        let mut snapshot = PatternSnapshot::new_default(track_count, slot_descriptors);
        if let Some(mode) = snapshot.instrument_run_modes.get_mut(track) {
            *mode = run_mode;
        }
        if let Some((descriptor, node_id, modulator_node_id, instrument_type)) = instrument {
            snapshot.sync_instrument_slot(
                track,
                descriptor,
                node_id,
                modulator_node_id,
                instrument_type,
            );
        }
        snapshot.track_pattern_data(track)
    }

    pub fn extend_all_pattern_snapshots_to_track(
        &self,
        track_count: usize,
        slot_descriptors: &[Vec<EffectDescriptor>],
        track: usize,
        run_mode: CustomInstrumentRunMode,
        instrument: Option<(&EffectDescriptor, u32, u32, InstrumentType)>,
    ) {
        let Some(default_data) = Self::default_track_pattern_data_for_track(
            track_count,
            slot_descriptors,
            track,
            run_mode,
            instrument,
        ) else {
            return;
        };
        let mut scenes = self.pattern.scenes.lock().unwrap();
        while scenes.track_pools.len() < track_count {
            scenes.track_pools.push(TrackPatternPool::default());
            scenes.track_overrides.push(None);
            for scene in &mut scenes.scenes {
                scene.cells.push(None);
            }
        }
        if track >= scenes.track_pools.len() {
            return;
        }
        for scene_idx in 0..scenes.scenes.len() {
            while scenes.scenes[scene_idx].cells.len() < track_count {
                scenes.scenes[scene_idx].cells.push(None);
            }
            if scenes.scenes[scene_idx].cells[track].is_none() {
                let id = scenes.track_pools[track].insert(default_data.clone());
                scenes.scenes[scene_idx].cells[track] = Some(id);
            }
        }
    }

    /// Seed `sample_id` onto every stored pattern of `track` that has never had
    /// a real sample chosen (negative buffer id). Patterns with an explicit
    /// sample keep it. Returns the number of patterns seeded.
    pub fn seed_unset_pattern_sample_ids(
        &self,
        track: usize,
        sample_id: (i32, String, u32),
    ) -> usize {
        if sample_id.0 < 0 {
            return 0;
        }
        let mut scenes = self.pattern.scenes.lock().unwrap();
        let Some(pool) = scenes.track_pools.get_mut(track) else {
            return 0;
        };
        let mut seeded = 0;
        for data in pool.patterns.values_mut() {
            if data.sample_id.0 < 0 {
                data.sample_id = sample_id.clone();
                seeded += 1;
            }
        }
        seeded
    }

    pub fn set_rack_track_for_all_pattern_snapshots(
        &self,
        track: usize,
        rack_track: RackTrackSnapshot,
    ) {
        if let Some(live_rack_track) = self.pattern.rack_tracks.lock().unwrap().get_mut(track) {
            *live_rack_track = Some(rack_track.clone());
        }
        let mut scenes = self.pattern.scenes.lock().unwrap();
        let Some(pool) = scenes.track_pools.get_mut(track) else {
            return;
        };
        for data in pool.patterns.values_mut() {
            prepare_track_pattern_data_for_rack(data);
            data.rack_track = Some(rack_track.clone());
        }
    }

    pub fn replace_instrument_container_with_rack(
        &self,
        track: usize,
        rack_track: RackTrackSnapshot,
    ) -> bool {
        if self.validate_instrument_source_reset_target(track).is_err() {
            return false;
        }
        self.set_rack_track_for_all_pattern_snapshots(track, rack_track);
        self.pattern.instrument_slots[track].clear();
        self.pattern.instrument_run_modes[track].store(
            CustomInstrumentRunMode::Instrument.runtime_flag(),
            Ordering::Release,
        );
        self.runtime.instrument_run_mode_flags[track].store(
            CustomInstrumentRunMode::Instrument.runtime_flag(),
            Ordering::Release,
        );
        self.runtime.instrument_type_flags[track]
            .store(InstrumentType::Rack.runtime_flag(), Ordering::Release);
        self.runtime.track_engine_ids[track].store(u32::MAX, Ordering::Release);
        true
    }

    /// Fold a flat track's instrument and insert chain into a one-slot rack in
    /// every stored pattern. Per-pattern instrument/effect values are retained;
    /// only their ownership moves from the track container to the rack slot.
    pub fn validate_group_flat_track_to_rack(&self, track: usize) -> Result<(), String> {
        self.validate_instrument_source_reset_target(track)?;
        let scenes = self.pattern.scenes.lock().unwrap();
        let effective_pattern_id = scenes
            .effective_pattern_id(track)
            .ok_or_else(|| format!("Track {} has no effective pattern", track + 1))?;
        let pool = scenes
            .track_pools
            .get(track)
            .ok_or_else(|| format!("Track {} has no stored pattern pool", track + 1))?;
        if !pool.patterns.contains_key(&effective_pattern_id) {
            return Err(format!(
                "Track {} effective pattern is missing from its pool",
                track + 1
            ));
        }
        for (scene_idx, scene) in scenes.scenes.iter().enumerate() {
            let Some(pattern_id) = scene.cells.get(track).copied().flatten() else {
                continue;
            };
            if !pool.contains(pattern_id) {
                return Err(format!(
                    "Track {} scene {} references a missing pattern",
                    track + 1,
                    scene_idx + 1
                ));
            }
        }
        if let Some(pattern_id) = scenes.track_overrides.get(track).copied().flatten() {
            if !pool.contains(pattern_id) {
                return Err(format!(
                    "Track {} override references a missing pattern",
                    track + 1
                ));
            }
        }
        Ok(())
    }

    pub fn group_flat_track_to_rack(
        &self,
        track: usize,
        instrument_type: InstrumentType,
        instrument_run_mode: CustomInstrumentRunMode,
        engine_id: Option<usize>,
        effect_descriptors: &[EffectDescriptor],
        custom_effect_names: &[Option<String>],
    ) -> Option<RackTrackSnapshot> {
        self.validate_group_flat_track_to_rack(track).ok()?;
        let live_instrument =
            EffectSlotSnapshot::capture(self.pattern.instrument_slots.get(track)?);
        let live_effects = self
            .pattern
            .effect_chains
            .get(track)?
            .iter()
            .map(EffectSlotSnapshot::capture)
            .collect::<Vec<_>>();

        let make_slot = |data: &TrackPatternData| {
            let mut track_sound_state = data.track_sound_state.clone();
            track_sound_state.engine_id = engine_id;
            let mut slot = RackSlotSnapshot {
                instrument_type,
                instrument_run_mode,
                instrument_base_note_offset: data.instrument_base_note_offset,
                pad_note: None,
                choke_group: None,
                gain: 1.0,
                pan: 0.0,
                mute: false,
                solo: false,
                max_polyphony: crate::voice::MAX_VOICES,
                param_plocks: RackSlotParamPlocks::new(),
                instrument_slot: data.instrument_slot.clone(),
                effect_slots: data.effect_slots.clone(),
                effect_descriptors: effect_descriptors.to_vec(),
                custom_effect_names: custom_effect_names.to_vec(),
                track_sound_state,
                sample_id: (instrument_type == InstrumentType::Sampler)
                    .then(|| data.sample_id.clone()),
            };
            slot.normalize_effect_chain();
            slot
        };

        let live_rack = {
            let mut scenes = self.pattern.scenes.lock().unwrap();
            let effective_id = scenes.effective_pattern_id(track)?;
            let pool = scenes.track_pools.get_mut(track)?;
            let mut effective_rack = None;
            // Pattern ids are stable identities shared by scene cells and the
            // optional launched-pattern override. Migrating each pool entry in
            // place preserves those mappings without cloning or re-keying any
            // pattern.
            for (pattern_id, data) in pool.patterns.iter_mut() {
                let slot = make_slot(data);
                if *pattern_id == effective_id {
                    effective_rack = Some(RackTrackSnapshot {
                        routing: RackRouting::Broadcast,
                        slots: vec![slot.clone()],
                        macros: default_rack_macros(),
                        runtime_macro_values: None,
                        runtime_macro_track: 0,
                    });
                }
                prepare_track_pattern_data_for_rack(data);
                data.effect_slots = (0..crate::lisp_host::MAX_CUSTOM_FX)
                    .map(|_| EffectSlotSnapshot::new_empty())
                    .collect();
                data.rack_track = Some(RackTrackSnapshot {
                    routing: RackRouting::Broadcast,
                    slots: vec![slot],
                    macros: default_rack_macros(),
                    runtime_macro_values: None,
                    runtime_macro_track: 0,
                });
            }
            effective_rack?
        };

        let mut live_rack = live_rack;
        live_rack.slots[0].instrument_slot = live_instrument;
        live_rack.slots[0].effect_slots = live_effects;
        live_rack.slots[0].normalize_effect_chain();
        self.pattern.rack_tracks.lock().unwrap()[track] = Some(live_rack.clone());
        self.pattern.instrument_slots[track].clear();
        for slot in &self.pattern.effect_chains[track] {
            slot.clear();
        }
        self.pattern.instrument_run_modes[track].store(
            CustomInstrumentRunMode::Instrument.runtime_flag(),
            Ordering::Release,
        );
        self.runtime.instrument_run_mode_flags[track].store(
            CustomInstrumentRunMode::Instrument.runtime_flag(),
            Ordering::Release,
        );
        self.runtime.instrument_type_flags[track]
            .store(InstrumentType::Rack.runtime_flag(), Ordering::Release);
        self.runtime.track_engine_ids[track].store(u32::MAX, Ordering::Release);
        Some(live_rack)
    }

    pub fn append_rack_slot_to_current_pattern(
        &self,
        track: usize,
        routing: RackRouting,
        slot: RackSlotSnapshot,
    ) -> bool {
        if self
            .pattern
            .rack_tracks
            .lock()
            .unwrap()
            .get(track)
            .is_none()
        {
            return false;
        }
        {
            let scenes = self.pattern.scenes.lock().unwrap();
            let Some(id) = scenes.effective_pattern_id(track) else {
                return false;
            };
            if scenes
                .track_pools
                .get(track)
                .and_then(|pool| pool.get(id))
                .is_none()
            {
                return false;
            }
        }

        if let Some(live_rack_track) = self.pattern.rack_tracks.lock().unwrap().get_mut(track) {
            match live_rack_track {
                Some(rack_track) => rack_track.slots.push(slot.clone()),
                None => {
                    *live_rack_track = Some(RackTrackSnapshot {
                        routing,
                        slots: vec![slot.clone()],
                        macros: default_rack_macros(),
                        runtime_macro_values: None,
                        runtime_macro_track: 0,
                    });
                }
            }
        } else {
            return false;
        }

        let mut scenes = self.pattern.scenes.lock().unwrap();
        let id = scenes
            .effective_pattern_id(track)
            .expect("validated current pattern id before rack slot append");
        let data = scenes
            .track_pools
            .get_mut(track)
            .and_then(|pool| pool.get_mut(id))
            .expect("validated current pattern data before rack slot append");
        prepare_track_pattern_data_for_rack(data);
        match data.rack_track.as_mut() {
            Some(rack_track) => rack_track.slots.push(slot),
            None => {
                data.rack_track = Some(RackTrackSnapshot {
                    routing,
                    slots: vec![slot],
                    macros: default_rack_macros(),
                    runtime_macro_values: None,
                    runtime_macro_track: 0,
                });
            }
        }
        true
    }

    pub fn append_rack_slot_for_all_pattern_snapshots(
        &self,
        track: usize,
        routing: RackRouting,
        slot: RackSlotSnapshot,
    ) {
        if let Some(live_rack_track) = self.pattern.rack_tracks.lock().unwrap().get_mut(track) {
            match live_rack_track {
                Some(rack_track) => rack_track.slots.push(slot.clone()),
                None => {
                    *live_rack_track = Some(RackTrackSnapshot {
                        routing,
                        slots: vec![slot.clone()],
                        macros: default_rack_macros(),
                        runtime_macro_values: None,
                        runtime_macro_track: 0,
                    });
                }
            }
        }
        let mut scenes = self.pattern.scenes.lock().unwrap();
        let Some(pool) = scenes.track_pools.get_mut(track) else {
            return;
        };
        for data in pool.patterns.values_mut() {
            prepare_track_pattern_data_for_rack(data);
            match data.rack_track.as_mut() {
                Some(rack_track) => rack_track.slots.push(slot.clone()),
                None => {
                    data.rack_track = Some(RackTrackSnapshot {
                        routing,
                        slots: vec![slot.clone()],
                        macros: default_rack_macros(),
                        runtime_macro_values: None,
                        runtime_macro_track: 0,
                    });
                }
            }
        }
    }

    pub fn remove_rack_slot_from_all_pattern_snapshots(
        &self,
        track: usize,
        slot_idx: usize,
    ) -> bool {
        let mut removed = false;
        if let Some(Some(live_rack_track)) = self.pattern.rack_tracks.lock().unwrap().get_mut(track)
        {
            if slot_idx < live_rack_track.slots.len() {
                live_rack_track.slots.remove(slot_idx);
                remove_rack_macro_slot_targets(&mut live_rack_track.macros, slot_idx);
                removed = true;
            }
        }
        if !removed {
            return false;
        }

        let mut scenes = self.pattern.scenes.lock().unwrap();
        if let Some(pool) = scenes.track_pools.get_mut(track) {
            for data in pool.patterns.values_mut() {
                if let Some(rack_track) = data.rack_track.as_mut() {
                    if slot_idx < rack_track.slots.len() {
                        rack_track.slots.remove(slot_idx);
                        remove_rack_macro_slot_targets(&mut rack_track.macros, slot_idx);
                    }
                }
            }
        }
        true
    }

    pub fn remove_rack_slot_from_current_pattern(&self, track: usize, slot_idx: usize) -> bool {
        {
            let rack_tracks = self.pattern.rack_tracks.lock().unwrap();
            let Some(Some(live_rack_track)) = rack_tracks.get(track) else {
                return false;
            };
            if live_rack_track.slots.get(slot_idx).is_none() {
                return false;
            }
        }
        {
            let scenes = self.pattern.scenes.lock().unwrap();
            let Some(id) = scenes.effective_pattern_id(track) else {
                return false;
            };
            let Some(data) = scenes.track_pools.get(track).and_then(|pool| pool.get(id)) else {
                return false;
            };
            let Some(rack_track) = data.rack_track.as_ref() else {
                return false;
            };
            if rack_track.slots.get(slot_idx).is_none() {
                return false;
            }
        }

        if let Some(Some(live_rack_track)) = self.pattern.rack_tracks.lock().unwrap().get_mut(track)
        {
            live_rack_track.slots.remove(slot_idx);
            remove_rack_macro_slot_targets(&mut live_rack_track.macros, slot_idx);
        }

        let mut scenes = self.pattern.scenes.lock().unwrap();
        let id = scenes
            .effective_pattern_id(track)
            .expect("validated current pattern id before rack slot removal");
        let data = scenes
            .track_pools
            .get_mut(track)
            .and_then(|pool| pool.get_mut(id))
            .expect("validated current pattern data before rack slot removal");
        let rack_track = data
            .rack_track
            .as_mut()
            .expect("validated current rack track before rack slot removal");
        rack_track.slots.remove(slot_idx);
        remove_rack_macro_slot_targets(&mut rack_track.macros, slot_idx);
        true
    }

    pub fn replace_rack_slot_source_in_current_pattern(
        &self,
        track: usize,
        slot_idx: usize,
        replacement: RackSlotSnapshot,
    ) -> bool {
        {
            let rack_tracks = self.pattern.rack_tracks.lock().unwrap();
            let Some(Some(live_rack_track)) = rack_tracks.get(track) else {
                return false;
            };
            if live_rack_track.slots.get(slot_idx).is_none() {
                return false;
            }
        }
        {
            let scenes = self.pattern.scenes.lock().unwrap();
            let Some(id) = scenes.effective_pattern_id(track) else {
                return false;
            };
            let Some(data) = scenes.track_pools.get(track).and_then(|pool| pool.get(id)) else {
                return false;
            };
            let Some(rack_track) = data.rack_track.as_ref() else {
                return false;
            };
            if rack_track.slots.get(slot_idx).is_none() {
                return false;
            }
        }

        if let Some(Some(live_rack_track)) = self.pattern.rack_tracks.lock().unwrap().get_mut(track)
        {
            let slot = live_rack_track
                .slots
                .get_mut(slot_idx)
                .expect("validated live rack slot before source replacement");
            replace_rack_slot_source_preserving_controls(slot, &replacement);
        }

        let mut scenes = self.pattern.scenes.lock().unwrap();
        let id = scenes
            .effective_pattern_id(track)
            .expect("validated current pattern id before source replacement");
        let data = scenes
            .track_pools
            .get_mut(track)
            .and_then(|pool| pool.get_mut(id))
            .expect("validated current pattern data before source replacement");
        prepare_track_pattern_data_for_rack(data);
        let rack_track = data
            .rack_track
            .as_mut()
            .expect("validated current rack track before source replacement");
        let slot = rack_track
            .slots
            .get_mut(slot_idx)
            .expect("validated current rack slot before source replacement");
        replace_rack_slot_source_preserving_controls(slot, &replacement);
        true
    }

    pub fn replace_rack_slot_source_for_all_pattern_snapshots(
        &self,
        track: usize,
        slot_idx: usize,
        replacement: RackSlotSnapshot,
    ) -> bool {
        let mut replaced = false;
        if let Some(Some(live_rack_track)) = self.pattern.rack_tracks.lock().unwrap().get_mut(track)
        {
            if let Some(slot) = live_rack_track.slots.get_mut(slot_idx) {
                replace_rack_slot_source_preserving_controls(slot, &replacement);
                replaced = true;
            }
        }
        if !replaced {
            return false;
        }

        let mut scenes = self.pattern.scenes.lock().unwrap();
        if let Some(pool) = scenes.track_pools.get_mut(track) {
            for data in pool.patterns.values_mut() {
                if let Some(rack_track) = data.rack_track.as_mut() {
                    if let Some(slot) = rack_track.slots.get_mut(slot_idx) {
                        replace_rack_slot_source_preserving_controls(slot, &replacement);
                    }
                }
            }
        }
        true
    }

    pub fn sync_rack_slot_instrument_bindings_for_current_pattern(
        &self,
        track: usize,
        bindings: &[(EffectDescriptor, u32, u32)],
    ) -> bool {
        let sync_slots = |slots: &mut [RackSlotSnapshot]| {
            for (slot, (descriptor, node_id, modulator_node_id)) in
                slots.iter_mut().zip(bindings.iter())
            {
                slot.instrument_slot.sync_to_descriptor_with_modulator(
                    descriptor,
                    *node_id,
                    *modulator_node_id,
                );
            }
        };

        let mut synced_live = false;
        if let Some(Some(live_rack_track)) = self.pattern.rack_tracks.lock().unwrap().get_mut(track)
        {
            sync_slots(&mut live_rack_track.slots);
            synced_live = true;
        }

        let mut synced_current = false;
        let mut scenes = self.pattern.scenes.lock().unwrap();
        if let Some(id) = scenes.effective_pattern_id(track) {
            if let Some(data) = scenes
                .track_pools
                .get_mut(track)
                .and_then(|pool| pool.get_mut(id))
            {
                if let Some(rack_track) = data.rack_track.as_mut() {
                    sync_slots(&mut rack_track.slots);
                    synced_current = true;
                }
            }
        }
        synced_live && synced_current
    }

    pub fn sync_rack_slot_instrument_bindings_for_all_patterns(
        &self,
        track: usize,
        bindings: &[(EffectDescriptor, u32, u32)],
    ) -> bool {
        let sync_slots = |slots: &mut [RackSlotSnapshot]| {
            if slots.len() != bindings.len() {
                return false;
            }
            for (slot, (descriptor, node_id, modulator_node_id)) in
                slots.iter_mut().zip(bindings.iter())
            {
                slot.instrument_slot.sync_to_descriptor_with_modulator(
                    descriptor,
                    *node_id,
                    *modulator_node_id,
                );
            }
            true
        };
        let synced_live = self
            .pattern
            .rack_tracks
            .lock()
            .unwrap()
            .get_mut(track)
            .and_then(Option::as_mut)
            .is_some_and(|rack| sync_slots(&mut rack.slots));
        if !synced_live {
            return false;
        }
        let mut scenes = self.pattern.scenes.lock().unwrap();
        let Some(pool) = scenes.track_pools.get_mut(track) else {
            return false;
        };
        if pool.patterns.values().any(|data| {
            data.rack_track
                .as_ref()
                .is_none_or(|rack| rack.slots.len() != bindings.len())
        }) {
            return false;
        }
        for data in pool.patterns.values_mut() {
            sync_slots(
                &mut data
                    .rack_track
                    .as_mut()
                    .expect("rack topology was validated")
                    .slots,
            );
        }
        true
    }

    pub fn update_live_rack_slot<F>(&self, track: usize, slot_idx: usize, update: F) -> bool
    where
        F: FnOnce(&mut RackSlotSnapshot),
    {
        let mut rack_tracks = self.pattern.rack_tracks.lock().unwrap();
        let Some(Some(rack_track)) = rack_tracks.get_mut(track) else {
            return false;
        };
        let Some(slot) = rack_track.slots.get_mut(slot_idx) else {
            return false;
        };
        update(slot);
        true
    }

    pub fn update_rack_macro_in_current_pattern<F>(
        &self,
        track: usize,
        id: RackMacroId,
        update: F,
    ) -> bool
    where
        F: Fn(&mut RackMacro),
    {
        let index = id.index();
        {
            let racks = self.pattern.rack_tracks.lock().unwrap();
            if racks
                .get(track)
                .and_then(Option::as_ref)
                .and_then(|rack| rack.macros.get(index))
                .is_none()
            {
                return false;
            }
        }
        {
            let scenes = self.pattern.scenes.lock().unwrap();
            let Some(pattern_id) = scenes.effective_pattern_id(track) else {
                return false;
            };
            if scenes
                .track_pools
                .get(track)
                .and_then(|pool| pool.get(pattern_id))
                .and_then(|data| data.rack_track.as_ref())
                .and_then(|rack| rack.macros.get(index))
                .is_none()
            {
                return false;
            }
        }
        if let Some(rack_macro) = self
            .pattern
            .rack_tracks
            .lock()
            .unwrap()
            .get_mut(track)
            .and_then(Option::as_mut)
            .and_then(|rack| rack.macros.get_mut(index))
        {
            update(rack_macro);
        }
        let mut scenes = self.pattern.scenes.lock().unwrap();
        let pattern_id = scenes
            .effective_pattern_id(track)
            .expect("validated rack pattern");
        let rack_macro = scenes
            .track_pools
            .get_mut(track)
            .and_then(|pool| pool.get_mut(pattern_id))
            .and_then(|data| data.rack_track.as_mut())
            .and_then(|rack| rack.macros.get_mut(index))
            .expect("validated rack macro");
        update(rack_macro);
        true
    }

    /// Set one rack macro's parameter locks in both live and persisted views as
    /// a single transaction. Macro-knob drags commonly target many selected
    /// steps, so validating and locking the two rack snapshots once avoids
    /// repeating the full state transaction for every selected step.
    pub fn set_rack_macro_plocks_in_current_pattern(
        &self,
        track: usize,
        id: RackMacroId,
        steps: &[usize],
        value: f32,
    ) -> bool {
        let valid_steps = steps
            .iter()
            .copied()
            .filter(|step| *step < MAX_STEPS)
            .collect::<Vec<_>>();
        if valid_steps.is_empty() {
            return false;
        }
        let value = value.clamp(0.0, 1.0);
        let updated = self.update_rack_macro_in_current_pattern(track, id, |rack_macro| {
            for &step in &valid_steps {
                rack_macro.plocks[step] = Some(value);
            }
        });
        if updated {
            for step in valid_steps {
                self.rack_macro_runtime_values
                    .set_plock(track, id, step, Some(value));
            }
        }
        updated
    }

    pub fn set_live_rack_macro_default(&self, track: usize, id: RackMacroId, value: f32) {
        self.rack_macro_runtime_values.set_default(track, id, value);
    }

    pub(crate) fn rack_macro_runtime_values(&self) -> Arc<RackMacroRuntimeValues> {
        Arc::clone(&self.rack_macro_runtime_values)
    }

    pub(crate) fn sync_rack_macro_runtime_track(
        &self,
        track: usize,
        rack: Option<&RackTrackSnapshot>,
    ) {
        self.rack_macro_runtime_values.sync_track(track, rack);
    }

    pub fn update_rack_macros_for_all_pattern_snapshots<F>(&self, track: usize, update: F)
    where
        F: Fn(&mut Vec<RackMacro>),
    {
        if let Some(Some(rack)) = self.pattern.rack_tracks.lock().unwrap().get_mut(track) {
            update(&mut rack.macros);
        }
        if let Some(pool) = self
            .pattern
            .scenes
            .lock()
            .unwrap()
            .track_pools
            .get_mut(track)
        {
            for data in pool.patterns.values_mut() {
                if let Some(rack) = data.rack_track.as_mut() {
                    update(&mut rack.macros);
                }
            }
        }
    }

    /// Apply an edit to both the scheduler's live rack snapshot and the
    /// effective pattern that owns it. Validation happens before either copy
    /// is changed so a malformed pattern cannot leave the two views split.
    pub fn update_rack_slot_in_current_pattern<F>(
        &self,
        track: usize,
        slot_idx: usize,
        update: F,
    ) -> bool
    where
        F: Fn(&mut RackSlotSnapshot),
    {
        {
            let rack_tracks = self.pattern.rack_tracks.lock().unwrap();
            let Some(Some(rack)) = rack_tracks.get(track) else {
                return false;
            };
            if rack.slots.get(slot_idx).is_none() {
                return false;
            }
        }
        {
            let scenes = self.pattern.scenes.lock().unwrap();
            let Some(pattern_id) = scenes.effective_pattern_id(track) else {
                return false;
            };
            let Some(slot) = scenes
                .track_pools
                .get(track)
                .and_then(|pool| pool.get(pattern_id))
                .and_then(|data| data.rack_track.as_ref())
                .and_then(|rack| rack.slots.get(slot_idx))
            else {
                return false;
            };
            let _ = slot;
        }

        if let Some(Some(rack)) = self.pattern.rack_tracks.lock().unwrap().get_mut(track) {
            update(&mut rack.slots[slot_idx]);
        }
        let mut scenes = self.pattern.scenes.lock().unwrap();
        let pattern_id = scenes
            .effective_pattern_id(track)
            .expect("validated effective rack pattern before mutation");
        let slot = scenes.track_pools[track]
            .get_mut(pattern_id)
            .and_then(|data| data.rack_track.as_mut())
            .and_then(|rack| rack.slots.get_mut(slot_idx))
            .expect("validated current rack slot before mutation");
        update(slot);
        true
    }

    /// Apply a structural rack-slot edit to the live scheduler snapshot and
    /// every stored pattern for the track. Rack instruments and their effect
    /// nodes are graph-owned device identity; only parameter values and locks
    /// vary per pattern. Adding, removing, or moving a device therefore must
    /// not leave another scene pointing at an empty or stale graph node.
    pub fn update_rack_slot_in_all_pattern_snapshots<F>(
        &self,
        track: usize,
        slot_idx: usize,
        update: F,
    ) -> bool
    where
        F: Fn(&mut RackSlotSnapshot),
    {
        {
            let rack_tracks = self.pattern.rack_tracks.lock().unwrap();
            let Some(Some(live_rack)) = rack_tracks.get(track) else {
                return false;
            };
            if live_rack.slots.get(slot_idx).is_none() {
                return false;
            }
        }
        {
            let scenes = self.pattern.scenes.lock().unwrap();
            let Some(pool) = scenes.track_pools.get(track) else {
                return false;
            };
            if pool.patterns.values().any(|data| {
                data.rack_track
                    .as_ref()
                    .and_then(|rack| rack.slots.get(slot_idx))
                    .is_none()
            }) {
                return false;
            }
        }

        let mut rack_tracks = self.pattern.rack_tracks.lock().unwrap();
        let live_slot = rack_tracks[track]
            .as_mut()
            .and_then(|rack| rack.slots.get_mut(slot_idx))
            .expect("validated live rack slot before structural mutation");
        update(live_slot);
        drop(rack_tracks);

        let mut scenes = self.pattern.scenes.lock().unwrap();
        let pool = scenes
            .track_pools
            .get_mut(track)
            .expect("validated rack pattern pool before structural mutation");
        for data in pool.patterns.values_mut() {
            let slot = data
                .rack_track
                .as_mut()
                .and_then(|rack| rack.slots.get_mut(slot_idx))
                .expect("validated stored rack slot before structural mutation");
            update(slot);
        }
        true
    }

    pub fn capture_rack_slot_pattern_state(
        &self,
        track: usize,
        slot_index: usize,
    ) -> Result<RackSlotPatternStateSnapshot, String> {
        let live = self
            .pattern
            .rack_tracks
            .lock()
            .unwrap()
            .get(track)
            .and_then(Option::as_ref)
            .and_then(|rack| rack.slots.get(slot_index))
            .cloned()
            .ok_or_else(|| format!("Track {} rack slot {} is missing", track + 1, slot_index + 1))?;
        let scenes = self.pattern.scenes.lock().unwrap();
        let pool = scenes
            .track_pools
            .get(track)
            .ok_or_else(|| format!("Track {} has no pattern pool", track + 1))?;
        let mut patterns = Vec::with_capacity(pool.patterns.len());
        for (pattern_id, data) in &pool.patterns {
            let slot = data
                .rack_track
                .as_ref()
                .and_then(|rack| rack.slots.get(slot_index))
                .cloned()
                .ok_or_else(|| {
                    format!(
                        "Track {} pattern {:?} has no rack slot {}",
                        track + 1,
                        pattern_id,
                        slot_index + 1
                    )
                })?;
            patterns.push((*pattern_id, slot));
        }
        Ok(RackSlotPatternStateSnapshot {
            slot_index,
            live,
            patterns,
        })
    }

    pub fn capture_rack_macro_pattern_state(
        &self,
        track: usize,
    ) -> Result<RackMacroPatternStateSnapshot, String> {
        let live = self.pattern.rack_tracks.lock().unwrap()
            .get(track)
            .and_then(Option::as_ref)
            .map(|rack| rack.macros.clone())
            .ok_or_else(|| format!("Track {} has no live rack", track + 1))?;
        let scenes = self.pattern.scenes.lock().unwrap();
        let pool = scenes.track_pools.get(track)
            .ok_or_else(|| format!("Track {} has no pattern pool", track + 1))?;
        let patterns = pool.patterns.iter().map(|(pattern, data)| {
            data.rack_track.as_ref()
                .map(|rack| (*pattern, rack.macros.clone()))
                .ok_or_else(|| format!("Track {} pattern {:?} has no rack", track + 1, pattern))
        }).collect::<Result<Vec<_>, String>>()?;
        Ok(RackMacroPatternStateSnapshot { live, patterns })
    }

    pub fn restore_rack_macro_pattern_state(
        &self,
        track: usize,
        snapshot: &RackMacroPatternStateSnapshot,
    ) -> Result<(), String> {
        let mut scenes = self.pattern.scenes.lock().unwrap();
        let pool = scenes.track_pools.get_mut(track)
            .ok_or_else(|| format!("Track {} has no pattern pool", track + 1))?;
        if pool.patterns.len() != snapshot.patterns.len()
            || snapshot.patterns.iter().any(|(pattern, _)| !pool.patterns.contains_key(pattern))
        {
            return Err(format!("Track {} rack macro pattern topology changed", track + 1));
        }
        for (pattern, macros) in &snapshot.patterns {
            let rack = pool.patterns.get_mut(pattern)
                .and_then(|data| data.rack_track.as_mut())
                .ok_or_else(|| format!("Track {} pattern {:?} has no rack", track + 1, pattern))?;
            rack.macros.clone_from(macros);
        }
        drop(scenes);
        let mut racks = self.pattern.rack_tracks.lock().unwrap();
        let rack = racks.get_mut(track).and_then(Option::as_mut)
            .ok_or_else(|| format!("Track {} has no live rack", track + 1))?;
        rack.macros.clone_from(&snapshot.live);
        self.sync_rack_macro_runtime_track(track, Some(rack));
        Ok(())
    }

    pub fn restore_rack_slot_effect_pattern_state(
        &self,
        track: usize,
        snapshot: &RackSlotPatternStateSnapshot,
    ) -> Result<(), String> {
        self.validate_rack_slot_pattern_state(track, snapshot)?;
        let copy_effects = |slot: &mut RackSlotSnapshot, saved: &RackSlotSnapshot| {
            slot.effect_slots.clone_from(&saved.effect_slots);
            slot.effect_descriptors.clone_from(&saved.effect_descriptors);
            slot.custom_effect_names.clone_from(&saved.custom_effect_names);
        };
        let mut scenes = self.pattern.scenes.lock().unwrap();
        let pool = scenes.track_pools.get_mut(track)
            .ok_or_else(|| format!("Track {} has no pattern pool", track + 1))?;
        for (pattern, saved) in &snapshot.patterns {
            let slot = pool.patterns.get_mut(pattern)
                .and_then(|data| data.rack_track.as_mut())
                .and_then(|rack| rack.slots.get_mut(snapshot.slot_index))
                .ok_or_else(|| format!("Track {} pattern {:?} lost rack slot {}", track + 1, pattern, snapshot.slot_index + 1))?;
            copy_effects(slot, saved);
        }
        drop(scenes);
        let mut racks = self.pattern.rack_tracks.lock().unwrap();
        let slot = racks.get_mut(track)
            .and_then(Option::as_mut)
            .and_then(|rack| rack.slots.get_mut(snapshot.slot_index))
            .ok_or_else(|| format!("Track {} lost rack slot {}", track + 1, snapshot.slot_index + 1))?;
        copy_effects(slot, &snapshot.live);
        Ok(())
    }

    pub fn restore_rack_slot_pattern_state(
        &self,
        track: usize,
        snapshot: &RackSlotPatternStateSnapshot,
        descriptor: &EffectDescriptor,
        node_id: u32,
        modulator_node_id: u32,
    ) -> Result<(), String> {
        let sync_slot = |slot: &mut RackSlotSnapshot, saved: &RackSlotSnapshot| {
            *slot = saved.clone();
            slot.instrument_slot.sync_to_descriptor_with_modulator(
                descriptor,
                node_id,
                modulator_node_id,
            );
        };
        self.validate_rack_slot_pattern_state(track, snapshot)?;
        let mut scenes = self.pattern.scenes.lock().unwrap();
        let pool = scenes
            .track_pools
            .get_mut(track)
            .ok_or_else(|| format!("Track {} has no pattern pool", track + 1))?;
        if pool.patterns.len() != snapshot.patterns.len()
            || snapshot.patterns.iter().any(|(id, _)| {
                pool.patterns
                    .get(id)
                    .and_then(|data| data.rack_track.as_ref())
                    .and_then(|rack| rack.slots.get(snapshot.slot_index))
                    .is_none()
            })
        {
            return Err(format!(
                "Track {} rack pattern topology changed before history replay",
                track + 1
            ));
        }
        for (pattern_id, saved) in &snapshot.patterns {
            let slot = pool
                .patterns
                .get_mut(pattern_id)
                .and_then(|data| data.rack_track.as_mut())
                .and_then(|rack| rack.slots.get_mut(snapshot.slot_index))
                .expect("rack pattern topology was validated");
            sync_slot(slot, saved);
        }
        drop(scenes);
        let mut racks = self.pattern.rack_tracks.lock().unwrap();
        let slot = racks[track]
            .as_mut()
            .and_then(|rack| rack.slots.get_mut(snapshot.slot_index))
            .expect("live rack topology was validated");
        sync_slot(slot, &snapshot.live);
        Ok(())
    }

    pub fn validate_rack_slot_pattern_state(
        &self,
        track: usize,
        snapshot: &RackSlotPatternStateSnapshot,
    ) -> Result<(), String> {
        let racks = self.pattern.rack_tracks.lock().unwrap();
        if racks
            .get(track)
            .and_then(Option::as_ref)
            .and_then(|rack| rack.slots.get(snapshot.slot_index))
            .is_none()
        {
            return Err(format!(
                "Track {} rack slot {} disappeared before history replay",
                track + 1,
                snapshot.slot_index + 1
            ));
        }
        drop(racks);
        let scenes = self.pattern.scenes.lock().unwrap();
        let pool = scenes
            .track_pools
            .get(track)
            .ok_or_else(|| format!("Track {} has no pattern pool", track + 1))?;
        if pool.patterns.len() != snapshot.patterns.len()
            || snapshot.patterns.iter().any(|(id, _)| {
                pool.patterns
                    .get(id)
                    .and_then(|data| data.rack_track.as_ref())
                    .and_then(|rack| rack.slots.get(snapshot.slot_index))
                    .is_none()
            })
        {
            return Err(format!(
                "Track {} rack pattern topology changed before history replay",
                track + 1
            ));
        }
        Ok(())
    }

    pub fn validate_rack_slot_append_pattern_state(
        &self,
        track: usize,
        snapshot: &RackSlotPatternStateSnapshot,
    ) -> Result<(), String> {
        let racks = self.pattern.rack_tracks.lock().unwrap();
        let live_len = racks
            .get(track)
            .and_then(Option::as_ref)
            .map(|rack| rack.slots.len())
            .ok_or_else(|| format!("Track {} has no live rack", track + 1))?;
        if live_len != snapshot.slot_index {
            return Err(format!(
                "Track {} rack has {live_len} slots; history expected {} before append",
                track + 1,
                snapshot.slot_index
            ));
        }
        drop(racks);
        let scenes = self.pattern.scenes.lock().unwrap();
        let pool = scenes
            .track_pools
            .get(track)
            .ok_or_else(|| format!("Track {} has no pattern pool", track + 1))?;
        if pool.patterns.len() != snapshot.patterns.len()
            || snapshot.patterns.iter().any(|(id, _)| {
                pool.patterns
                    .get(id)
                    .and_then(|data| data.rack_track.as_ref())
                    .is_none_or(|rack| rack.slots.len() != snapshot.slot_index)
            })
        {
            return Err(format!(
                "Track {} rack pattern topology changed before slot append replay",
                track + 1
            ));
        }
        Ok(())
    }

    pub fn insert_effect_slot_in_other_track_patterns(&self, track: usize, slot_idx: usize) {
        let mut scenes = self.pattern.scenes.lock().unwrap();
        scenes.edit_other_track_patterns(track, |data| data.insert_empty_effect_slot(slot_idx));
    }

    pub fn move_effect_slot_in_other_track_patterns(
        &self,
        track: usize,
        source_slot: usize,
        target_slot: usize,
    ) {
        let mut scenes = self.pattern.scenes.lock().unwrap();
        scenes.edit_other_track_patterns(track, |data| {
            data.move_effect_slot_to(source_slot, target_slot);
        });
    }

    pub fn remove_effect_slot_from_track_patterns(&self, track: usize, slot_idx: usize) {
        self.save_current_track_effect_snapshot(track);
        let mut scenes = self.pattern.scenes.lock().unwrap();
        scenes.edit_other_track_patterns(track, |data| data.remove_effect_slot(slot_idx));
    }

    pub fn insert_midi_fx_slot_in_other_track_patterns(
        &self,
        track: usize,
        slot_idx: usize,
        name: String,
        descriptor: &EffectDescriptor,
    ) {
        let mut scenes = self.pattern.scenes.lock().unwrap();
        scenes.edit_other_track_patterns(track, |data| {
            data.insert_midi_fx_slot(slot_idx, name.clone(), descriptor);
        });
    }

    pub fn replace_midi_fx_slot_in_all_track_patterns(
        &self,
        track: usize,
        slot_idx: usize,
        name: String,
        descriptor: &EffectDescriptor,
    ) -> Result<(), String> {
        if track >= self.pattern.track_params.len()
            || slot_idx >= self.pattern.midi_fx_slots[track].len()
        {
            return Err("MIDI-FX replacement target is out of range".to_string());
        }
        let mut live_chain = self.pattern.track_params[track].midi_fx_chain();
        if slot_idx >= live_chain.len() {
            return Err("MIDI-FX replacement target is empty".to_string());
        }
        let mut scenes = self.pattern.scenes.lock().unwrap();
        let pool = scenes
            .track_pools
            .get_mut(track)
            .ok_or_else(|| format!("Track {} has no pattern pool", track + 1))?;
        if pool.patterns.values().any(|data| {
            slot_idx >= data.track_params.midi_fx_chain.len()
                || slot_idx >= data.midi_fx_slots.len()
        }) {
            return Err("stored MIDI-FX replacement target is missing".to_string());
        }
        live_chain[slot_idx] = name.clone();
        self.pattern.track_params[track].set_midi_fx_chain(live_chain);
        self.pattern.midi_fx_slots[track][slot_idx].apply_descriptor(descriptor, 0);
        for data in pool.patterns.values_mut() {
            data.track_params.midi_fx_chain[slot_idx] = name.clone();
            data.midi_fx_slots[slot_idx].sync_to_descriptor(descriptor, 0);
        }
        Ok(())
    }

    pub fn move_midi_fx_slot_in_other_track_patterns(
        &self,
        track: usize,
        source_slot: usize,
        target_slot: usize,
    ) {
        let mut scenes = self.pattern.scenes.lock().unwrap();
        scenes.edit_other_track_patterns(track, |data| {
            data.move_midi_fx_slot_to(source_slot, target_slot);
        });
    }

    pub fn remove_midi_fx_slot_from_track_patterns(&self, track: usize, slot_idx: usize) {
        self.save_current_track_midi_fx_snapshot(track);
        let mut scenes = self.pattern.scenes.lock().unwrap();
        scenes.edit_other_track_patterns(track, |data| data.remove_midi_fx_slot(slot_idx));
    }

    pub fn remove_bus_references_from_all_track_patterns(&self, bus_id: BusId) {
        let mut scenes = self.pattern.scenes.lock().unwrap();
        for pool in &mut scenes.track_pools {
            for data in pool.patterns.values_mut() {
                if data.track_params.output == TrackOutput::Bus(bus_id) {
                    data.track_params.output = TrackOutput::Mix;
                }
                data.track_params
                    .sends
                    .retain(|send| send.destination != bus_id);
            }
        }
    }

    /// Force one track's output across every stored scene. Track output is
    /// otherwise per-scene, but a track group is a global concept — its members
    /// must reach the backing bus in every scene, or switching scenes would tear
    /// the group's routing apart (and a saved project would silently lose it).
    pub fn set_track_output_in_all_track_patterns(
        &self,
        track: usize,
        output: TrackOutput,
    ) -> bool {
        let mut scenes = self.pattern.scenes.lock().unwrap();
        let mut changed = false;
        if let Some(pool) = scenes.track_pools.get_mut(track) {
            for data in pool.patterns.values_mut() {
                if data.track_params.output != output {
                    data.track_params.output = output.clone();
                    changed = true;
                }
            }
        }
        changed
    }

    fn ensure_scene_bus_patterns_len_locked(
        scenes: &mut ProjectScenes,
        len: usize,
        default_snapshot: &[BusPatternSnapshot],
    ) {
        for scene in scenes.scenes.iter_mut().take(len) {
            if scene.bus_patterns.is_empty() {
                scene.bus_patterns = default_snapshot.to_vec();
            }
        }
    }

    pub fn save_current_bus_pattern_snapshot(&self, snapshot: Vec<BusPatternSnapshot>) {
        let current_scene = self.current_scene_index();
        let target_len = self.scene_count().max(current_scene + 1);
        let mut scenes = self.pattern.scenes.lock().unwrap();
        Self::ensure_scene_bus_patterns_len_locked(&mut scenes, target_len, &snapshot);
        scenes.scenes[current_scene].bus_patterns = snapshot;
    }

    pub fn bus_pattern_snapshot_or_default(
        &self,
        scene_idx: usize,
        default_snapshot: &[BusPatternSnapshot],
    ) -> Vec<BusPatternSnapshot> {
        let mut scenes = self.pattern.scenes.lock().unwrap();
        Self::ensure_scene_bus_patterns_len_locked(&mut scenes, scene_idx + 1, default_snapshot);
        scenes
            .scenes
            .get(scene_idx)
            .map(|scene| scene.bus_patterns.clone())
            .unwrap_or_else(|| default_snapshot.to_vec())
    }

    pub fn ensure_bus_pattern_repository_len(
        &self,
        len: usize,
        default_snapshot: &[BusPatternSnapshot],
    ) {
        let mut scenes = self.pattern.scenes.lock().unwrap();
        Self::ensure_scene_bus_patterns_len_locked(&mut scenes, len, default_snapshot);
    }

    pub fn clone_bus_pattern_snapshot(
        &self,
        source_idx: usize,
        new_idx: usize,
        default_snapshot: &[BusPatternSnapshot],
    ) -> Vec<BusPatternSnapshot> {
        let mut scenes = self.pattern.scenes.lock().unwrap();
        Self::ensure_scene_bus_patterns_len_locked(
            &mut scenes,
            source_idx.max(new_idx) + 1,
            default_snapshot,
        );
        let source = scenes
            .scenes
            .get(source_idx)
            .map(|scene| scene.bus_patterns.clone())
            .unwrap_or_else(|| default_snapshot.to_vec());
        if let Some(scene) = scenes.scenes.get_mut(new_idx) {
            scene.bus_patterns = source.clone();
        }
        source
    }

    pub fn delete_bus_pattern_snapshot(
        &self,
        _deleted_idx: usize,
        new_idx: usize,
        default_snapshot: &[BusPatternSnapshot],
    ) -> Vec<BusPatternSnapshot> {
        let mut scenes = self.pattern.scenes.lock().unwrap();
        Self::ensure_scene_bus_patterns_len_locked(&mut scenes, new_idx + 1, default_snapshot);
        scenes
            .scenes
            .get(new_idx)
            .map(|scene| scene.bus_patterns.clone())
            .unwrap_or_else(|| default_snapshot.to_vec())
    }

    pub fn export_bus_pattern_repository(
        &self,
        default_snapshot: &[BusPatternSnapshot],
    ) -> Vec<Vec<BusPatternSnapshot>> {
        let target_len = self.scene_count();
        let mut scenes = self.pattern.scenes.lock().unwrap();
        Self::ensure_scene_bus_patterns_len_locked(&mut scenes, target_len, default_snapshot);
        scenes
            .scenes
            .iter()
            .map(|scene| scene.bus_patterns.clone())
            .collect()
    }

    pub fn replace_bus_pattern_repository(
        &self,
        snapshots: Vec<Vec<BusPatternSnapshot>>,
        default_snapshot: &[BusPatternSnapshot],
    ) {
        let mut scenes = self.pattern.scenes.lock().unwrap();
        let target_len = scenes.scene_count().max(snapshots.len());
        Self::ensure_scene_bus_patterns_len_locked(&mut scenes, target_len, default_snapshot);
        for (scene, snapshot) in scenes.scenes.iter_mut().zip(snapshots) {
            scene.bus_patterns = snapshot;
        }
    }

    pub fn insert_bus_effect_slot_in_other_scene_patterns(
        &self,
        bus_idx: usize,
        slot_idx: usize,
        default_snapshot: &[BusPatternSnapshot],
    ) {
        let slot_count = crate::lisp_host::MAX_CUSTOM_FX;
        if slot_idx >= slot_count {
            return;
        }
        let mut new_to_old = (0..slot_count).map(Some).collect::<Vec<_>>();
        new_to_old.insert(slot_idx, None);
        new_to_old.truncate(slot_count);
        self.remap_bus_effect_slots_in_other_scene_patterns(bus_idx, &new_to_old, default_snapshot);
    }

    pub fn move_bus_effect_slot_in_other_scene_patterns(
        &self,
        bus_idx: usize,
        source_slot: usize,
        target_slot: usize,
        default_snapshot: &[BusPatternSnapshot],
    ) {
        let slot_count = crate::lisp_host::MAX_CUSTOM_FX;
        if source_slot >= slot_count || target_slot >= slot_count || source_slot == target_slot {
            return;
        }
        let mut new_to_old = (0..slot_count).map(Some).collect::<Vec<_>>();
        let source = new_to_old.remove(source_slot);
        new_to_old.insert(target_slot, source);
        self.remap_bus_effect_slots_in_other_scene_patterns(bus_idx, &new_to_old, default_snapshot);
    }

    pub fn remap_bus_effect_slots_in_other_scene_patterns(
        &self,
        bus_idx: usize,
        new_to_old: &[Option<usize>],
        default_snapshot: &[BusPatternSnapshot],
    ) {
        let current_scene = self.current_scene_index();
        let mut scenes = self.pattern.scenes.lock().unwrap();
        let target_len = scenes.scene_count().max(current_scene + 1);
        Self::ensure_scene_bus_patterns_len_locked(&mut scenes, target_len, default_snapshot);
        for (scene_idx, scene) in scenes.scenes.iter_mut().enumerate() {
            if scene_idx == current_scene {
                continue;
            }
            if let Some(bus) = scene.bus_patterns.get_mut(bus_idx) {
                bus.remap_effect_slots(new_to_old);
            }
        }
    }

    pub fn replace_bus_effect_slot_in_other_scene_patterns(
        &self,
        bus_idx: usize,
        slot_idx: usize,
        source_snapshot: &[BusPatternSnapshot],
    ) {
        let Some(source_bus) = source_snapshot.get(bus_idx) else {
            return;
        };
        let defaults = source_bus
            .effect_defaults
            .get(slot_idx)
            .cloned()
            .unwrap_or_default();
        let plocks = source_bus
            .effect_plocks
            .get(slot_idx)
            .cloned()
            .unwrap_or_default();
        let current_scene = self.current_scene_index();
        let mut scenes = self.pattern.scenes.lock().unwrap();
        let target_len = scenes.scene_count().max(current_scene + 1);
        Self::ensure_scene_bus_patterns_len_locked(&mut scenes, target_len, source_snapshot);
        for (scene_idx, scene) in scenes.scenes.iter_mut().enumerate() {
            if scene_idx == current_scene {
                continue;
            }
            if let Some(bus) = scene.bus_patterns.get_mut(bus_idx) {
                bus.replace_effect_slot(slot_idx, defaults.clone(), plocks.clone());
            }
        }
    }

    pub fn copy_bus_effect_values_to_all_scene_patterns(
        &self,
        bus_idx: usize,
        slot_idx: usize,
        source_snapshot: &[BusPatternSnapshot],
    ) -> usize {
        let Some(source_defaults) = source_snapshot
            .get(bus_idx)
            .and_then(|bus| bus.effect_defaults.get(slot_idx))
            .cloned()
        else {
            return 0;
        };
        let mut scenes = self.pattern.scenes.lock().unwrap();
        let target_len = scenes.scene_count().max(self.current_scene_index() + 1);
        Self::ensure_scene_bus_patterns_len_locked(&mut scenes, target_len, source_snapshot);
        let mut updated = 0;
        for scene in &mut scenes.scenes {
            let Some(bus) = scene.bus_patterns.get_mut(bus_idx) else {
                continue;
            };
            bus.effect_defaults.resize_with(slot_idx + 1, Vec::new);
            bus.effect_defaults[slot_idx] = source_defaults.clone();
            updated += 1;
        }
        updated
    }

    pub fn clear_bus_effect_slot_in_other_scene_patterns(
        &self,
        bus_idx: usize,
        slot_idx: usize,
        default_snapshot: &[BusPatternSnapshot],
    ) {
        let current_scene = self.current_scene_index();
        let mut scenes = self.pattern.scenes.lock().unwrap();
        let target_len = scenes.scene_count().max(current_scene + 1);
        Self::ensure_scene_bus_patterns_len_locked(&mut scenes, target_len, default_snapshot);
        for (scene_idx, scene) in scenes.scenes.iter_mut().enumerate() {
            if scene_idx == current_scene {
                continue;
            }
            if let Some(bus) = scene.bus_patterns.get_mut(bus_idx) {
                bus.replace_effect_slot(slot_idx, Vec::new(), Vec::new());
            }
        }
    }

    pub fn edit_pattern_repository<F, R>(&self, edit: F) -> R
    where
        F: FnOnce(&mut Vec<PatternSnapshot>, usize) -> R,
    {
        let current_pattern = self.current_pattern_index();
        let mut scenes = self.pattern.scenes.lock().unwrap();
        let bus_patterns = scenes
            .scenes
            .iter()
            .map(|scene| scene.bus_patterns.clone())
            .collect::<Vec<_>>();
        let mut snapshots = scenes.snapshots();
        let result = edit(&mut snapshots, current_pattern);
        let mut rebuilt = ProjectScenes::from_pattern_snapshots(&snapshots, current_pattern);
        for (scene, bus_patterns) in rebuilt.scenes.iter_mut().zip(bus_patterns) {
            scene.bus_patterns = bus_patterns;
        }
        *scenes = rebuilt;
        result
    }

    pub fn edit_non_current_pattern_snapshots<F>(&self, mut edit: F)
    where
        F: FnMut(&mut PatternSnapshot),
    {
        self.edit_pattern_repository(|bank, current_pattern| {
            for (pattern_idx, snapshot) in bank.iter_mut().enumerate() {
                if pattern_idx != current_pattern {
                    edit(snapshot);
                }
            }
        });
    }

    pub fn edit_all_pattern_snapshots<F>(&self, mut edit: F)
    where
        F: FnMut(&mut PatternSnapshot),
    {
        self.edit_pattern_repository(|bank, _| {
            for snapshot in bank.iter_mut() {
                edit(snapshot);
            }
        });
    }

    pub fn current_scene_metadata(
        &self,
    ) -> (
        Vec<ModConnection>,
        Vec<ProjectNeuralNetwork>,
        Vec<ProjectGraphOverrides>,
    ) {
        let current_pattern = self.current_pattern_index();
        self.pattern
            .scenes
            .lock()
            .unwrap()
            .scene_metadata(current_pattern)
            .unwrap_or_default()
    }

    pub fn current_mod_connections(&self) -> Vec<ModConnection> {
        self.current_scene_metadata().0
    }

    pub fn edit_current_mod_connections<F, R>(&self, edit: F) -> Result<R, String>
    where
        F: FnOnce(&mut Vec<ModConnection>) -> Result<R, String>,
    {
        let current_pattern = self.current_pattern_index();
        let result = {
            let mut bank = self
                .pattern
                .scenes
                .lock()
                .map_err(|_| "failed to lock pattern bank".to_string())?;
            if bank.current_scene != current_pattern {
                bank.current_scene = current_pattern.min(bank.scene_count().saturating_sub(1));
            }
            bank.edit_current_mod_connections(edit)?
        };
        self.publish_scheduler_snapshot();
        Ok(result)
    }
    pub fn latest_scheduler_snapshot(&self) -> Arc<SequencerSnapshot> {
        self.scheduler_snapshot.lock().unwrap().clone()
    }

    pub fn set_neural_visualization(&self, snapshot: NeuralVisualizationSnapshot) {
        *self.neural_visualization.lock().unwrap() = snapshot;
    }

    pub fn neural_visualization(&self) -> NeuralVisualizationSnapshot {
        self.neural_visualization.lock().unwrap().clone()
    }

    pub fn set_graph_visualizations(&self, snapshots: Vec<GraphVisualizationSnapshot>) {
        *self.graph_visualizations.lock().unwrap() = snapshots;
    }

    pub fn graph_visualizations(&self) -> Vec<GraphVisualizationSnapshot> {
        self.graph_visualizations.lock().unwrap().clone()
    }

    pub fn append_track_output_events(&self, events: impl IntoIterator<Item = TrackOutputEvent>) {
        let mut history = self.track_output_events.lock().unwrap();
        history.extend(events);
        let overflow = history.len().saturating_sub(TRACK_OUTPUT_EVENT_HISTORY_CAP);
        if overflow > 0 {
            history.drain(0..overflow);
        }
    }

    pub fn clear_track_output_events(&self) {
        self.track_output_events.lock().unwrap().clear();
    }

    pub fn track_output_events(&self) -> Vec<TrackOutputEvent> {
        self.track_output_events.lock().unwrap().clone()
    }

    pub fn set_track_output_current_beat(&self, beat: f64) {
        self.track_output_current_beat_bits
            .store(beat.max(0.0).to_bits(), Ordering::Relaxed);
    }

    pub fn track_output_current_beat(&self) -> f64 {
        f64::from_bits(self.track_output_current_beat_bits.load(Ordering::Relaxed))
    }

    /// Publish the audio clock used to expire scheduled-note activity without
    /// taking a lock on the realtime thread.
    pub fn set_audio_rendered_sample(&self, sample: u64) {
        self.audio_rendered_sample.store(sample, Ordering::Release);
    }

    /// Keep a scheduled MIDI note active through its gate end. `fetch_max`
    /// preserves overlapping/retriggered instances of the same pitch.
    pub fn mark_scheduled_note_active_until(&self, track: usize, note: u8, sample: u64) {
        if let Some(notes) = self.active_note_until_samples.get(track) {
            notes[note as usize].fetch_max(sample, Ordering::Relaxed);
        }
    }

    /// Live notes have explicit note-off events, so replace their compact mask
    /// independently from scheduled expirations. The two sources can overlap.
    pub fn replace_live_notes(&self, track: usize, notes: impl IntoIterator<Item = u8>) {
        let Some(words) = self.live_note_masks.get(track) else {
            return;
        };
        let mut next = [0_u64; 2];
        for note in notes {
            next[note as usize / 64] |= 1_u64 << (note as usize % 64);
        }
        for (word, value) in words.iter().zip(next) {
            word.store(value, Ordering::Release);
        }
    }

    pub fn active_notes(&self, track: usize) -> Vec<u8> {
        let (Some(until), Some(live)) = (
            self.active_note_until_samples.get(track),
            self.live_note_masks.get(track),
        ) else {
            return Vec::new();
        };
        let rendered = self.audio_rendered_sample.load(Ordering::Acquire);
        (0_u8..=127)
            .filter(|note| {
                let idx = *note as usize;
                let bit = 1_u64 << (idx % 64);
                live[idx / 64].load(Ordering::Relaxed) & bit != 0
                    || until[idx].load(Ordering::Relaxed) > rendered
            })
            .collect()
    }

    pub fn publish_scheduler_snapshot(&self) -> Arc<SequencerSnapshot> {
        let snapshot = Arc::new(SequencerSnapshot::capture(self));
        self.publish_scheduler_snapshot_arc(snapshot)
    }

    /// Publish one complete track through a copy-on-write scheduler snapshot.
    /// Unchanged tracks keep their existing `Arc`, while the edited track is
    /// recaptured with its step payloads, device p-locks, and process state.
    pub fn publish_scheduler_track(&self, track: usize) -> Arc<SequencerSnapshot> {
        let current = self.scheduler_snapshot.lock().unwrap().clone();
        if track >= self.active_track_count()
            || track >= current.tracks.len()
            || current.tracks.len() != self.active_track_count()
        {
            return self.publish_scheduler_snapshot();
        }
        let mut next = (*current).clone();
        let Some(next_track) = SequencerSnapshot::capture_live_track(self, track) else {
            return self.publish_scheduler_snapshot();
        };
        next.tracks[track] = Arc::new(next_track);
        next.transport = SequencerTransportSnapshot {
            bpm: self.transport.bpm.load(Ordering::Relaxed),
            playing: self.transport.playing.load(Ordering::Relaxed),
            current_pattern: self.current_scene_index(),
            pattern_epoch: self.transport.pattern_epoch.load(Ordering::Relaxed),
            topology_epoch: self.transport.topology_epoch.load(Ordering::Relaxed),
            num_tracks: self.active_track_count(),
        };
        self.publish_scheduler_snapshot_arc(Arc::new(next))
    }

    /// Replaces the command-thread macro layer and immediately publishes a
    /// scheduler snapshot containing those effective defaults.
    pub fn publish_macro_overrides(
        &self,
        overrides: HashMap<crate::macro_engine::MacroParamKey, f32>,
    ) -> Arc<SequencerSnapshot> {
        *self.live_macro_overrides.lock().unwrap() = overrides;
        self.publish_scheduler_snapshot()
    }

    pub(super) fn live_macro_overrides(&self) -> HashMap<crate::macro_engine::MacroParamKey, f32> {
        self.live_macro_overrides.lock().unwrap().clone()
    }

    fn publish_scheduler_snapshot_arc(
        &self,
        snapshot: Arc<SequencerSnapshot>,
    ) -> Arc<SequencerSnapshot> {
        {
            let mut published = self.scheduler_snapshot.lock().unwrap();
            *published = Arc::clone(&snapshot);
        }
        self.scheduler_snapshot_version
            .fetch_add(1, Ordering::AcqRel);
        snapshot
    }

    #[allow(clippy::too_many_arguments)]
    fn publish_scheduler_snapshot_from_track_pattern_data(
        &self,
        tracks: &[TrackPatternData],
        mod_connections: Vec<ModConnection>,
        neural_networks: Vec<ProjectNeuralNetwork>,
        graph_overrides: Vec<ProjectGraphOverrides>,
        project_process_chain: crate::process::TrackProcessChain,
    ) -> Arc<SequencerSnapshot> {
        self.publish_scheduler_snapshot_arc(Arc::new(
            SequencerSnapshot::capture_from_track_pattern_data(
                self,
                tracks,
                mod_connections,
                neural_networks,
                graph_overrides,
                project_process_chain,
            ),
        ))
    }

    pub fn current_neural_networks(&self) -> Vec<ProjectNeuralNetwork> {
        self.pattern
            .scenes
            .lock()
            .unwrap()
            .current_neural_networks()
    }

    pub fn edit_current_neural_networks<F, R>(&self, edit: F) -> Result<R, String>
    where
        F: FnOnce(&mut Vec<ProjectNeuralNetwork>) -> Result<R, String>,
    {
        let result = {
            let mut bank = self
                .pattern
                .scenes
                .lock()
                .map_err(|_| "failed to lock pattern bank".to_string())?;
            bank.current_scene = self
                .current_scene_index()
                .min(bank.scene_count().saturating_sub(1));
            bank.edit_current_neural_networks(edit)?
        };
        self.publish_scheduler_snapshot();
        Ok(result)
    }

    pub fn current_graph_overrides(&self) -> Vec<ProjectGraphOverrides> {
        self.pattern
            .scenes
            .lock()
            .unwrap()
            .current_graph_overrides()
    }

    pub fn edit_current_graph_overrides<F, R>(&self, edit: F) -> Result<R, String>
    where
        F: FnOnce(&mut Vec<ProjectGraphOverrides>) -> Result<R, String>,
    {
        let result = {
            let mut bank = self
                .pattern
                .scenes
                .lock()
                .map_err(|_| "failed to lock pattern bank".to_string())?;
            bank.current_scene = self
                .current_scene_index()
                .min(bank.scene_count().saturating_sub(1));
            bank.edit_current_graph_overrides(edit)?
        };
        self.publish_scheduler_snapshot();
        Ok(result)
    }

    pub fn set_neural_reset_step(
        &self,
        track: usize,
        step: usize,
        enabled: bool,
    ) -> Result<bool, String> {
        if track >= self.active_track_count() {
            return Err("track out of range".to_string());
        }
        if step >= MAX_STEPS {
            return Err("step out of range".to_string());
        }
        self.pattern.neural_reset_patterns[track].set_step_active(step, enabled);
        self.transport.pattern_epoch.fetch_add(1, Ordering::Relaxed);
        self.publish_scheduler_snapshot();
        Ok(enabled)
    }

    pub fn request_track_delete_boundary(&self, track_idx: usize) -> u64 {
        let request_id = self
            .transport
            .topology_edit_request_id
            .fetch_add(1, Ordering::AcqRel)
            + 1;
        self.transport
            .topology_edit_track
            .store(track_idx as u32, Ordering::Release);
        self.transport
            .topology_edit_kind
            .store(TOPOLOGY_EDIT_DELETE_TRACK, Ordering::Release);
        request_id
    }

    pub fn topology_edit_ready(&self, request_id: u64) -> bool {
        self.transport
            .topology_edit_ready_id
            .load(Ordering::Acquire)
            >= request_id
    }

    pub fn topology_edit_in_flight(&self) -> bool {
        self.transport.topology_edit_kind.load(Ordering::Acquire) != TOPOLOGY_EDIT_NONE
    }

    pub fn complete_topology_edit(&self, request_id: u64) {
        self.transport
            .topology_epoch
            .fetch_add(1, Ordering::Relaxed);
        self.transport
            .topology_edit_applied_id
            .store(request_id, Ordering::Release);
        self.transport
            .topology_edit_kind
            .store(TOPOLOGY_EDIT_NONE, Ordering::Release);
        self.transport
            .topology_edit_track
            .store(u32::MAX, Ordering::Release);
    }

    fn reset_track_params_to_default(&self, track: usize) {
        let defaults = TrackParamsSnapshot::default();
        let params = &self.pattern.track_params[track];
        params.gate.store(defaults.gate, Ordering::Relaxed);
        params.set_attack_ms(defaults.attack_ms);
        params.set_release_ms(defaults.release_ms);
        params.set_swing(defaults.swing);
        params.set_swing_resolution(defaults.swing_resolution);
        params.set_num_steps(defaults.num_steps);
        params.set_volume(defaults.volume);
        params.set_pan(defaults.pan);
        params.set_mute(defaults.mute);
        params.set_solo(defaults.solo);
        params.set_send(defaults.send);
        params.set_output(defaults.output);
        params.set_sends(defaults.sends);
        params
            .polyphonic
            .store(defaults.polyphonic, Ordering::Relaxed);
        params.set_max_polyphony(defaults.max_polyphony);
        params.set_timebase(defaults.timebase);
        params.set_accumulator_idx(defaults.accumulator_idx);
        params.set_script_accumulator_name(defaults.script_accumulator_name);
        params.set_midi_fx_chain(defaults.midi_fx_chain);
        params.set_midi_fx_position(defaults.midi_fx_position);
        params.set_accum_limit(defaults.accum_limit);
        params.set_accum_mode(defaults.accum_mode);
        params.set_fts_scale(defaults.fts_scale);
        params.set_mute_group(defaults.mute_group);
        params.set_global_transpose(defaults.global_transpose);
    }

    pub fn clear_live_track_state(&self, track_count: usize) {
        for track in 0..track_count.min(MAX_TRACKS) {
            self.clear_live_track_lane(track);
            self.clear_runtime_track_binding_in_place(track);
        }
    }

    fn clear_live_track_lane(&self, track: usize) {
        self.pattern.patterns[track].store_bits([0u64; TRACK_PATTERN_WORDS]);
        self.pattern.neural_reset_patterns[track].store_bits([0u64; TRACK_PATTERN_WORDS]);
        for step in 0..MAX_STEPS {
            for param in StepParam::ALL {
                self.pattern.step_data[track].set(step, param, param.default_value());
            }
            self.pattern.chord_data[track].clear_step(step);
            self.pattern.timebase_plocks[track].clear(step);
            self.pattern.swing_plocks[track].clear(step);
            self.pattern.swing_resolution_plocks[track].clear(step);
        }
        self.reset_track_params_to_default(track);
        for slot in &self.pattern.effect_chains[track] {
            slot.clear();
        }
        for slot in &self.pattern.midi_fx_slots[track] {
            slot.clear();
        }
        self.pattern.instrument_slots[track].clear();
        self.pattern.instrument_base_note_offsets[track].store(0.0f32.to_bits(), Ordering::Relaxed);
        self.pattern.instrument_run_modes[track].store(
            CustomInstrumentRunMode::Instrument.runtime_flag(),
            Ordering::Relaxed,
        );
        if let Some(sound) = self
            .pattern
            .track_sound_state
            .lock()
            .unwrap()
            .get_mut(track)
        {
            *sound = TrackSoundState::default();
        }
        if let Some(rack_track) = self.pattern.rack_tracks.lock().unwrap().get_mut(track) {
            *rack_track = None;
        }
        if let Some(chain) = self.pattern.process_chains.lock().unwrap().get_mut(track) {
            *chain = crate::process::TrackProcessChain::default();
        }
        if let Some(overrides) = self
            .pattern
            .project_process_lane_overrides
            .lock()
            .unwrap()
            .get_mut(track)
        {
            overrides.clear();
        }
        if let Some(registry) = self
            .pattern
            .plock_variant_registries
            .lock()
            .unwrap()
            .get_mut(track)
        {
            *registry = PlockVariantRegistry::default();
        }
        if let Some(registry) = self
            .pattern
            .key_lock_variant_registries
            .lock()
            .unwrap()
            .get_mut(track)
        {
            *registry = PlockVariantRegistry::default();
        }
    }

    fn clear_runtime_track_binding_in_place(&self, track: usize) {
        self.set_scene_silenced(track, false);
        self.transport.track_playheads[track].store(0, Ordering::Relaxed);
        self.transport.trigger_flash[track].store(0, Ordering::Relaxed);
        self.runtime.sampler_lids[track].store(0, Ordering::Relaxed);
        self.runtime.modulator_lids[track].store(0, Ordering::Relaxed);
        self.runtime.voice_counts[track].store(0, Ordering::Relaxed);
        self.runtime.instrument_type_flags[track].store(0, Ordering::Relaxed);
        self.runtime.instrument_run_mode_flags[track].store(
            CustomInstrumentRunMode::Instrument.runtime_flag(),
            Ordering::Relaxed,
        );
        self.runtime.track_engine_ids[track].store(u32::MAX, Ordering::Relaxed);
        self.runtime.sampler_analysis_buffer_ids[track].store(u32::MAX, Ordering::Relaxed);
        self.runtime.sampler_analysis_bpm[track].store(0.0_f32.to_bits(), Ordering::Relaxed);
        self.runtime.sampler_onset_ptr_lo[track].store(0, Ordering::Relaxed);
        self.runtime.sampler_onset_ptr_hi[track].store(0, Ordering::Relaxed);
        self.runtime.sampler_analysis_status[track].store(0, Ordering::Relaxed);
        for slot in 0..MAX_RACK_SLOTS {
            self.runtime.rack_slot_pan_lids[track][slot].store(0, Ordering::Relaxed);
        }
        for voice in 0..MAX_VOICES {
            self.runtime.voice_lids[track][voice].store(0, Ordering::Relaxed);
            self.runtime.synth_node_ids[track][voice].store(0, Ordering::Relaxed);
            self.runtime.sampler_gatepitch_node_ids[track][voice].store(0, Ordering::Relaxed);
            self.runtime.sampler_modulator_node_ids[track][voice].store(0, Ordering::Relaxed);
        }
        self.pending_accumulator_reset_tracks[track].store(false, Ordering::Relaxed);
        for engine_id in 0..self.runtime.engine_route_lids.len() {
            for voice in 0..MAX_VOICES {
                self.runtime.engine_route_lids[engine_id][voice][track].store(0, Ordering::Relaxed);
                self.runtime.engine_route_lids_r[engine_id][voice][track]
                    .store(0, Ordering::Relaxed);
                for input in 0..EXT_MOD_INPUT_COUNT {
                    self.runtime.engine_ext_route_lids[engine_id][voice][track][input]
                        .store(0, Ordering::Relaxed);
                }
            }
        }
    }

    fn shift_runtime_track_bindings_left(&self, track_idx: usize, old_count: usize) {
        for idx in track_idx..old_count.saturating_sub(1) {
            let next = idx + 1;
            self.transport.track_playheads[idx].store(
                self.transport.track_playheads[next].load(Ordering::Relaxed),
                Ordering::Relaxed,
            );
            self.transport.trigger_flash[idx].store(
                self.transport.trigger_flash[next].load(Ordering::Relaxed),
                Ordering::Relaxed,
            );
            self.pattern.scene_silenced[idx].store(
                self.pattern.scene_silenced[next].load(Ordering::Relaxed),
                Ordering::Relaxed,
            );
            self.runtime.sampler_lids[idx].store(
                self.runtime.sampler_lids[next].load(Ordering::Relaxed),
                Ordering::Relaxed,
            );
            self.runtime.modulator_lids[idx].store(
                self.runtime.modulator_lids[next].load(Ordering::Relaxed),
                Ordering::Relaxed,
            );
            self.runtime.pan_lids[idx].store(
                self.runtime.pan_lids[next].load(Ordering::Relaxed),
                Ordering::Relaxed,
            );
            self.runtime.delay_lids[idx].store(
                self.runtime.delay_lids[next].load(Ordering::Relaxed),
                Ordering::Relaxed,
            );
            self.runtime.send_lids[idx].store(
                self.runtime.send_lids[next].load(Ordering::Relaxed),
                Ordering::Relaxed,
            );
            for slot in 0..MAX_RACK_SLOTS {
                self.runtime.rack_slot_pan_lids[idx][slot].store(
                    self.runtime.rack_slot_pan_lids[next][slot].load(Ordering::Relaxed),
                    Ordering::Relaxed,
                );
            }
            self.runtime.voice_counts[idx].store(
                self.runtime.voice_counts[next].load(Ordering::Relaxed),
                Ordering::Relaxed,
            );
            self.runtime.instrument_type_flags[idx].store(
                self.runtime.instrument_type_flags[next].load(Ordering::Relaxed),
                Ordering::Relaxed,
            );
            self.runtime.instrument_run_mode_flags[idx].store(
                self.runtime.instrument_run_mode_flags[next].load(Ordering::Relaxed),
                Ordering::Relaxed,
            );
            self.runtime.track_engine_ids[idx].store(
                self.runtime.track_engine_ids[next].load(Ordering::Relaxed),
                Ordering::Relaxed,
            );
            self.runtime.sampler_analysis_buffer_ids[idx].store(
                self.runtime.sampler_analysis_buffer_ids[next].load(Ordering::Relaxed),
                Ordering::Relaxed,
            );
            self.runtime.sampler_analysis_bpm[idx].store(
                self.runtime.sampler_analysis_bpm[next].load(Ordering::Relaxed),
                Ordering::Relaxed,
            );
            self.runtime.sampler_onset_ptr_lo[idx].store(
                self.runtime.sampler_onset_ptr_lo[next].load(Ordering::Relaxed),
                Ordering::Relaxed,
            );
            self.runtime.sampler_onset_ptr_hi[idx].store(
                self.runtime.sampler_onset_ptr_hi[next].load(Ordering::Relaxed),
                Ordering::Relaxed,
            );
            self.runtime.sampler_analysis_status[idx].store(
                self.runtime.sampler_analysis_status[next].load(Ordering::Relaxed),
                Ordering::Relaxed,
            );
            for voice in 0..MAX_VOICES {
                self.runtime.voice_lids[idx][voice].store(
                    self.runtime.voice_lids[next][voice].load(Ordering::Relaxed),
                    Ordering::Relaxed,
                );
                self.runtime.synth_node_ids[idx][voice].store(
                    self.runtime.synth_node_ids[next][voice].load(Ordering::Relaxed),
                    Ordering::Relaxed,
                );
                self.runtime.sampler_gatepitch_node_ids[idx][voice].store(
                    self.runtime.sampler_gatepitch_node_ids[next][voice].load(Ordering::Relaxed),
                    Ordering::Relaxed,
                );
                self.runtime.sampler_modulator_node_ids[idx][voice].store(
                    self.runtime.sampler_modulator_node_ids[next][voice].load(Ordering::Relaxed),
                    Ordering::Relaxed,
                );
            }
            self.pending_accumulator_reset_tracks[idx].store(
                self.pending_accumulator_reset_tracks[next].load(Ordering::Relaxed),
                Ordering::Relaxed,
            );
            for engine_id in 0..self.runtime.engine_route_lids.len() {
                for voice in 0..MAX_VOICES {
                    self.runtime.engine_route_lids[engine_id][voice][idx].store(
                        self.runtime.engine_route_lids[engine_id][voice][next]
                            .load(Ordering::Relaxed),
                        Ordering::Relaxed,
                    );
                    self.runtime.engine_route_lids_r[engine_id][voice][idx].store(
                        self.runtime.engine_route_lids_r[engine_id][voice][next]
                            .load(Ordering::Relaxed),
                        Ordering::Relaxed,
                    );
                    for input in 0..EXT_MOD_INPUT_COUNT {
                        self.runtime.engine_ext_route_lids[engine_id][voice][idx][input].store(
                            self.runtime.engine_ext_route_lids[engine_id][voice][next][input]
                                .load(Ordering::Relaxed),
                            Ordering::Relaxed,
                        );
                    }
                }
            }
        }

        if old_count == 0 {
            return;
        }
        let last = old_count - 1;
        self.set_scene_silenced(last, false);
        self.transport.track_playheads[last].store(0, Ordering::Relaxed);
        self.transport.trigger_flash[last].store(0, Ordering::Relaxed);
        self.runtime.sampler_lids[last].store(0, Ordering::Relaxed);
        self.runtime.modulator_lids[last].store(0, Ordering::Relaxed);
        self.runtime.pan_lids[last].store(0, Ordering::Relaxed);
        self.runtime.delay_lids[last].store(0, Ordering::Relaxed);
        self.runtime.send_lids[last].store(0, Ordering::Relaxed);
        for slot in 0..MAX_RACK_SLOTS {
            self.runtime.rack_slot_pan_lids[last][slot].store(0, Ordering::Relaxed);
        }
        self.runtime.voice_counts[last].store(0, Ordering::Relaxed);
        self.runtime.instrument_type_flags[last].store(0, Ordering::Relaxed);
        self.runtime.instrument_run_mode_flags[last].store(
            CustomInstrumentRunMode::Instrument.runtime_flag(),
            Ordering::Relaxed,
        );
        self.runtime.track_engine_ids[last].store(u32::MAX, Ordering::Relaxed);
        self.runtime.sampler_analysis_buffer_ids[last].store(u32::MAX, Ordering::Relaxed);
        self.runtime.sampler_analysis_bpm[last].store(0.0_f32.to_bits(), Ordering::Relaxed);
        self.runtime.sampler_onset_ptr_lo[last].store(0, Ordering::Relaxed);
        self.runtime.sampler_onset_ptr_hi[last].store(0, Ordering::Relaxed);
        self.runtime.sampler_analysis_status[last].store(0, Ordering::Relaxed);
        for voice in 0..MAX_VOICES {
            self.runtime.voice_lids[last][voice].store(0, Ordering::Relaxed);
            self.runtime.synth_node_ids[last][voice].store(0, Ordering::Relaxed);
            self.runtime.sampler_gatepitch_node_ids[last][voice].store(0, Ordering::Relaxed);
            self.runtime.sampler_modulator_node_ids[last][voice].store(0, Ordering::Relaxed);
        }
        self.pending_accumulator_reset_tracks[last].store(false, Ordering::Relaxed);
        for engine_id in 0..self.runtime.engine_route_lids.len() {
            for voice in 0..MAX_VOICES {
                self.runtime.engine_route_lids[engine_id][voice][last].store(0, Ordering::Relaxed);
                self.runtime.engine_route_lids_r[engine_id][voice][last]
                    .store(0, Ordering::Relaxed);
                for input in 0..EXT_MOD_INPUT_COUNT {
                    self.runtime.engine_ext_route_lids[engine_id][voice][last][input]
                        .store(0, Ordering::Relaxed);
                }
            }
        }
    }

    /// Delete one track from live sequencer state and compact higher track indices.
    ///
    /// This state-side helper is the non-graph half of track deletion semantics:
    /// the deleted lane disappears from the current pattern and all snapshots in
    /// memory, higher lanes shift down immediately, and the old trailing lane is
    /// cleared so stale state cannot leak back after future restores.
    pub fn capture_track_pattern_lane_state(
        &self,
        track_idx: usize,
        effect_descriptors: &[Vec<EffectDescriptor>],
    ) -> Result<TrackPatternLaneState, String> {
        let scenes = self.pattern.scenes.lock().unwrap();
        let pool = scenes.track_pools.get(track_idx)
            .cloned()
            .ok_or_else(|| format!("Track {} has no pattern pool", track_idx + 1))?;
        let scene_cells = scenes.scenes.iter()
            .map(|scene| scene.cells.get(track_idx).copied().flatten())
            .collect();
        let track_override = scenes.track_overrides.get(track_idx).copied().flatten();
        let scene_references = scenes.scenes.iter().map(|scene| SceneTrackReferenceState {
            mod_connections: scene.mod_connections.clone(),
            neural_networks: scene.neural_networks.clone(),
            graph_overrides: scene.graph_overrides.clone(),
        }).collect();
        let mut sidechains = Vec::new();
        for (owner_track, owner_pool) in scenes.track_pools.iter().enumerate() {
            if owner_track == track_idx {
                continue;
            }
            let Some(descriptors) = effect_descriptors.get(owner_track) else {
                continue;
            };
            let sidechain_slots = descriptors.iter().enumerate()
                .filter(|(_, descriptor)| descriptor.params.iter().any(|param| {
                    matches!(param.host_control, Some(HostControl::FxSidechain { .. }))
                }))
                .map(|(slot, _)| slot)
                .collect::<Vec<_>>();
            if sidechain_slots.is_empty() {
                continue;
            }
            for (pattern, data) in &owner_pool.patterns {
                let slots = sidechain_slots.iter().filter_map(|slot| {
                    data.effect_slots.get(*slot).cloned().map(|state| (*slot, state))
                }).collect::<Vec<_>>();
                if !slots.is_empty() {
                    sidechains.push(TrackSidechainPatternState {
                        owner_track,
                        pattern: *pattern,
                        slots,
                    });
                }
            }
        }
        Ok(TrackPatternLaneState {
            pool,
            scene_cells,
            track_override,
            scene_references,
            sidechains,
        })
    }

    pub fn replace_appended_track_pattern_lane(
        &self,
        snapshot: &TrackPatternLaneState,
    ) -> Result<usize, String> {
        let mut scenes = self.pattern.scenes.lock().unwrap();
        let track = scenes.track_pools.len().checked_sub(1)
            .ok_or_else(|| "Cannot restore a track lane into an empty project".to_string())?;
        scenes.track_pools[track] = snapshot.pool.clone();
        scenes.track_overrides[track] = snapshot.track_override;
        if scenes.scenes.len() != snapshot.scene_cells.len() {
            return Err("Track history scene topology no longer matches the project".to_string());
        }
        for (scene, cell) in scenes.scenes.iter_mut().zip(&snapshot.scene_cells) {
            scene.cells[track] = *cell;
        }
        let current = scenes.current_scene;
        let live = scenes.scene_snapshot(current)
            .ok_or_else(|| "Current scene is missing during track restore".to_string())?;
        drop(scenes);
        live.restore(self);
        Ok(track)
    }

    pub fn move_appended_track_pattern_lane_to(
        &self,
        target: usize,
        snapshot: &TrackPatternLaneState,
    ) -> Result<(), String> {
        let mut scenes = self.pattern.scenes.lock().unwrap();
        let last = scenes.track_pools.len().checked_sub(1)
            .ok_or_else(|| "Cannot move a track lane in an empty project".to_string())?;
        if target > last || scenes.scenes.len() != snapshot.scene_references.len() {
            return Err("Track history topology no longer matches the project".to_string());
        }
        let pool = scenes.track_pools.remove(last);
        scenes.track_pools.insert(target, pool);
        let track_override = scenes.track_overrides.remove(last);
        scenes.track_overrides.insert(target, track_override);
        for ((scene, references), expected_cell) in scenes.scenes.iter_mut()
            .zip(&snapshot.scene_references)
            .zip(&snapshot.scene_cells)
        {
            let cell = scene.cells.remove(last);
            scene.cells.insert(target, cell);
            if scene.cells[target] != *expected_cell {
                return Err("Restored Track Pattern assignment changed during insertion".to_string());
            }
            scene.mod_connections = references.mod_connections.clone();
            scene.neural_networks = references.neural_networks.clone();
            scene.graph_overrides = references.graph_overrides.clone();
        }
        for saved in &snapshot.sidechains {
            let Some(data) = scenes.track_pools.get_mut(saved.owner_track)
                .and_then(|pool| pool.patterns.get_mut(&saved.pattern)) else {
                return Err(format!(
                    "Sidechain history target track {} pattern {:?} is missing",
                    saved.owner_track + 1,
                    saved.pattern,
                ));
            };
            for (slot, state) in &saved.slots {
                let Some(target_slot) = data.effect_slots.get_mut(*slot) else {
                    return Err(format!("Sidechain history effect slot {} is missing", slot + 1));
                };
                *target_slot = state.clone();
            }
        }
        let current = scenes.current_scene;
        let live = scenes.scene_snapshot(current)
            .ok_or_else(|| "Current scene is missing during track insertion".to_string())?;
        drop(scenes);
        live.restore(self);
        self.transport.pattern_epoch.fetch_add(1, Ordering::Relaxed);
        self.schedule_mod_resync();
        self.request_all_accumulator_resets();
        Ok(())
    }

    pub fn remove_track(
        &self,
        track_idx: usize,
        buffer_ids: &[i32],
        sample_rates: &[u32],
        names: &[String],
        instrument_types: &[InstrumentType],
        effect_descriptors: &[Vec<EffectDescriptor>],
    ) -> bool {
        let old_count = self.active_track_count();
        if old_count <= 1 || track_idx >= old_count {
            return false;
        }

        let current_pattern = self.current_scene_index();
        let mut current_snapshot = self.capture_current_pattern_snapshot(
            old_count,
            buffer_ids,
            sample_rates,
            names,
            instrument_types,
        );
        {
            let mut scenes = self.pattern.scenes.lock().unwrap();
            scenes.save_scene_snapshot(current_pattern, current_snapshot.clone());
            for owner_track in 0..old_count {
                if owner_track == track_idx {
                    continue;
                }
                let Some(track_descs) = effect_descriptors.get(owner_track) else {
                    continue;
                };
                let Some(pool) = scenes.track_pools.get_mut(owner_track) else {
                    continue;
                };
                for data in pool.patterns.values_mut() {
                    data.remap_sidechain_references_after_track_delete(
                        owner_track,
                        track_descs,
                        track_idx,
                        old_count,
                    );
                }
            }
            scenes.remove_track(track_idx);
        }

        remap_snapshot_sidechain_references_after_track_delete(
            &mut current_snapshot,
            effect_descriptors,
            track_idx,
            old_count,
        );
        current_snapshot.remove_track(track_idx);
        current_snapshot.restore(self);
        self.shift_runtime_track_bindings_left(track_idx, old_count);
        self.clear_live_track_lane(old_count - 1);
        self.transport
            .num_tracks
            .store((old_count - 1) as u32, Ordering::Release);
        self.transport.pattern_epoch.fetch_add(1, Ordering::Relaxed);
        self.schedule_mod_resync();
        self.request_all_accumulator_resets();
        self.publish_scheduler_snapshot();
        true
    }

    pub fn clear_track_in_place(
        &self,
        track_idx: usize,
        effect_descriptors: &[Vec<EffectDescriptor>],
    ) -> bool {
        let track_count = self.active_track_count();
        if track_idx >= track_count {
            return false;
        }

        {
            let mut scenes = self.pattern.scenes.lock().unwrap();
            let Some(pool) = scenes.track_pools.get_mut(track_idx) else {
                return false;
            };
            for data in pool.patterns.values_mut() {
                data.clear(track_idx, effect_descriptors, InstrumentType::Sampler);
            }
        }

        self.clear_live_track_lane(track_idx);
        self.clear_runtime_track_binding_in_place(track_idx);
        self.transport.pattern_epoch.fetch_add(1, Ordering::Relaxed);
        self.schedule_mod_resync();
        self.request_all_accumulator_resets();
        self.publish_scheduler_snapshot();
        true
    }

    pub fn scratch_source(&self) -> String {
        self.scratch_source.lock().unwrap().clone()
    }
    pub fn scratch_source_version(&self) -> u64 {
        self.scratch_source_version.load(Ordering::Acquire)
    }
    pub fn set_scratch_source(&self, source: impl Into<String>) {
        *self.scratch_source.lock().unwrap() = source.into();
        self.scratch_source_version.fetch_add(1, Ordering::AcqRel);
    }
    /// Publish (upsert by id) a UI-authored generator definition for the scheduler.
    pub fn publish_sequencer(&self, sequencer: PublishedSequencer) {
        {
            let mut list = self.published_sequencers.lock().unwrap();
            if let Some(existing) = list.iter_mut().find(|s| s.id == sequencer.id) {
                *existing = sequencer;
            } else {
                list.push(sequencer);
            }
        }
        self.published_sequencers_version
            .fetch_add(1, Ordering::AcqRel);
    }
    pub fn unpublish_sequencer_by_name(&self, name: &str) -> bool {
        let removed = {
            let mut list = self.published_sequencers.lock().unwrap();
            let before = list.len();
            list.retain(|sequencer| sequencer.name != name);
            list.len() != before
        };
        if removed {
            self.published_sequencers_version
                .fetch_add(1, Ordering::AcqRel);
        }
        removed
    }
    pub fn published_sequencers(&self) -> Vec<PublishedSequencer> {
        self.published_sequencers.lock().unwrap().clone()
    }
    pub fn published_sequencers_version(&self) -> u64 {
        self.published_sequencers_version.load(Ordering::Acquire)
    }
    /// Publish the complete UI-authored process/channel authoring snapshot.
    pub fn publish_process_authoring(
        &self,
        snapshot: crate::process::PublishedProcessAuthoringSnapshot,
    ) {
        *self.published_process_authoring.lock().unwrap() = snapshot;
        self.published_process_authoring_version
            .fetch_add(1, Ordering::AcqRel);
    }
    pub fn published_process_authoring(&self) -> crate::process::PublishedProcessAuthoringSnapshot {
        self.published_process_authoring.lock().unwrap().clone()
    }
    pub fn published_process_authoring_version(&self) -> u64 {
        self.published_process_authoring_version
            .load(Ordering::Acquire)
    }
    pub fn track_process_chain(&self, track: usize) -> Option<crate::process::TrackProcessChain> {
        if track >= self.active_track_count() {
            return None;
        }
        self.pattern
            .process_chains
            .lock()
            .unwrap()
            .get(track)
            .cloned()
    }
    pub fn set_track_process_chain(
        &self,
        track: usize,
        chain: crate::process::TrackProcessChain,
    ) -> bool {
        if track >= self.active_track_count() {
            return false;
        }
        let mut chains = self.pattern.process_chains.lock().unwrap();
        let Some(slot) = chains.get_mut(track) else {
            return false;
        };
        *slot = chain;
        drop(chains);
        self.transport.pattern_epoch.fetch_add(1, Ordering::Relaxed);
        self.publish_scheduler_snapshot();
        true
    }
    fn publish_process_chain_edit(&self) {
        self.transport.pattern_epoch.fetch_add(1, Ordering::Relaxed);
        self.publish_scheduler_snapshot();
    }
    /// A track's effective chain as the scheduler sees it: project-layer
    /// slots ahead of the track's own slots. UI surfaces read this composed
    /// view so project slots appear (badged) in every track's process column.
    pub fn composed_track_process_chain(
        &self,
        track: usize,
    ) -> Option<crate::process::TrackProcessChain> {
        let track_chain = self.track_process_chain(track)?;
        let mut project_chain = self.project_process_chain();
        if let Some(overrides) = self
            .pattern
            .project_process_lane_overrides
            .lock()
            .unwrap()
            .get(track)
        {
            crate::process::apply_project_lane_overrides(&mut project_chain, overrides);
        }
        Some(crate::process::compose_effective_process_chain(
            &project_chain,
            &track_chain,
        ))
    }
    /// The project-level default process chain for the current scene. Every
    /// track — present and future — runs these slots ahead of its own chain.
    pub fn project_process_chain(&self) -> crate::process::TrackProcessChain {
        self.pattern
            .scenes
            .lock()
            .unwrap()
            .current_project_process_chain()
    }
    /// Whole-layer replace of the project process chain (`(processes :project ...)`).
    pub fn set_project_process_chain(&self, chain: crate::process::TrackProcessChain) -> bool {
        let identities = chain
            .slots
            .iter()
            .map(crate::process::project_slot_identity_id)
            .collect::<std::collections::BTreeSet<_>>();
        let updated = {
            let mut scenes = self.pattern.scenes.lock().unwrap();
            scenes
                .edit_current_project_process_chain(|current| {
                    *current = chain;
                    Ok(())
                })
                .is_ok()
        };
        if updated {
            for overrides in self
                .pattern
                .project_process_lane_overrides
                .lock()
                .unwrap()
                .iter_mut()
            {
                overrides.retain(|identity, _| identities.contains(identity));
            }
            self.publish_process_chain_edit();
        }
        updated
    }
    fn edit_project_process_chain_slot<R>(
        &self,
        instance_id: crate::process::ProcessInstanceId,
        edit: impl FnOnce(&mut crate::process::TrackProcessSlot) -> R,
    ) -> Option<R> {
        let mut scenes = self.pattern.scenes.lock().unwrap();
        scenes
            .edit_current_project_process_chain(|chain| {
                Ok(chain
                    .slots
                    .iter_mut()
                    .find(|slot| slot.instance_id == instance_id)
                    .map(edit))
            })
            .ok()
            .flatten()
    }
    /// Enable or bypass one pattern-scoped process-chain slot.
    ///
    /// Returns `false` only when the track or instance does not exist. A
    /// repeated write of the current value is a successful no-op.
    pub fn set_track_process_slot_enabled(
        &self,
        track: usize,
        instance_id: crate::process::ProcessInstanceId,
        enabled: bool,
    ) -> bool {
        if track >= self.active_track_count() {
            return false;
        }
        let changed = {
            let mut chains = self.pattern.process_chains.lock().unwrap();
            let Some(chain) = chains.get_mut(track) else {
                return false;
            };
            match chain
                .slots
                .iter_mut()
                .find(|slot| slot.instance_id == instance_id)
            {
                Some(slot) => {
                    let changed = slot.enabled != enabled;
                    slot.enabled = enabled;
                    Some(changed)
                }
                None => None,
            }
        };
        // Project-layer slots are editable from any track's panel; the toggle
        // lands on the one shared object.
        let Some(changed) = changed.or_else(|| {
            self.edit_project_process_chain_slot(instance_id, |slot| {
                let changed = slot.enabled != enabled;
                slot.enabled = enabled;
                changed
            })
        }) else {
            return false;
        };
        if changed {
            self.publish_process_chain_edit();
        }
        true
    }
    /// Move a slot before another instance, or to the end when `before` is
    /// `None`. Instance ids make this stable across reactive UI refreshes and
    /// avoid index-shift ambiguity while dragging downward.
    pub fn move_track_process_slot_before(
        &self,
        track: usize,
        instance_id: crate::process::ProcessInstanceId,
        before: Option<crate::process::ProcessInstanceId>,
    ) -> bool {
        if track >= self.active_track_count() {
            return false;
        }
        fn move_slot_within_chain(
            chain: &mut crate::process::TrackProcessChain,
            instance_id: crate::process::ProcessInstanceId,
            before: Option<crate::process::ProcessInstanceId>,
        ) -> Option<bool> {
            let source_index = chain
                .slots
                .iter()
                .position(|slot| slot.instance_id == instance_id)?;
            if before == Some(instance_id) {
                return Some(false);
            }
            if before
                .is_some_and(|target| !chain.slots.iter().any(|slot| slot.instance_id == target))
            {
                return None;
            }
            let previous_order = chain
                .slots
                .iter()
                .map(|slot| slot.instance_id)
                .collect::<Vec<_>>();
            let slot = chain.slots.remove(source_index);
            let target_index = before
                .and_then(|target| {
                    chain
                        .slots
                        .iter()
                        .position(|slot| slot.instance_id == target)
                })
                .unwrap_or(chain.slots.len());
            chain.slots.insert(target_index, slot);
            Some(
                previous_order
                    != chain
                        .slots
                        .iter()
                        .map(|slot| slot.instance_id)
                        .collect::<Vec<_>>(),
            )
        }
        let changed = {
            let mut chains = self.pattern.process_chains.lock().unwrap();
            let Some(chain) = chains.get_mut(track) else {
                return false;
            };
            move_slot_within_chain(chain, instance_id, before)
        };
        // Reordering a project slot moves it within the project layer only.
        let changed = changed.or_else(|| {
            let mut scenes = self.pattern.scenes.lock().unwrap();
            scenes
                .edit_current_project_process_chain(|chain| {
                    Ok(move_slot_within_chain(chain, instance_id, before))
                })
                .ok()
                .flatten()
        });
        let Some(changed) = changed else {
            return false;
        };
        if changed {
            self.publish_process_chain_edit();
        }
        true
    }
    /// Detach one slot from one track in the current pattern.
    pub fn remove_track_process_slot(
        &self,
        track: usize,
        instance_id: crate::process::ProcessInstanceId,
    ) -> bool {
        if track >= self.active_track_count() {
            return false;
        }
        let removed = {
            let mut chains = self.pattern.process_chains.lock().unwrap();
            let Some(chain) = chains.get_mut(track) else {
                return false;
            };
            let previous_len = chain.slots.len();
            chain.slots.retain(|slot| slot.instance_id != instance_id);
            chain.slots.len() != previous_len
        };
        // A project slot has no per-track detach: removing it from any track's
        // panel removes the shared slot from the project layer.
        let mut removed_identity = None;
        let removed = removed || {
            let mut scenes = self.pattern.scenes.lock().unwrap();
            scenes
                .edit_current_project_process_chain(|chain| {
                    removed_identity = chain
                        .slots
                        .iter()
                        .find(|slot| slot.instance_id == instance_id)
                        .map(crate::process::project_slot_identity_id);
                    let previous_len = chain.slots.len();
                    chain.slots.retain(|slot| slot.instance_id != instance_id);
                    Ok(chain.slots.len() != previous_len)
                })
                .unwrap_or(false)
        };
        if removed {
            if let Some(identity) = removed_identity {
                for overrides in self
                    .pattern
                    .project_process_lane_overrides
                    .lock()
                    .unwrap()
                    .iter_mut()
                {
                    overrides.remove(&identity);
                }
            }
            self.publish_process_chain_edit();
        }
        removed
    }
    pub fn set_process_lane_value(
        &self,
        track: usize,
        instance_id: crate::process::ProcessInstanceId,
        inlet_name: impl Into<String>,
        step: usize,
        value: f32,
    ) -> bool {
        if track >= self.active_track_count() || step >= MAX_STEPS {
            return false;
        }
        let inlet_name = inlet_name.into();
        let write_lane_step = |slot: &mut crate::process::TrackProcessSlot| {
            let lane = slot.lanes.entry(inlet_name.clone()).or_default();
            if lane.values.len() <= step {
                lane.values.resize(step + 1, 0.0);
            }
            lane.values[step] = value;
        };
        let updated = {
            let mut chains = self.pattern.process_chains.lock().unwrap();
            let Some(chain) = chains.get_mut(track) else {
                return false;
            };
            match chain
                .slots
                .iter_mut()
                .find(|slot| slot.instance_id == instance_id)
            {
                Some(slot) => {
                    write_lane_step(slot);
                    true
                }
                None => false,
            }
        };
        if !updated {
            let project_slot = self
                .project_process_chain()
                .slots
                .into_iter()
                .find(|slot| slot.instance_id == instance_id);
            let Some(project_slot) = project_slot else {
                return false;
            };
            let identity = crate::process::project_slot_identity_id(&project_slot);
            let mut overrides = self.pattern.project_process_lane_overrides.lock().unwrap();
            let Some(track_overrides) = overrides.get_mut(track) else {
                return false;
            };
            let lane = track_overrides
                .entry(identity)
                .or_default()
                .entry(inlet_name.clone())
                .or_insert_with(|| {
                    project_slot
                        .lanes
                        .get(&inlet_name)
                        .cloned()
                        .unwrap_or_default()
                });
            if lane.values.len() <= step {
                lane.values.resize(step + 1, 0.0);
            }
            lane.values[step] = value;
        }
        self.transport.pattern_epoch.fetch_add(1, Ordering::Relaxed);
        self.publish_scheduler_snapshot();
        true
    }
    pub fn clear_project_process_lane_override(
        &self,
        track: usize,
        instance_id: crate::process::ProcessInstanceId,
        inlet_name: &str,
    ) -> bool {
        let Some(slot) = self
            .project_process_chain()
            .slots
            .into_iter()
            .find(|slot| slot.instance_id == instance_id)
        else {
            return false;
        };
        let identity = crate::process::project_slot_identity_id(&slot);
        let mut all = self.pattern.project_process_lane_overrides.lock().unwrap();
        let Some(track_overrides) = all.get_mut(track) else {
            return false;
        };
        let removed = track_overrides
            .get_mut(&identity)
            .is_some_and(|lanes| lanes.remove(inlet_name).is_some());
        if track_overrides
            .get(&identity)
            .is_some_and(|lanes| lanes.is_empty())
        {
            track_overrides.remove(&identity);
        }
        drop(all);
        if removed {
            self.publish_process_chain_edit();
        }
        removed
    }
    pub fn has_project_process_lane_override(
        &self,
        track: usize,
        instance_id: crate::process::ProcessInstanceId,
        inlet_name: &str,
    ) -> bool {
        let Some(slot) = self
            .project_process_chain()
            .slots
            .into_iter()
            .find(|slot| slot.instance_id == instance_id)
        else {
            return false;
        };
        self.pattern
            .project_process_lane_overrides
            .lock()
            .unwrap()
            .get(track)
            .and_then(|overrides| overrides.get(&crate::process::project_slot_identity_id(&slot)))
            .is_some_and(|lanes| lanes.contains_key(inlet_name))
    }
    /// Replace a scalar inlet on every current-pattern chain slot owned by
    /// `instance_id`. This is the durable counterpart to authoring-handle knob
    /// edits like `(climb :limit 6)`: it updates pattern-scoped attachment
    /// state without touching step data or p-lock storage.
    pub fn set_process_inlet_value(
        &self,
        instance_id: crate::process::ProcessInstanceId,
        inlet_name: &str,
        value: crate::process::ProcessLiteral,
    ) -> usize {
        let mut updated = 0;
        {
            let mut chains = self.pattern.process_chains.lock().unwrap();
            for chain in chains.iter_mut() {
                for slot in chain
                    .slots
                    .iter_mut()
                    .filter(|slot| slot.instance_id == instance_id)
                {
                    slot.inlets.insert(inlet_name.to_string(), value.clone());
                    updated += 1;
                }
            }
        }
        if self
            .edit_project_process_chain_slot(instance_id, |slot| {
                slot.inlets.insert(inlet_name.to_string(), value.clone());
            })
            .is_some()
        {
            updated += 1;
        }
        if updated > 0 {
            self.transport.pattern_epoch.fetch_add(1, Ordering::Relaxed);
            self.publish_scheduler_snapshot();
        }
        updated
    }
    /// Replace a scalar inlet on one track attachment. UI slot editors use
    /// this track-local form; authored process handles intentionally retain
    /// the all-attachments behavior of `set_process_inlet_value` above.
    pub fn set_track_process_inlet_value(
        &self,
        track: usize,
        instance_id: crate::process::ProcessInstanceId,
        inlet_name: &str,
        value: crate::process::ProcessLiteral,
    ) -> bool {
        if track >= self.active_track_count() {
            return false;
        }
        let changed = {
            let mut chains = self.pattern.process_chains.lock().unwrap();
            let Some(chain) = chains.get_mut(track) else {
                return false;
            };
            chain
                .slots
                .iter_mut()
                .find(|slot| slot.instance_id == instance_id)
                .map(|slot| {
                    let changed = slot.inlets.get(inlet_name) != Some(&value);
                    slot.inlets.insert(inlet_name.to_string(), value.clone());
                    changed
                })
        };
        let Some(changed) = changed.or_else(|| {
            self.edit_project_process_chain_slot(instance_id, |slot| {
                let changed = slot.inlets.get(inlet_name) != Some(&value);
                slot.inlets.insert(inlet_name.to_string(), value.clone());
                changed
            })
        }) else {
            return false;
        };
        if changed {
            self.publish_process_chain_edit();
        }
        true
    }
    pub fn set_process_port_binding(
        &self,
        track: usize,
        instance_id: crate::process::ProcessInstanceId,
        port_name: &str,
        target: crate::process::ParamTarget,
    ) -> bool {
        if track >= self.active_track_count() {
            return false;
        }

        let apply = |slot: &mut crate::process::TrackProcessSlot| {
            let current = slot.bindings.get(port_name);
            if matches!(current, Some(Some(existing)) if existing == &target) {
                false
            } else {
                slot.bindings
                    .insert(port_name.to_string(), Some(target.clone()));
                true
            }
        };
        let changed = {
            let mut chains = self.pattern.process_chains.lock().unwrap();
            let Some(chain) = chains.get_mut(track) else {
                return false;
            };
            chain
                .slots
                .iter_mut()
                .find(|slot| slot.instance_id == instance_id)
                .map(apply)
        };
        let Some(changed) =
            changed.or_else(|| self.edit_project_process_chain_slot(instance_id, apply))
        else {
            return false;
        };
        if changed {
            self.transport.pattern_epoch.fetch_add(1, Ordering::Relaxed);
            self.publish_scheduler_snapshot();
        }
        true
    }
    /// Bind a process target port on every current-pattern chain slot owned by
    /// `instance_id`. This mirrors `set_process_lane_values`: authored Lisp and
    /// UI interactions both update the pattern-owned slots currently attached to
    /// tracks without touching step/plock storage.
    pub fn set_process_port_binding_for_instance(
        &self,
        instance_id: crate::process::ProcessInstanceId,
        port_name: &str,
        target: crate::process::ParamTarget,
    ) -> usize {
        let mut updated = 0;
        let mut changed = false;
        {
            let mut chains = self.pattern.process_chains.lock().unwrap();
            for chain in chains.iter_mut() {
                for slot in chain
                    .slots
                    .iter_mut()
                    .filter(|slot| slot.instance_id == instance_id)
                {
                    updated += 1;
                    let current = slot.bindings.get(port_name);
                    if !matches!(current, Some(Some(existing)) if existing == &target) {
                        slot.bindings
                            .insert(port_name.to_string(), Some(target.clone()));
                        changed = true;
                    }
                }
            }
        }
        if let Some(project_changed) = self.edit_project_process_chain_slot(instance_id, |slot| {
            let current = slot.bindings.get(port_name);
            if matches!(current, Some(Some(existing)) if existing == &target) {
                false
            } else {
                slot.bindings
                    .insert(port_name.to_string(), Some(target.clone()));
                true
            }
        }) {
            updated += 1;
            changed |= project_changed;
        }
        if changed {
            self.transport.pattern_epoch.fetch_add(1, Ordering::Relaxed);
            self.publish_scheduler_snapshot();
        }
        updated
    }
    pub fn clear_process_port_binding(
        &self,
        track: usize,
        instance_id: crate::process::ProcessInstanceId,
        port_name: &str,
    ) -> bool {
        if track >= self.active_track_count() {
            return false;
        }

        let changed = {
            let mut chains = self.pattern.process_chains.lock().unwrap();
            let Some(chain) = chains.get_mut(track) else {
                return false;
            };
            chain
                .slots
                .iter_mut()
                .find(|slot| slot.instance_id == instance_id)
                .map(|slot| slot.bindings.remove(port_name).is_some())
        };
        let Some(changed) = changed.or_else(|| {
            self.edit_project_process_chain_slot(instance_id, |slot| {
                slot.bindings.remove(port_name).is_some()
            })
        }) else {
            return false;
        };
        if changed {
            self.transport.pattern_epoch.fetch_add(1, Ordering::Relaxed);
            self.publish_scheduler_snapshot();
        }
        true
    }
    /// Replace a lane wholesale on every current-pattern chain slot owned by
    /// `instance_id` (a handle can be attached to several tracks). Returns the
    /// number of slots updated; publishes only when at least one matched.
    pub fn set_process_lane_values(
        &self,
        instance_id: crate::process::ProcessInstanceId,
        inlet_name: &str,
        values: Vec<f32>,
    ) -> usize {
        let mut updated = 0;
        {
            let mut chains = self.pattern.process_chains.lock().unwrap();
            for chain in chains.iter_mut() {
                for slot in chain
                    .slots
                    .iter_mut()
                    .filter(|slot| slot.instance_id == instance_id)
                {
                    slot.lanes.insert(
                        inlet_name.to_string(),
                        crate::process::ProcessLane {
                            values: values.clone(),
                        },
                    );
                    updated += 1;
                }
            }
        }
        if self
            .edit_project_process_chain_slot(instance_id, |slot| {
                slot.lanes.insert(
                    inlet_name.to_string(),
                    crate::process::ProcessLane {
                        values: values.clone(),
                    },
                );
            })
            .is_some()
        {
            updated += 1;
        }
        if updated > 0 {
            self.transport.pattern_epoch.fetch_add(1, Ordering::Relaxed);
            self.publish_scheduler_snapshot();
        }
        updated
    }
    pub fn process_instance_attachment_count(
        &self,
        instance_id: crate::process::ProcessInstanceId,
    ) -> usize {
        let track_attachments: usize = self
            .pattern
            .process_chains
            .lock()
            .unwrap()
            .iter()
            .map(|chain| {
                chain
                    .slots
                    .iter()
                    .filter(|slot| slot.instance_id == instance_id)
                    .count()
            })
            .sum();
        let project_attachments = self
            .project_process_chain()
            .slots
            .iter()
            .filter(|slot| slot.instance_id == instance_id)
            .count();
        track_attachments + project_attachments
    }
    pub fn process_inlet_value(
        &self,
        instance_id: crate::process::ProcessInstanceId,
        inlet_name: &str,
    ) -> Option<crate::process::ProcessLiteral> {
        self.pattern
            .process_chains
            .lock()
            .unwrap()
            .iter()
            .flat_map(|chain| chain.slots.iter())
            .find(|slot| slot.instance_id == instance_id)
            .and_then(|slot| slot.inlets.get(inlet_name))
            .cloned()
    }
    pub fn scratch_runtime_descriptors(
        &self,
    ) -> (Vec<Vec<EffectDescriptor>>, Vec<EffectDescriptor>) {
        (
            self.scratch_effect_descriptors.lock().unwrap().clone(),
            self.scratch_instrument_descriptors.lock().unwrap().clone(),
        )
    }
    pub fn set_scratch_runtime_descriptors(
        &self,
        effect_descriptors: Vec<Vec<EffectDescriptor>>,
        instrument_descriptors: Vec<EffectDescriptor>,
    ) {
        {
            *self.scratch_effect_descriptors.lock().unwrap() = effect_descriptors;
        }
        {
            *self.scratch_instrument_descriptors.lock().unwrap() = instrument_descriptors;
        }
        self.publish_scheduler_snapshot();
    }
    pub fn process_trace_enabled(&self) -> bool {
        self.process_trace_enabled.load(Ordering::Relaxed)
    }
    pub fn set_process_trace_enabled(&self, enabled: bool) {
        let previous = self.process_trace_enabled.swap(enabled, Ordering::Relaxed);
        if previous != enabled {
            self.publish_scheduler_snapshot();
        }
    }
    pub fn request_accumulator_reset(&self, track: usize) {
        if track < MAX_TRACKS {
            self.pending_accumulator_reset_tracks[track].store(true, Ordering::Release);
        }
    }
    pub fn request_all_accumulator_resets(&self) {
        self.pending_accumulator_reset_all
            .store(true, Ordering::Release);
    }
    pub fn take_accumulator_reset_requests(&self) -> (bool, [bool; MAX_TRACKS]) {
        let all = self
            .pending_accumulator_reset_all
            .swap(false, Ordering::AcqRel);
        let mut tracks = [false; MAX_TRACKS];
        for (idx, flag) in tracks.iter_mut().enumerate() {
            *flag = self.pending_accumulator_reset_tracks[idx].swap(false, Ordering::AcqRel);
        }
        (all, tracks)
    }
    pub fn current_step(&self) -> usize {
        self.transport.playhead.load(Ordering::Relaxed) as usize
    }
    pub fn track_step(&self, track: usize) -> usize {
        self.transport.track_playheads[track].load(Ordering::Relaxed) as usize
    }

    /// Resolve a track-local step and phase from the transport beat clock.
    /// This mirrors the scheduler's timebase-override and sync-boundary rules
    /// so live recording does not accidentally use the global 16th-note phase.
    pub fn record_position_at_beat(&self, track: usize, beats: f64) -> Option<RecordPosition> {
        if track >= self.active_track_count() || !beats.is_finite() {
            return None;
        }
        let params = &self.pattern.track_params[track];
        let num_steps = params.get_num_steps().clamp(1, MAX_STEPS);
        let default_timebase = params.get_timebase();
        let mut boundaries = [0.0_f64; MAX_STEPS + 1];
        let mut step_ends = [0.0_f64; MAX_STEPS];
        let mut accumulated = 0.0_f64;
        for step in 0..num_steps {
            let timebase = self.pattern.timebase_plocks[track]
                .get(step)
                .unwrap_or(default_timebase);
            let sync = sync_beats(self.pattern.step_data[track].get(step, StepParam::Sync));
            if sync > f64::EPSILON {
                accumulated = (accumulated / sync).ceil() * sync;
            }
            boundaries[step] = accumulated;
            let step_beats = timebase.step_beats(num_steps).max(f64::EPSILON);
            step_ends[step] = accumulated + step_beats;
            accumulated += step_beats;
        }
        boundaries[num_steps] = accumulated;
        let initial_sync = sync_beats(self.pattern.step_data[track].get(0, StepParam::Sync));
        let cycle_beats = if initial_sync > f64::EPSILON {
            (accumulated / initial_sync).ceil() * initial_sync
        } else {
            accumulated
        }
        .max(f64::EPSILON);
        let position = beats.max(0.0) % cycle_beats;
        let idx = boundaries[..=num_steps].partition_point(|&boundary| boundary <= position);
        let step = idx.saturating_sub(1).min(num_steps - 1);
        (position < step_ends[step]).then(|| RecordPosition {
            step,
            phase: ((position - boundaries[step]) / (step_ends[step] - boundaries[step]))
                .clamp(0.0, 1.0) as f32,
        })
    }

    /// Interpolate the audio clock at a keyboard press and compensate the
    /// configured render-ahead latency before resolving a track-local phase.
    pub fn record_position_at_instant(
        &self,
        track: usize,
        timestamp: Instant,
    ) -> Option<RecordPosition> {
        let (anchor_beats, elapsed) = self.transport.record_clock.sample(timestamp)?;
        let bpm = self.transport.bpm.load(Ordering::Relaxed) as f64;
        let latency_seconds = f32::from_bits(
            self.transport
                .record_latency_seconds
                .load(Ordering::Relaxed),
        )
        .max(0.0) as f64;
        let beats =
            anchor_beats + elapsed.as_secs_f64() * bpm / 60.0 - latency_seconds * bpm / 60.0;
        self.record_position_at_beat(track, beats)
    }

    pub fn is_playing(&self) -> bool {
        self.transport.playing.load(Ordering::Relaxed)
    }

    pub fn start_playback(&self) {
        self.reset_playheads();
        self.transport.playing.store(true, Ordering::Relaxed);
        self.transport.pattern_epoch.fetch_add(1, Ordering::Relaxed);
        self.publish_scheduler_snapshot();
    }

    pub fn stop_playback(&self) {
        self.transport.playing.store(false, Ordering::Relaxed);
        self.reset_playheads();
        self.transport.pattern_epoch.fetch_add(1, Ordering::Relaxed);
        self.publish_scheduler_snapshot();
    }

    pub fn toggle_play(&self) -> bool {
        let playing = self.toggle_play_no_publish();
        self.publish_scheduler_snapshot();
        playing
    }

    pub(crate) fn toggle_play_no_publish(&self) -> bool {
        if self.is_playing() {
            self.transport.playing.store(false, Ordering::Relaxed);
            self.reset_playheads();
            self.transport.pattern_epoch.fetch_add(1, Ordering::Relaxed);
            false
        } else {
            self.reset_playheads();
            self.transport.playing.store(true, Ordering::Relaxed);
            self.transport.pattern_epoch.fetch_add(1, Ordering::Relaxed);
            true
        }
    }

    pub fn reset_playheads(&self) {
        self.transport.playhead.store(0, Ordering::Relaxed);
        self.transport
            .playhead_phase
            .store(0.0_f32.to_bits(), Ordering::Relaxed);
        for playhead in &self.transport.track_playheads {
            playhead.store(0, Ordering::Relaxed);
        }
        for phase in &self.transport.track_playhead_phases {
            phase.store(0.0_f32.to_bits(), Ordering::Relaxed);
        }
        for playhead in &self.transport.sampler_playheads {
            playhead.store(0.0_f32.to_bits(), Ordering::Relaxed);
        }
    }
    /// Publish a snapshot of all pattern/transport atomics so that future
    /// snapshot-based audio-thread readers can pick up the latest state.
    ///
    /// Currently this is a **no-op** because the audio thread reads atomics
    /// directly from `SequencerState`.  The method exists as a hook for the
    /// planned `Arc<SequencerSnapshot>` architecture — once that lands, this
    pub fn schedule_mod_resync(&self) {
        if self.is_playing() {
            self.transport
                .pending_mod_resync
                .store(true, Ordering::Relaxed);
        } else {
            self.transport
                .mod_reset_counter
                .fetch_add(1, Ordering::Relaxed);
        }
    }

    pub fn scene_track_pattern_id(&self, scene: usize, track: usize) -> Option<PatternId> {
        self.pattern
            .scenes
            .lock()
            .unwrap()
            .scenes
            .get(scene)?
            .cells
            .get(track)
            .copied()
            .flatten()
    }

    pub fn track_pattern_cells(&self, track: usize) -> Vec<TrackPatternCellView> {
        self.pattern
            .scenes
            .lock()
            .unwrap()
            .track_pattern_cells(track)
    }

    pub fn launch_scene(
        &self,
        scene_idx: usize,
        num_tracks: usize,
        buffer_ids: &[i32],
        sample_rates: &[u32],
        names: &[String],
        instrument_types: &[InstrumentType],
    ) -> Option<Vec<(i32, String, u32)>> {
        self.launch_scene_profiled(
            scene_idx,
            num_tracks,
            buffer_ids,
            sample_rates,
            names,
            instrument_types,
        )
        .map(|result| result.sample_ids)
    }

    pub fn launch_scene_profiled(
        &self,
        scene_idx: usize,
        num_tracks: usize,
        buffer_ids: &[i32],
        sample_rates: &[u32],
        names: &[String],
        instrument_types: &[InstrumentType],
    ) -> Option<PatternSwitchResult> {
        let total_started = Instant::now();
        let mut profile = PatternSwitchProfile::default();

        let started = Instant::now();
        let current_snapshot = self.capture_current_pattern_snapshot(
            num_tracks,
            buffer_ids,
            sample_rates,
            names,
            instrument_types,
        );
        profile.capture_current_snapshot = started.elapsed();

        let (sample_ids, snapshot_source) = {
            let started = Instant::now();
            let mut scenes = self.pattern.scenes.lock().unwrap();
            profile.scene_lock_wait = started.elapsed();

            let current_scene = self.current_scene_index();
            if scene_idx >= scenes.scene_count() {
                return None;
            }

            let started = Instant::now();
            scenes.save_scene_snapshot(current_scene, current_snapshot);
            profile.save_current_snapshot = started.elapsed();

            let started = Instant::now();
            let launched = scenes.launch_scene(scene_idx)?;
            profile.launch_scene_data = started.elapsed();

            let started = Instant::now();
            for (track, data) in launched.iter().enumerate() {
                if let Some(data) = data {
                    data.restore_to(self, track);
                    self.set_scene_silenced(track, false);
                } else {
                    self.set_scene_silenced(track, true);
                }
            }
            profile.restore_tracks = started.elapsed();

            let started = Instant::now();
            let sample_ids = scenes.scene_sample_ids(scene_idx).unwrap_or_default();
            profile.collect_sample_ids = started.elapsed();

            let started = Instant::now();
            self.pattern
                .current_pattern
                .store(scene_idx as u32, Ordering::Relaxed);
            self.pattern
                .num_patterns
                .store(scenes.scene_count() as u32, Ordering::Relaxed);
            self.transport.pattern_epoch.fetch_add(1, Ordering::Relaxed);
            profile.update_pattern_atoms = started.elapsed();

            let metadata = scenes.current_scene_metadata();
            let project_process_chain = scenes.current_project_process_chain();
            let snapshot_source = launched
                .into_iter()
                .collect::<Option<Vec<_>>>()
                .map(|tracks| {
                    (
                        tracks,
                        metadata.0,
                        metadata.1,
                        metadata.2,
                        project_process_chain,
                    )
                });

            (sample_ids, snapshot_source)
        };

        let started = Instant::now();
        self.schedule_mod_resync();
        profile.schedule_mod_resync = started.elapsed();

        let started = Instant::now();
        if let Some((
            tracks,
            mod_connections,
            neural_networks,
            graph_overrides,
            project_process_chain,
        )) = snapshot_source
        {
            self.publish_scheduler_snapshot_from_track_pattern_data(
                &tracks,
                mod_connections,
                neural_networks,
                graph_overrides,
                project_process_chain,
            );
        } else {
            self.publish_scheduler_snapshot();
        }
        profile.publish_scheduler_snapshot = started.elapsed();
        profile.total = total_started.elapsed();

        Some(PatternSwitchResult {
            sample_ids,
            profile,
        })
    }

    pub fn launch_track_pattern(
        &self,
        track: usize,
        pattern_id: PatternId,
        num_tracks: usize,
        buffer_ids: &[i32],
        sample_rates: &[u32],
        names: &[String],
        instrument_types: &[InstrumentType],
    ) -> bool {
        if track >= num_tracks {
            return false;
        }
        let current_snapshot = self.capture_current_pattern_snapshot(
            num_tracks,
            buffer_ids,
            sample_rates,
            names,
            instrument_types,
        );
        let launched = {
            let mut scenes = self.pattern.scenes.lock().unwrap();
            let current_scene = self.current_scene_index();
            if !scenes.save_scene_snapshot(current_scene, current_snapshot) {
                return false;
            }
            scenes.launch_track_pattern(track, pattern_id)
        };
        let Some(data) = launched else {
            return false;
        };
        data.restore_to(self, track);
        self.set_scene_silenced(track, false);
        self.transport.pattern_epoch.fetch_add(1, Ordering::Relaxed);
        self.publish_scheduler_snapshot();
        true
    }

    pub fn launch_scene_tracks(
        &self,
        scene: usize,
        tracks: &[usize],
        num_tracks: usize,
        buffer_ids: &[i32],
        sample_rates: &[u32],
        names: &[String],
        instrument_types: &[InstrumentType],
    ) -> bool {
        if tracks.is_empty() || tracks.iter().any(|track| *track >= num_tracks) {
            return false;
        }
        let current_snapshot = self.capture_current_pattern_snapshot(
            num_tracks,
            buffer_ids,
            sample_rates,
            names,
            instrument_types,
        );
        let launched = {
            let mut scenes = self.pattern.scenes.lock().unwrap();
            if scene >= scenes.scene_count() {
                return false;
            }
            // Validate the target before saving the current live state. Saving
            // is a mutation too, and a rejected launch must be side-effect free.
            if tracks.iter().any(|track| {
                scenes
                    .scenes
                    .get(scene)
                    .and_then(|scene| scene.cells.get(*track))
                    .copied()
                    .flatten()
                    .and_then(|id| scenes.track_pools.get(*track)?.get(id))
                    .is_none()
            }) {
                return false;
            }
            let current_scene = self.current_scene_index();
            if !scenes.save_scene_snapshot(current_scene, current_snapshot) {
                return false;
            }
            scenes.launch_scene_tracks(scene, tracks)
        };
        let Some(launched) = launched else {
            return false;
        };
        for (track, data) in launched {
            data.restore_to(self, track);
            self.set_scene_silenced(track, false);
        }
        self.transport.pattern_epoch.fetch_add(1, Ordering::Relaxed);
        self.publish_scheduler_snapshot();
        true
    }

    pub fn fork_current_track_pattern(
        &self,
        track: usize,
        num_tracks: usize,
        buffer_ids: &[i32],
        sample_rates: &[u32],
        names: &[String],
        instrument_types: &[InstrumentType],
    ) -> Option<PatternId> {
        if track >= num_tracks {
            return None;
        }
        let current_snapshot = self.capture_current_pattern_snapshot(
            num_tracks,
            buffer_ids,
            sample_rates,
            names,
            instrument_types,
        );
        let id = {
            let mut scenes = self.pattern.scenes.lock().unwrap();
            let current_scene = self.current_scene_index();
            scenes.save_scene_snapshot(current_scene, current_snapshot);
            scenes.fork_track_pattern(track)?
        };
        self.set_scene_silenced(track, false);
        self.transport.pattern_epoch.fetch_add(1, Ordering::Relaxed);
        self.publish_scheduler_snapshot();
        Some(id)
    }

    pub fn clone_current_scene_track_pattern(
        &self,
        track: usize,
        num_tracks: usize,
        buffer_ids: &[i32],
        sample_rates: &[u32],
        names: &[String],
        instrument_types: &[InstrumentType],
    ) -> Option<PatternId> {
        if track >= num_tracks {
            return None;
        }
        let current_snapshot = self.capture_current_pattern_snapshot(
            num_tracks,
            buffer_ids,
            sample_rates,
            names,
            instrument_types,
        );
        let (id, data) = {
            let mut scenes = self.pattern.scenes.lock().unwrap();
            let current_scene = self.current_scene_index();
            scenes.save_scene_snapshot(current_scene, current_snapshot);
            let id = scenes.clone_track_pattern_into_current_scene(track)?;
            let data = scenes.effective_track_pattern(track)?.clone();
            (id, data)
        };
        data.restore_to(self, track);
        self.set_scene_silenced(track, false);
        self.transport.pattern_epoch.fetch_add(1, Ordering::Relaxed);
        self.publish_scheduler_snapshot();
        Some(id)
    }

    pub fn clone_track_pattern_id_into_current_scene(
        &self,
        track: usize,
        source_id: PatternId,
        num_tracks: usize,
        buffer_ids: &[i32],
        sample_rates: &[u32],
        names: &[String],
        instrument_types: &[InstrumentType],
    ) -> Option<PatternId> {
        if track >= num_tracks {
            return None;
        }
        let current_snapshot = self.capture_current_pattern_snapshot(
            num_tracks,
            buffer_ids,
            sample_rates,
            names,
            instrument_types,
        );
        let (id, data) = {
            let mut scenes = self.pattern.scenes.lock().unwrap();
            let current_scene = self.current_scene_index();
            scenes.save_scene_snapshot(current_scene, current_snapshot);
            let id = scenes.clone_track_pattern_id_into_current_scene(track, source_id)?;
            let data = scenes.effective_track_pattern(track)?.clone();
            (id, data)
        };
        data.restore_to(self, track);
        self.set_scene_silenced(track, false);
        self.transport.pattern_epoch.fetch_add(1, Ordering::Relaxed);
        self.publish_scheduler_snapshot();
        Some(id)
    }

    pub fn delete_track_pattern(
        &self,
        track: usize,
        pattern_id: PatternId,
        num_tracks: usize,
        buffer_ids: &[i32],
        sample_rates: &[u32],
        names: &[String],
        instrument_types: &[InstrumentType],
    ) -> bool {
        if track >= num_tracks {
            return false;
        }
        let current_snapshot = self.capture_current_pattern_snapshot(
            num_tracks,
            buffer_ids,
            sample_rates,
            names,
            instrument_types,
        );
        let (was_effective, replacement) = {
            let mut scenes = self.pattern.scenes.lock().unwrap();
            let current_scene = self.current_scene_index();
            scenes.save_scene_snapshot(current_scene, current_snapshot);
            let was_effective = scenes.effective_pattern_id(track) == Some(pattern_id);
            if !scenes.delete_track_pattern(track, pattern_id) {
                return false;
            }
            let replacement = if was_effective {
                scenes.effective_track_pattern(track).cloned()
            } else {
                None
            };
            (was_effective, replacement)
        };

        if was_effective {
            if let Some(data) = replacement {
                data.restore_to(self, track);
                self.set_scene_silenced(track, false);
            } else {
                self.set_scene_silenced(track, true);
            }
        }
        self.transport.pattern_epoch.fetch_add(1, Ordering::Relaxed);
        self.publish_scheduler_snapshot();
        true
    }

    pub fn set_scene_cell(
        &self,
        scene: usize,
        track: usize,
        pattern_id: PatternId,
        num_tracks: usize,
        buffer_ids: &[i32],
        sample_rates: &[u32],
        names: &[String],
        instrument_types: &[InstrumentType],
    ) -> bool {
        if track >= num_tracks {
            return false;
        }
        let current_snapshot = self.capture_current_pattern_snapshot(
            num_tracks,
            buffer_ids,
            sample_rates,
            names,
            instrument_types,
        );
        let restore_current_track = {
            let mut scenes = self.pattern.scenes.lock().unwrap();
            let current_scene = self.current_scene_index();
            if !scenes.save_scene_snapshot(current_scene, current_snapshot) {
                return false;
            }
            if !scenes.set_cell(scene, track, pattern_id) {
                return false;
            }
            if scene == current_scene {
                if let Some(override_slot) = scenes.track_overrides.get_mut(track) {
                    *override_slot = None;
                }
                scenes
                    .track_pools
                    .get(track)
                    .and_then(|pool| pool.get(pattern_id))
                    .cloned()
            } else {
                None
            }
        };

        if let Some(data) = restore_current_track {
            data.restore_to(self, track);
            self.set_scene_silenced(track, false);
            self.transport.pattern_epoch.fetch_add(1, Ordering::Relaxed);
            self.publish_scheduler_snapshot();
        }
        true
    }

    pub fn clear_scene_cell(
        &self,
        scene: usize,
        track: usize,
        num_tracks: usize,
        buffer_ids: &[i32],
        sample_rates: &[u32],
        names: &[String],
        instrument_types: &[InstrumentType],
    ) -> Option<PatternId> {
        if track >= num_tracks {
            return None;
        }
        let current_snapshot = self.capture_current_pattern_snapshot(
            num_tracks,
            buffer_ids,
            sample_rates,
            names,
            instrument_types,
        );
        let (cleared, should_silence) = {
            let mut scenes = self.pattern.scenes.lock().unwrap();
            let current_scene = self.current_scene_index();
            if !scenes.save_scene_snapshot(current_scene, current_snapshot) {
                return None;
            }
            let cleared = scenes.clear_cell(scene, track)?;
            let should_silence =
                scene == current_scene && scenes.effective_pattern_id(track).is_none();
            (cleared, should_silence)
        };

        if should_silence {
            self.set_scene_silenced(track, true);
            self.transport.pattern_epoch.fetch_add(1, Ordering::Relaxed);
            self.publish_scheduler_snapshot();
        }
        Some(cleared)
    }

    pub fn switch_pattern(
        &self,
        new_idx: usize,
        num_tracks: usize,
        buffer_ids: &[i32],
        sample_rates: &[u32],
        names: &[String],
        instrument_types: &[InstrumentType],
    ) -> Option<Vec<(i32, String, u32)>> {
        self.switch_pattern_profiled(
            new_idx,
            num_tracks,
            buffer_ids,
            sample_rates,
            names,
            instrument_types,
        )
        .map(|result| result.sample_ids)
    }

    pub fn switch_pattern_profiled(
        &self,
        new_idx: usize,
        num_tracks: usize,
        buffer_ids: &[i32],
        sample_rates: &[u32],
        names: &[String],
        instrument_types: &[InstrumentType],
    ) -> Option<PatternSwitchResult> {
        let cur = self.current_scene_index();
        if new_idx == cur {
            return None;
        }
        self.launch_scene_profiled(
            new_idx,
            num_tracks,
            buffer_ids,
            sample_rates,
            names,
            instrument_types,
        )
    }

    pub fn clone_pattern(
        &self,
        num_tracks: usize,
        buffer_ids: &[i32],
        sample_rates: &[u32],
        names: &[String],
        instrument_types: &[InstrumentType],
    ) -> usize {
        let new_idx = {
            let mut scenes = self.pattern.scenes.lock().unwrap();
            let cur = self.current_scene_index();
            let current_metadata = scenes.current_scene_metadata();
            let current_snapshot = PatternSnapshot::capture_with_mod_connections(
                self,
                num_tracks,
                buffer_ids,
                sample_rates,
                names,
                instrument_types,
                current_metadata.0,
                current_metadata.1,
                current_metadata.2,
            );
            scenes.save_scene_snapshot(cur, current_snapshot);
            let new_idx = scenes.new_scene();
            self.pattern
                .current_pattern
                .store(new_idx as u32, Ordering::Relaxed);
            self.pattern
                .num_patterns
                .store(scenes.scene_count() as u32, Ordering::Relaxed);
            self.transport.pattern_epoch.fetch_add(1, Ordering::Relaxed);
            new_idx
        };
        self.publish_scheduler_snapshot();
        new_idx
    }

    /// Reorder scenes while keeping the currently playing scene active and
    /// leaving all per-track pattern pools untouched.
    pub fn reorder_scene(&self, source: usize, target: usize) -> Option<usize> {
        let _ = self.quantized_launches.cancel_all();
        let current_scene = {
            let mut scenes = self.pattern.scenes.lock().unwrap();
            scenes.reorder_scene(source, target)?
        };
        self.pattern
            .current_pattern
            .store(current_scene as u32, Ordering::Relaxed);
        Some(current_scene)
    }

    pub fn rename_scene(&self, scene: usize, name: String) -> bool {
        let name = name.trim();
        if name.is_empty() {
            return false;
        }
        let mut scenes = self.pattern.scenes.lock().unwrap();
        let Some(target) = scenes.scenes.get_mut(scene) else {
            return false;
        };
        if target.name == name {
            return false;
        }
        target.name = name.to_string();
        true
    }

    pub fn delete_pattern(
        &self,
        num_tracks: usize,
        buffer_ids: &[i32],
        sample_rates: &[u32],
        names: &[String],
        instrument_types: &[InstrumentType],
    ) -> Option<Vec<(i32, String, u32)>> {
        let _ = self.quantized_launches.cancel_all();
        let sample_ids = {
            let mut scenes = self.pattern.scenes.lock().unwrap();
            if scenes.scene_count() <= 1 {
                return None;
            }
            let cur = self.current_scene_index();
            let current_metadata = scenes.current_scene_metadata();
            let current_snapshot = PatternSnapshot::capture_with_mod_connections(
                self,
                num_tracks,
                buffer_ids,
                sample_rates,
                names,
                instrument_types,
                current_metadata.0,
                current_metadata.1,
                current_metadata.2,
            );
            scenes.save_scene_snapshot(cur, current_snapshot);
            let new_idx = scenes.delete_scene(cur)?;
            let launched = scenes.launch_scene(new_idx)?;
            for (track, data) in launched.into_iter().enumerate() {
                if let Some(data) = data {
                    data.restore_to(self, track);
                    self.set_scene_silenced(track, false);
                } else {
                    self.set_scene_silenced(track, true);
                }
            }
            let sample_ids = scenes
                .scene_snapshot(new_idx)
                .map(|snapshot| snapshot.sample_ids)
                .unwrap_or_default();
            self.pattern
                .current_pattern
                .store(new_idx as u32, Ordering::Relaxed);
            self.pattern
                .num_patterns
                .store(scenes.scene_count() as u32, Ordering::Relaxed);
            self.transport.pattern_epoch.fetch_add(1, Ordering::Relaxed);
            sample_ids
        };
        self.schedule_mod_resync();
        self.publish_scheduler_snapshot();
        Some(sample_ids)
    }

    pub fn propagate_track_to_all_patterns(
        &self,
        track: usize,
        num_tracks: usize,
        buffer_ids: &[i32],
        sample_rates: &[u32],
        names: &[String],
        instrument_types: &[InstrumentType],
    ) -> bool {
        let mut scenes = self.pattern.scenes.lock().unwrap();
        let cur = self.current_scene_index();
        if cur >= scenes.scene_count() || track >= num_tracks {
            return false;
        }
        let current_metadata = scenes.current_scene_metadata();
        let current_snapshot = PatternSnapshot::capture_with_mod_connections(
            self,
            num_tracks,
            buffer_ids,
            sample_rates,
            names,
            instrument_types,
            current_metadata.0,
            current_metadata.1,
            current_metadata.2,
        );
        scenes.save_scene_snapshot(cur, current_snapshot);
        let Some(source) = scenes.scene_snapshot(cur) else {
            return false;
        };
        let mut snapshots = scenes.snapshots();
        for (pattern_idx, snapshot) in snapshots.iter_mut().enumerate() {
            if pattern_idx != cur {
                snapshot.clone_track_lane_from(&source, track);
            }
        }
        let bus_patterns = scenes
            .scenes
            .iter()
            .map(|scene| scene.bus_patterns.clone())
            .collect::<Vec<_>>();
        let mut rebuilt = ProjectScenes::from_pattern_snapshots(&snapshots, cur);
        for (scene, bus_patterns) in rebuilt.scenes.iter_mut().zip(bus_patterns) {
            scene.bus_patterns = bus_patterns;
        }
        *scenes = rebuilt;
        true
    }

    pub(crate) fn toggle_step_and_clear_plocks_no_publish(&self, track: usize, step: usize) {
        let was_active = self.pattern.patterns[track].is_active(step);
        if was_active {
            let params: [f32; NUM_PARAMS] = std::array::from_fn(|param_idx| {
                self.pattern.step_data[track].get(step, StepParam::ALL[param_idx])
            });
            self.clear_step_payload_inner(track, step);
            for param in StepParam::ALL {
                self.pattern.step_data[track].set(step, param, params[param.index()]);
            }
        } else {
            self.pattern.patterns[track].set_step_active(step, true);
        }
    }

    pub fn toggle_step_and_clear_plocks(&self, track: usize, step: usize) {
        self.toggle_step_and_clear_plocks_no_publish(track, step);
        self.publish_scheduler_snapshot();
    }

    fn drum_lane_notes(&self, track: usize, step: usize) -> Vec<(f32, f32, f32)> {
        if track >= MAX_TRACKS || step >= MAX_STEPS || !self.pattern.patterns[track].is_active(step)
        {
            return Vec::new();
        }

        let step_duration = self.pattern.step_data[track]
            .get(step, StepParam::Duration)
            .max(0.0);
        let step_delay = self.pattern.step_data[track].get(step, StepParam::Delay);
        let chord_count = self.pattern.chord_data[track].count(step);
        if chord_count == 0 {
            return vec![(
                self.pattern.step_data[track].get(step, StepParam::Transpose),
                step_duration,
                step_delay,
            )];
        }

        (0..chord_count)
            .map(|voice| {
                let duration = self.pattern.chord_data[track].get_duration(step, voice);
                (
                    self.pattern.chord_data[track].get(step, voice),
                    if duration > 0.0 {
                        duration
                    } else {
                        step_duration
                    },
                    self.pattern.chord_data[track].get_delay(step, voice),
                )
            })
            .collect()
    }

    fn write_drum_lane_notes(&self, track: usize, step: usize, mut notes: Vec<(f32, f32, f32)>) {
        if notes.is_empty() {
            self.clear_step_payload_inner(track, step);
            return;
        }

        notes.sort_by(|left, right| {
            left.0
                .partial_cmp(&right.0)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        self.pattern.chord_data[track].clear_step(step);
        let max_duration = notes
            .iter()
            .map(|(_, duration, _)| *duration)
            .fold(0.0, f32::max);
        if notes.len() > 1 {
            for (note, duration, delay) in &notes {
                self.pattern.chord_data[track].add_note_with_timing(step, *note, *duration, *delay);
            }
        }
        self.pattern.step_data[track].set(step, StepParam::Transpose, notes[0].0);
        self.pattern.step_data[track].set(step, StepParam::Duration, max_duration);
        self.pattern.step_data[track].set(
            step,
            StepParam::Delay,
            if notes.len() == 1 { notes[0].2 } else { 0.0 },
        );
        self.pattern.patterns[track].set_step_active(step, true);
    }

    pub fn drum_lane_step_duration(&self, track: usize, step: usize, pad_note: i32) -> Option<f32> {
        self.drum_lane_notes(track, step)
            .into_iter()
            .find(|(note, _, _)| note.round() as i32 == pad_note)
            .map(|(_, duration, _)| duration)
    }

    /// Set the duration of one drum-pad voice without changing the durations
    /// of simultaneous hits stored in the same polyphonic step.
    pub fn set_drum_lane_step_duration(
        &self,
        track: usize,
        step: usize,
        pad_note: i32,
        duration: f32,
    ) -> Option<f32> {
        let duration = self.set_drum_lane_step_duration_no_publish(
            track,
            step,
            pad_note,
            duration,
        )?;
        self.publish_scheduler_snapshot();
        Some(duration)
    }

    pub fn set_drum_lane_step_duration_no_publish(
        &self,
        track: usize,
        step: usize,
        pad_note: i32,
        duration: f32,
    ) -> Option<f32> {
        if track >= MAX_TRACKS || step >= MAX_STEPS {
            return None;
        }
        let duration = duration.clamp(StepParam::Duration.min(), StepParam::Duration.max());
        let mut notes = self.drum_lane_notes(track, step);
        let (_, note_duration, _) = notes
            .iter_mut()
            .find(|(note, _, _)| note.round() as i32 == pad_note)?;
        *note_duration = duration;
        self.write_drum_lane_notes(track, step, notes);
        Some(duration)
    }

    /// Toggle one pitch lane within a polyphonic step. Drum-rack lanes are a
    /// projection of the existing step/chord representation: a single hit is
    /// stored in the step transpose field, while simultaneous hits are stored
    /// in chord data. Removing the final lane clears the complete step payload,
    /// matching the normal step-toggle behavior.
    pub fn toggle_drum_lane_step(&self, track: usize, step: usize, pad_note: i32) -> bool {
        let activated = self.toggle_drum_lane_step_no_publish(track, step, pad_note);
        self.publish_scheduler_snapshot();
        activated
    }

    pub fn toggle_drum_lane_step_no_publish(
        &self,
        track: usize,
        step: usize,
        pad_note: i32,
    ) -> bool {
        if track >= MAX_TRACKS || step >= MAX_STEPS {
            return false;
        }

        let transpose = pad_note as f32;
        let step_duration = self.pattern.step_data[track]
            .get(step, StepParam::Duration)
            .max(0.0);
        let step_delay = self.pattern.step_data[track].get(step, StepParam::Delay);
        let mut notes = self.drum_lane_notes(track, step);

        let existing = notes
            .iter()
            .position(|(note, _, _)| note.round() as i32 == pad_note);
        let activated = if let Some(index) = existing {
            notes.remove(index);
            false
        } else if notes.len() < MAX_VOICES {
            notes.push((transpose, step_duration, step_delay));
            true
        } else {
            return false;
        };

        self.write_drum_lane_notes(track, step, notes);
        activated
    }

    /// Move one or more hits in a single drum-pad lane without disturbing
    /// simultaneous hits belonging to other pads. Destination hits in this
    /// lane are replaced, matching the overwrite behavior of normal step drag.
    pub fn move_drum_lane_steps(
        &self,
        track: usize,
        pad_note: i32,
        steps: &[usize],
        delta: isize,
    ) -> bool {
        let moved = self.move_drum_lane_steps_no_publish(track, pad_note, steps, delta);
        if moved {
            self.publish_scheduler_snapshot();
        }
        moved
    }

    pub fn move_drum_lane_steps_no_publish(
        &self,
        track: usize,
        pad_note: i32,
        steps: &[usize],
        delta: isize,
    ) -> bool {
        if track >= MAX_TRACKS || delta == 0 || steps.is_empty() {
            return false;
        }
        let mut sources = steps.to_vec();
        sources.sort_unstable();
        sources.dedup();
        if sources.iter().any(|step| *step >= MAX_STEPS) {
            return false;
        }
        let destinations = sources
            .iter()
            .map(|step| *step as isize + delta)
            .collect::<Vec<_>>();
        if destinations
            .iter()
            .any(|step| *step < 0 || *step >= MAX_STEPS as isize)
        {
            return false;
        }

        let moved = sources
            .iter()
            .filter_map(|step| {
                let notes = self.drum_lane_notes(track, *step);
                notes
                    .iter()
                    .find(|(note, _, _)| note.round() as i32 == pad_note)
                    .copied()
                    .map(|note| {
                        (
                            *step,
                            note,
                            (notes.len() == 1).then(|| self.capture_step_snapshot(track, *step)),
                        )
                    })
            })
            .collect::<Vec<_>>();
        if moved.is_empty() {
            return false;
        }

        for (step, _, _) in &moved {
            let notes = self
                .drum_lane_notes(track, *step)
                .into_iter()
                .filter(|(note, _, _)| note.round() as i32 != pad_note)
                .collect();
            self.write_drum_lane_notes(track, *step, notes);
        }
        for (step, note, exclusive_snapshot) in moved {
            let destination = (step as isize + delta) as usize;
            let mut notes = self
                .drum_lane_notes(track, destination)
                .into_iter()
                .filter(|(existing, _, _)| existing.round() as i32 != pad_note)
                .collect::<Vec<_>>();
            if notes.is_empty() {
                if let Some(snapshot) = exclusive_snapshot {
                    self.restore_step_snapshot_inner(track, destination, &snapshot);
                    continue;
                }
            }
            notes.push(note);
            self.write_drum_lane_notes(track, destination, notes);
        }
        true
    }

    /// Clear selected hits from one drum-pad lane while retaining every other
    /// pad hit and the shared payload of steps that remain active.
    pub fn clear_drum_lane_steps(&self, track: usize, pad_note: i32, steps: &[usize]) -> usize {
        let cleared = self.clear_drum_lane_steps_no_publish(track, pad_note, steps);
        if cleared > 0 {
            self.publish_scheduler_snapshot();
        }
        cleared
    }

    pub fn clear_drum_lane_steps_no_publish(
        &self,
        track: usize,
        pad_note: i32,
        steps: &[usize],
    ) -> usize {
        if track >= MAX_TRACKS {
            return 0;
        }
        let mut cleared = 0;
        for step in steps.iter().copied().filter(|step| *step < MAX_STEPS) {
            let notes = self.drum_lane_notes(track, step);
            let retained = notes
                .iter()
                .copied()
                .filter(|(note, _, _)| note.round() as i32 != pad_note)
                .collect::<Vec<_>>();
            if retained.len() != notes.len() {
                self.write_drum_lane_notes(track, step, retained);
                cleared += 1;
            }
        }
        cleared
    }

    pub fn capture_step_snapshot(&self, track: usize, step: usize) -> StepSnapshot {
        let mut params = [0.0; NUM_PARAMS];
        for param in StepParam::ALL {
            params[param.index()] = self.pattern.step_data[track].get(step, param);
        }

        let chord_count = self.pattern.chord_data[track].count(step);
        let mut chord = Vec::with_capacity(chord_count);
        let mut chord_durations = Vec::with_capacity(chord_count);
        let mut chord_delays = Vec::with_capacity(chord_count);
        for note_idx in 0..chord_count {
            chord.push(self.pattern.chord_data[track].get(step, note_idx));
            chord_durations.push(self.pattern.chord_data[track].get_duration(step, note_idx));
            chord_delays.push(self.pattern.chord_data[track].get_delay(step, note_idx));
        }

        let midi_fx_plocks = self.pattern.midi_fx_slots[track]
            .iter()
            .map(|slot| capture_live_slot_step_plocks(slot, step))
            .collect();
        let effect_plocks = self.pattern.effect_chains[track]
            .iter()
            .map(|slot| capture_live_slot_step_plocks(slot, step))
            .collect();
        let instrument_plocks =
            capture_live_slot_step_plocks(&self.pattern.instrument_slots[track], step);
        let (
            rack_macro_plocks,
            rack_slot_param_plocks,
            rack_slot_instrument_plocks,
            rack_slot_effect_plocks,
        ) = self
            .pattern
            .rack_tracks
            .lock()
            .unwrap()
            .get(track)
            .and_then(|rack| rack.as_ref())
            .map(|rack| {
                let macro_plocks = rack
                    .macros
                    .iter()
                    .map(|rack_macro| rack_macro.plocks.get(step).copied().flatten())
                    .collect();
                let slot_params = rack
                    .slots
                    .iter()
                    .map(|slot| {
                        let params = RackSlotParam::ALL
                            .iter()
                            .map(|param| slot.param_plocks.get(step, *param))
                            .collect();
                        StepSlotPlocks {
                            params,
                            tensor_params: Vec::new(),
                        }
                    })
                    .collect();
                let instrument_params = rack
                    .slots
                    .iter()
                    .map(|slot| capture_snapshot_slot_step_plocks(&slot.instrument_slot, step))
                    .collect();
                let effect_params = rack
                    .slots
                    .iter()
                    .map(|slot| {
                        slot.effect_slots
                            .iter()
                            .map(|effect| capture_snapshot_slot_step_plocks(effect, step))
                            .collect()
                    })
                    .collect();
                (macro_plocks, slot_params, instrument_params, effect_params)
            })
            .unwrap_or_default();

        StepSnapshot {
            active: self.pattern.patterns[track].is_active(step),
            neural_reset: self.pattern.neural_reset_patterns[track].is_active(step),
            params,
            chord,
            chord_durations,
            chord_delays,
            timebase: self.pattern.timebase_plocks[track].get(step),
            swing: self.pattern.swing_plocks[track].get(step),
            swing_resolution: self.pattern.swing_resolution_plocks[track].get(step),
            midi_fx_plocks,
            effect_plocks,
            instrument_plocks,
            rack_macro_plocks,
            rack_slot_param_plocks,
            rack_slot_instrument_plocks,
            rack_slot_effect_plocks,
        }
    }

    /// Capture step cells from one stable Track Pattern target.
    ///
    /// The live lanes are authoritative only when `pattern_id` is currently
    /// effective. Inactive targets are read directly from their pattern pool.
    pub(crate) fn capture_pattern_step_cells(
        &self,
        track: usize,
        pattern_id: PatternId,
        steps: &[usize],
    ) -> Result<(Vec<StepSnapshot>, PlockVariantRegistry), String> {
        if steps.iter().any(|step| *step >= MAX_STEPS) {
            return Err("step target is out of range".to_string());
        }
        let is_effective = {
            let scenes = self.pattern.scenes.lock().unwrap();
            let is_effective = scenes.effective_pattern_id(track) == Some(pattern_id);
            if !is_effective {
                let data = scenes
                    .track_pools
                    .get(track)
                    .and_then(|pool| pool.get(pattern_id))
                    .ok_or_else(|| "Track Pattern target no longer exists".to_string())?;
                let cells = steps
                    .iter()
                    .map(|step| {
                        data.capture_step_snapshot(*step)
                            .ok_or_else(|| "stored step target is out of range".to_string())
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                return Ok((cells, data.plock_variant_registry.clone()));
            }
            is_effective
        };

        if is_effective {
            if track >= self.pattern.patterns.len() {
                return Err("live track target no longer exists".to_string());
            }
            let cells = steps
                .iter()
                .map(|step| self.capture_step_snapshot(track, *step))
                .collect();
            let registry = self
                .pattern
                .plock_variant_registries
                .lock()
                .unwrap()
                .get(track)
                .cloned()
                .ok_or_else(|| "live p-lock variant registry is missing".to_string())?;
            Ok((cells, registry))
        } else {
            unreachable!("inactive Track Pattern capture returned while holding repository")
        }
    }

    pub(crate) fn capture_pattern_num_steps(
        &self,
        track: usize,
        pattern_id: PatternId,
    ) -> Result<usize, String> {
        let scenes = self.pattern.scenes.lock().unwrap();
        if scenes.effective_pattern_id(track) == Some(pattern_id) {
            return self
                .pattern
                .track_params
                .get(track)
                .map(TrackParams::get_num_steps)
                .ok_or_else(|| "live track target no longer exists".to_string());
        }
        scenes
            .track_pools
            .get(track)
            .and_then(|pool| pool.get(pattern_id))
            .map(|data| data.track_params.num_steps)
            .ok_or_else(|| "Track Pattern target no longer exists".to_string())
    }

    pub(crate) fn capture_pattern_track_params(
        &self,
        track: usize,
        pattern_id: PatternId,
    ) -> Result<TrackParamsSnapshot, String> {
        let scenes = self.pattern.scenes.lock().unwrap();
        if scenes.effective_pattern_id(track) == Some(pattern_id) {
            return self
                .pattern
                .track_params
                .get(track)
                .map(capture_track_params_snapshot)
                .ok_or_else(|| "live track target no longer exists".to_string());
        }
        scenes
            .track_pools
            .get(track)
            .and_then(|pool| pool.get(pattern_id))
            .map(|data| data.track_params.clone())
            .ok_or_else(|| "Track Pattern target no longer exists".to_string())
    }

    pub(crate) fn restore_pattern_track_params_no_publish(
        &self,
        track: usize,
        pattern_id: PatternId,
        snapshot: &TrackParamsSnapshot,
    ) -> Result<bool, String> {
        let mut scenes = self.pattern.scenes.lock().unwrap();
        let is_effective = scenes.effective_pattern_id(track) == Some(pattern_id);
        let data = scenes
            .track_pools
            .get_mut(track)
            .and_then(|pool| pool.get_mut(pattern_id))
            .ok_or_else(|| "Track Pattern target no longer exists".to_string())?;
        data.track_params = snapshot.clone();
        if is_effective {
            let live = self
                .pattern
                .track_params
                .get(track)
                .ok_or_else(|| "live track target no longer exists".to_string())?;
            restore_track_params_snapshot(live, snapshot);
        }
        Ok(is_effective)
    }

    pub(crate) fn capture_pattern_instrument_base_note_offset(
        &self,
        track: usize,
        pattern_id: PatternId,
    ) -> Result<f32, String> {
        let scenes = self.pattern.scenes.lock().unwrap();
        if scenes.effective_pattern_id(track) == Some(pattern_id) {
            return self
                .pattern
                .instrument_base_note_offsets
                .get(track)
                .map(|value| f32::from_bits(value.load(Ordering::Relaxed)))
                .ok_or_else(|| "live track target no longer exists".to_string());
        }
        scenes
            .track_pools
            .get(track)
            .and_then(|pool| pool.get(pattern_id))
            .map(|data| data.instrument_base_note_offset)
            .ok_or_else(|| "Track Pattern target no longer exists".to_string())
    }

    pub(crate) fn restore_pattern_instrument_base_note_offset_no_publish(
        &self,
        track: usize,
        pattern_id: PatternId,
        value: f32,
    ) -> Result<bool, String> {
        let mut scenes = self.pattern.scenes.lock().unwrap();
        let is_effective = scenes.effective_pattern_id(track) == Some(pattern_id);
        let data = scenes
            .track_pools
            .get_mut(track)
            .and_then(|pool| pool.get_mut(pattern_id))
            .ok_or_else(|| "Track Pattern target no longer exists".to_string())?;
        data.instrument_base_note_offset = value;
        if is_effective {
            self.pattern
                .instrument_base_note_offsets
                .get(track)
                .ok_or_else(|| "live track target no longer exists".to_string())?
                .store(value.to_bits(), Ordering::Relaxed);
        }
        Ok(is_effective)
    }

    pub(crate) fn capture_pattern_instrument_device_values(
        &self,
        track: usize,
        pattern_id: PatternId,
    ) -> Result<InstrumentDeviceValuesSnapshot, String> {
        let scenes = self.pattern.scenes.lock().unwrap();
        if scenes.effective_pattern_id(track) == Some(pattern_id) {
            let slot = self
                .pattern
                .instrument_slots
                .get(track)
                .ok_or_else(|| "live instrument target no longer exists".to_string())?;
            let base_note_offset_bits = self
                .pattern
                .instrument_base_note_offsets
                .get(track)
                .ok_or_else(|| "live instrument base note is missing".to_string())?
                .load(Ordering::Relaxed);
            let sound_state = self
                .pattern
                .track_sound_state
                .lock()
                .unwrap()
                .get(track)
                .cloned()
                .ok_or_else(|| "live instrument sound state is missing".to_string())?;
            let key_lock_variant_registry = self
                .pattern
                .key_lock_variant_registries
                .lock()
                .unwrap()
                .get(track)
                .cloned()
                .ok_or_else(|| "live key-lock variant registry is missing".to_string())?;
            return Ok(InstrumentDeviceValuesSnapshot {
                slot: EffectSlotSnapshot::capture_authoring_values(slot),
                base_note_offset_bits,
                sound_state,
                key_lock_variant_registry,
            });
        }
        let data = scenes
            .track_pools
            .get(track)
            .and_then(|pool| pool.get(pattern_id))
            .ok_or_else(|| "Track Pattern target no longer exists".to_string())?;
        Ok(InstrumentDeviceValuesSnapshot {
            slot: data.instrument_slot.authoring_values(),
            base_note_offset_bits: data.instrument_base_note_offset.to_bits(),
            sound_state: data.track_sound_state.clone(),
            key_lock_variant_registry: data.key_lock_variant_registry.clone(),
        })
    }

    pub(crate) fn restore_pattern_instrument_device_values_no_publish(
        &self,
        track: usize,
        pattern_id: PatternId,
        values: &InstrumentDeviceValuesSnapshot,
    ) -> Result<bool, String> {
        let mut scenes = self.pattern.scenes.lock().unwrap();
        let is_effective = scenes.effective_pattern_id(track) == Some(pattern_id);
        let data = scenes
            .track_pools
            .get_mut(track)
            .and_then(|pool| pool.get_mut(pattern_id))
            .ok_or_else(|| "Track Pattern target no longer exists".to_string())?;
        let mut stored_slot = data.instrument_slot.clone();
        if let Err(error) = stored_slot.apply_authoring_values(&values.slot) {
            if !is_effective {
                return Err(error);
            }
            // The pool copy can predate the current instrument descriptor
            // (older saved project, e.g. the sampler grew params). The values
            // being applied were captured from the live slot, so reseed the
            // stored layout from it instead of failing the edit forever.
            let slot = self
                .pattern
                .instrument_slots
                .get(track)
                .ok_or_else(|| "live instrument target no longer exists".to_string())?;
            stored_slot = EffectSlotSnapshot::capture(slot);
            stored_slot.apply_authoring_values(&values.slot)?;
        }
        let live_slot = if is_effective {
            let slot = self
                .pattern
                .instrument_slots
                .get(track)
                .ok_or_else(|| "live instrument target no longer exists".to_string())?;
            let mut snapshot = EffectSlotSnapshot::capture(slot);
            snapshot.apply_authoring_values(&values.slot)?;
            Some((slot, snapshot))
        } else {
            None
        };

        data.instrument_slot = stored_slot;
        data.instrument_base_note_offset = f32::from_bits(values.base_note_offset_bits);
        data.track_sound_state = values.sound_state.clone();
        data.key_lock_variant_registry = values.key_lock_variant_registry.clone();
        if let Some((slot, snapshot)) = live_slot {
            snapshot.restore(slot);
            self.pattern
                .instrument_base_note_offsets
                .get(track)
                .ok_or_else(|| "live instrument base note is missing".to_string())?
                .store(values.base_note_offset_bits, Ordering::Relaxed);
            *self
                .pattern
                .track_sound_state
                .lock()
                .unwrap()
                .get_mut(track)
                .ok_or_else(|| "live instrument sound state is missing".to_string())? =
                values.sound_state.clone();
            *self
                .pattern
                .key_lock_variant_registries
                .lock()
                .unwrap()
                .get_mut(track)
                .ok_or_else(|| "live key-lock variant registry is missing".to_string())? =
                values.key_lock_variant_registry.clone();
        }
        Ok(is_effective)
    }

    pub(crate) fn capture_pattern_effect_device_values(
        &self,
        track: usize,
        pattern_id: PatternId,
        slot_idx: usize,
    ) -> Result<EffectSlotValuesSnapshot, String> {
        let scenes = self.pattern.scenes.lock().unwrap();
        if scenes.effective_pattern_id(track) == Some(pattern_id) {
            return self
                .pattern
                .effect_chains
                .get(track)
                .and_then(|chain| chain.get(slot_idx))
                .map(EffectSlotSnapshot::capture_authoring_values)
                .ok_or_else(|| "live effect target no longer exists".to_string());
        }
        scenes
            .track_pools
            .get(track)
            .and_then(|pool| pool.get(pattern_id))
            .and_then(|data| data.effect_slots.get(slot_idx))
            .map(EffectSlotSnapshot::authoring_values)
            .ok_or_else(|| "Track Pattern effect target no longer exists".to_string())
    }

    pub(crate) fn capture_track_effect_chain_values(
        &self,
        track: usize,
        first_slot: usize,
        slot_count: usize,
    ) -> Result<Vec<(PatternId, Vec<EffectSlotValuesSnapshot>)>, String> {
        let scenes = self.pattern.scenes.lock().unwrap();
        let pool = scenes
            .track_pools
            .get(track)
            .ok_or_else(|| format!("Track {} has no pattern pool", track + 1))?;
        let effective = scenes
            .effective_pattern_id(track)
            .ok_or_else(|| format!("Track {} has no effective pattern", track + 1))?;
        pool.patterns
            .iter()
            .map(|(pattern, data)| {
                let slots = if *pattern == effective {
                    self.pattern
                        .effect_chains
                        .get(track)
                        .ok_or_else(|| "live effect chain is missing".to_string())?
                        .iter()
                        .skip(first_slot)
                        .take(slot_count)
                        .map(EffectSlotSnapshot::capture_authoring_values)
                        .collect()
                } else {
                    data.effect_slots
                        .iter()
                        .skip(first_slot)
                        .take(slot_count)
                        .map(EffectSlotSnapshot::authoring_values)
                        .collect()
                };
                Ok((*pattern, slots))
            })
            .collect()
    }

    pub(crate) fn restore_track_effect_chain_values(
        &self,
        track: usize,
        first_slot: usize,
        descriptors: &[EffectDescriptor],
        node_ids: &[(u32, u32)],
        patterns: &[(PatternId, Vec<EffectSlotValuesSnapshot>)],
    ) -> Result<(), String> {
        if descriptors.len() != node_ids.len() {
            return Err("effect-chain descriptor/node layout mismatch".to_string());
        }
        let mut scenes = self.pattern.scenes.lock().unwrap();
        let effective = scenes
            .effective_pattern_id(track)
            .ok_or_else(|| format!("Track {} has no effective pattern", track + 1))?;
        let pool = scenes
            .track_pools
            .get_mut(track)
            .ok_or_else(|| format!("Track {} has no pattern pool", track + 1))?;
        if pool.patterns.len() != patterns.len()
            || patterns.iter().any(|(id, _)| !pool.patterns.contains_key(id))
        {
            return Err(format!(
                "Track {} pattern set changed before effect history replay",
                track + 1
            ));
        }
        for (pattern, values) in patterns {
            if values.len() != descriptors.len() {
                return Err("effect-chain pattern layout mismatch".to_string());
            }
            let data = pool.patterns.get_mut(pattern).expect("pattern set was validated");
            for (offset, ((descriptor, (node_id, modulator_node_id)), values)) in descriptors
                .iter()
                .zip(node_ids)
                .zip(values)
                .enumerate()
            {
                let slot = data
                    .effect_slots
                    .get_mut(first_slot + offset)
                    .ok_or_else(|| "stored effect slot is missing".to_string())?;
                slot.sync_to_descriptor_with_modulator(
                    descriptor,
                    *node_id,
                    *modulator_node_id,
                );
                slot.apply_authoring_values(values)?;
            }
        }
        let live_values = patterns
            .iter()
            .find(|(pattern, _)| *pattern == effective)
            .map(|(_, values)| values)
            .ok_or_else(|| "effective effect pattern is missing from history".to_string())?;
        let live_chain = self
            .pattern
            .effect_chains
            .get(track)
            .ok_or_else(|| "live effect chain is missing".to_string())?;
        for (offset, ((descriptor, (node_id, modulator_node_id)), values)) in descriptors
            .iter()
            .zip(node_ids)
            .zip(live_values)
            .enumerate()
        {
            let slot = live_chain
                .get(first_slot + offset)
                .ok_or_else(|| "live effect slot is missing".to_string())?;
            let mut snapshot = EffectSlotSnapshot::capture(slot);
            snapshot.sync_to_descriptor_with_modulator(
                descriptor,
                *node_id,
                *modulator_node_id,
            );
            snapshot.apply_authoring_values(values)?;
            snapshot.restore(slot);
        }
        Ok(())
    }

    pub(crate) fn capture_track_effect_binding_state(
        &self,
        track: usize,
    ) -> Result<TrackEffectBindingStateSnapshot, String> {
        let scenes = self.pattern.scenes.lock().unwrap();
        let pool = scenes
            .track_pools
            .get(track)
            .ok_or_else(|| format!("Track {} has no pattern pool", track + 1))?;
        let effective = scenes
            .effective_pattern_id(track)
            .ok_or_else(|| format!("Track {} has no effective pattern", track + 1))?;
        let live_chain = self
            .pattern
            .process_chains
            .lock()
            .unwrap()
            .get(track)
            .cloned()
            .ok_or_else(|| "live process chain is missing".to_string())?;
        let live_lane_overrides = self
            .pattern
            .project_process_lane_overrides
            .lock()
            .unwrap()
            .get(track)
            .cloned()
            .ok_or_else(|| "live project process lane overrides are missing".to_string())?;
        let process_chains = pool
            .patterns
            .iter()
            .map(|(id, data)| {
                (*id, if *id == effective { live_chain.clone() } else { data.process_chain.clone() })
            })
            .collect();
        let project_process_lane_overrides = pool
            .patterns
            .iter()
            .map(|(id, data)| {
                (
                    *id,
                    if *id == effective {
                        live_lane_overrides.clone()
                    } else {
                        data.project_process_lane_overrides.clone()
                    },
                )
            })
            .collect();
        let mut neural_overrides = Vec::new();
        for (scene_idx, scene) in scenes.scenes.iter().enumerate() {
            for (network_idx, network) in scene.neural_networks.iter().enumerate() {
                for (neuron_idx, neuron) in network.neurons.iter().enumerate() {
                    let entries = neuron
                        .output_overrides
                        .effects
                        .iter()
                        .enumerate()
                        .filter(|(_, value)| value.target_track == track)
                        .map(|(index, value)| (index, value.clone()))
                        .collect::<Vec<_>>();
                    if !entries.is_empty() {
                        neural_overrides.push(NeuralEffectOverrideState {
                            scene: scene_idx,
                            network: network_idx,
                            neuron: neuron_idx,
                            entries,
                        });
                    }
                }
            }
        }
        Ok(TrackEffectBindingStateSnapshot {
            process_chains,
            project_process_lane_overrides,
            neural_overrides,
        })
    }

    pub(crate) fn remap_track_effect_references(
        &self,
        track: usize,
        old_to_new: &[Option<usize>],
        drop_neural_slots: &[bool],
        effect_descriptors: &[EffectDescriptor],
    ) -> Result<(), String> {
        fn remap_chain(
            chain: &mut crate::process::TrackProcessChain,
            old_to_new: &[Option<usize>],
        ) {
            for process_slot in &mut chain.slots {
                for binding in process_slot.bindings.values_mut() {
                    let Some(crate::process::ParamTarget::EffectParam { slot, .. }) = binding.as_mut() else {
                        continue;
                    };
                    match old_to_new.get(*slot).copied().flatten() {
                        Some(new_slot) => *slot = new_slot,
                        None => *binding = None,
                    }
                }
            }
        }

        let live_effect_slots = self
            .pattern
            .effect_chains
            .get(track)
            .ok_or_else(|| "live effect chain is missing".to_string())?;
        let mut scenes = self.pattern.scenes.lock().unwrap();
        let pool = scenes
            .track_pools
            .get_mut(track)
            .ok_or_else(|| format!("Track {} has no pattern pool", track + 1))?;
        for data in pool.patterns.values_mut() {
            remap_chain(&mut data.process_chain, old_to_new);
            crate::process::rebind_track_process_chain_effect_param_ids(
                &mut data.process_chain,
                effect_descriptors,
                &data.effect_slots,
            );
        }
        for scene in &mut scenes.scenes {
            for network in &mut scene.neural_networks {
                for neuron in &mut network.neurons {
                    neuron.output_overrides.effects.retain_mut(|value| {
                        if value.target_track != track {
                            return true;
                        }
                        if drop_neural_slots
                            .get(value.slot_index)
                            .copied()
                            .unwrap_or(true)
                        {
                            return false;
                        }
                        let Some(new_slot) = old_to_new
                            .get(value.slot_index)
                            .copied()
                            .flatten()
                        else {
                            return false;
                        };
                        value.slot_index = new_slot;
                        let Some(slot) = live_effect_slots.get(new_slot) else {
                            return false;
                        };
                        let Some(raw_idx) = slot
                            .param_node_indices
                            .get(value.param_index)
                            .map(|value| value.load(Ordering::Relaxed))
                        else {
                            return false;
                        };
                        let Some(param_id) = crate::neural::ParamNodeId::from_slot_param(
                            slot.node_id.load(Ordering::Relaxed),
                            slot.modulator_node_id.load(Ordering::Relaxed),
                            raw_idx,
                        ) else {
                            return false;
                        };
                        value.param_id = param_id;
                        true
                    });
                }
            }
        }
        drop(scenes);

        let mut live_chains = self.pattern.process_chains.lock().unwrap();
        let live_chain = live_chains
            .get_mut(track)
            .ok_or_else(|| "live process chain is missing".to_string())?;
        remap_chain(live_chain, old_to_new);
        let live_slots = live_effect_slots
            .iter()
            .map(EffectSlotSnapshot::capture)
            .collect::<Vec<_>>();
        crate::process::rebind_track_process_chain_effect_param_ids(
            live_chain,
            effect_descriptors,
            &live_slots,
        );
        Ok(())
    }

    pub(crate) fn restore_track_effect_binding_state(
        &self,
        track: usize,
        snapshot: &TrackEffectBindingStateSnapshot,
        effect_descriptors: &[EffectDescriptor],
    ) -> Result<(), String> {
        let mut scenes = self.pattern.scenes.lock().unwrap();
        let effective = scenes
            .effective_pattern_id(track)
            .ok_or_else(|| format!("Track {} has no effective pattern", track + 1))?;
        let pool = scenes
            .track_pools
            .get_mut(track)
            .ok_or_else(|| format!("Track {} has no pattern pool", track + 1))?;
        if pool.patterns.len() != snapshot.process_chains.len()
            || snapshot.process_chains.iter().any(|(id, _)| !pool.patterns.contains_key(id))
        {
            return Err(format!(
                "Track {} pattern set changed before effect binding replay",
                track + 1
            ));
        }
        if snapshot.project_process_lane_overrides.len() != snapshot.process_chains.len()
            || snapshot
                .project_process_lane_overrides
                .iter()
                .any(|(id, _)| !pool.patterns.contains_key(id))
        {
            return Err("effect history project-lane pattern set changed".to_string());
        }
        let mut live_chain = None;
        let mut live_lane_overrides = None;
        for (id, saved_chain) in &snapshot.process_chains {
            let data = pool.patterns.get_mut(id).expect("pattern set was validated");
            let mut chain = saved_chain.clone();
            crate::process::refresh_track_process_chain_binding_param_ids(
                &mut chain,
                None,
                None,
                effect_descriptors,
                &data.effect_slots,
            );
            data.process_chain = chain.clone();
            if *id == effective {
                live_chain = Some(chain);
            }
        }
        for (id, saved) in &snapshot.project_process_lane_overrides {
            pool.patterns
                .get_mut(id)
                .expect("pattern set was validated")
                .project_process_lane_overrides = saved.clone();
            if *id == effective {
                live_lane_overrides = Some(saved.clone());
            }
        }
        for scene in &mut scenes.scenes {
            for network in &mut scene.neural_networks {
                for neuron in &mut network.neurons {
                    neuron
                        .output_overrides
                        .effects
                        .retain(|value| value.target_track != track);
                }
            }
        }
        let live_slots = self
            .pattern
            .effect_chains
            .get(track)
            .ok_or_else(|| "live effect chain is missing".to_string())?;
        for saved in &snapshot.neural_overrides {
            let neuron = scenes
                .scenes
                .get_mut(saved.scene)
                .and_then(|scene| scene.neural_networks.get_mut(saved.network))
                .and_then(|network| network.neurons.get_mut(saved.neuron))
                .ok_or_else(|| {
                    format!(
                        "Track {} neural topology changed before effect history replay",
                        track + 1
                    )
                })?;
            for (index, value) in &saved.entries {
                let mut value = value.clone();
                let slot = live_slots
                    .get(value.slot_index)
                    .ok_or_else(|| "neural effect slot is out of range".to_string())?;
                let raw_idx = slot
                    .param_node_indices
                    .get(value.param_index)
                    .map(|value| value.load(Ordering::Relaxed))
                    .ok_or_else(|| "neural effect parameter is out of range".to_string())?;
                value.param_id = crate::neural::ParamNodeId::from_slot_param(
                    slot.node_id.load(Ordering::Relaxed),
                    slot.modulator_node_id.load(Ordering::Relaxed),
                    raw_idx,
                )
                .ok_or_else(|| "neural effect parameter has no live identity".to_string())?;
                neuron
                    .output_overrides
                    .effects
                    .insert((*index).min(neuron.output_overrides.effects.len()), value);
            }
        }
        drop(scenes);
        self.pattern.process_chains.lock().unwrap()[track] = live_chain
            .ok_or_else(|| "effective process chain is missing from history".to_string())?;
        self.pattern.project_process_lane_overrides.lock().unwrap()[track] = live_lane_overrides
            .ok_or_else(|| "effective project process lanes are missing from history".to_string())?;
        Ok(())
    }

    pub(crate) fn restore_pattern_effect_device_values_no_publish(
        &self,
        track: usize,
        pattern_id: PatternId,
        slot_idx: usize,
        values: &EffectSlotValuesSnapshot,
    ) -> Result<bool, String> {
        self.restore_pattern_slot_device_values_no_publish(
            track,
            pattern_id,
            slot_idx,
            values,
            false,
        )
    }

    pub(crate) fn capture_pattern_midi_fx_device_values(
        &self,
        track: usize,
        pattern_id: PatternId,
        slot_idx: usize,
    ) -> Result<EffectSlotValuesSnapshot, String> {
        let scenes = self.pattern.scenes.lock().unwrap();
        if scenes.effective_pattern_id(track) == Some(pattern_id) {
            return self
                .pattern
                .midi_fx_slots
                .get(track)
                .and_then(|slots| slots.get(slot_idx))
                .map(EffectSlotSnapshot::capture_authoring_values)
                .ok_or_else(|| "live MIDI-FX target no longer exists".to_string());
        }
        scenes
            .track_pools
            .get(track)
            .and_then(|pool| pool.get(pattern_id))
            .and_then(|data| data.midi_fx_slots.get(slot_idx))
            .map(EffectSlotSnapshot::authoring_values)
            .ok_or_else(|| "Track Pattern MIDI-FX target no longer exists".to_string())
    }

    pub(crate) fn capture_track_midi_fx_chain_values(
        &self,
        track: usize,
    ) -> Result<Vec<(PatternId, Vec<EffectSlotValuesSnapshot>)>, String> {
        let scenes = self.pattern.scenes.lock().unwrap();
        let pool = scenes
            .track_pools
            .get(track)
            .ok_or_else(|| format!("Track {} has no pattern pool", track + 1))?;
        let effective = scenes
            .effective_pattern_id(track)
            .ok_or_else(|| format!("Track {} has no effective pattern", track + 1))?;
        pool.patterns
            .iter()
            .map(|(pattern, data)| {
                let slots = if *pattern == effective {
                    self.pattern
                        .midi_fx_slots
                        .get(track)
                        .ok_or_else(|| "live MIDI-FX chain is missing".to_string())?
                        .iter()
                        .map(EffectSlotSnapshot::capture_authoring_values)
                        .collect()
                } else {
                    data.midi_fx_slots
                        .iter()
                        .map(EffectSlotSnapshot::authoring_values)
                        .collect()
                };
                Ok((*pattern, slots))
            })
            .collect()
    }

    pub(crate) fn restore_track_midi_fx_chain_values(
        &self,
        track: usize,
        names: &[String],
        descriptors: &[EffectDescriptor],
        patterns: &[(PatternId, Vec<EffectSlotValuesSnapshot>)],
    ) -> Result<(), String> {
        if names.len() != descriptors.len() || descriptors.len() > crate::lisp_host::MAX_MIDI_FX_SLOTS {
            return Err("MIDI-FX history layout is invalid".to_string());
        }
        let mut scenes = self.pattern.scenes.lock().unwrap();
        let effective = scenes
            .effective_pattern_id(track)
            .ok_or_else(|| format!("Track {} has no effective pattern", track + 1))?;
        let pool = scenes
            .track_pools
            .get_mut(track)
            .ok_or_else(|| format!("Track {} has no pattern pool", track + 1))?;
        if pool.patterns.len() != patterns.len()
            || patterns.iter().any(|(id, _)| !pool.patterns.contains_key(id))
        {
            return Err(format!(
                "Track {} pattern set changed before MIDI-FX history replay",
                track + 1
            ));
        }
        for (pattern, values) in patterns {
            if values.len() != crate::lisp_host::MAX_MIDI_FX_SLOTS {
                return Err("MIDI-FX pattern layout is invalid".to_string());
            }
            let data = pool.patterns.get_mut(pattern).expect("pattern set was validated");
            data.track_params.midi_fx_chain = names.to_vec();
            for slot_idx in 0..crate::lisp_host::MAX_MIDI_FX_SLOTS {
                let descriptor = descriptors
                    .get(slot_idx)
                    .cloned()
                    .unwrap_or_else(EffectDescriptor::empty_custom_slot);
                let slot = data
                    .midi_fx_slots
                    .get_mut(slot_idx)
                    .ok_or_else(|| "stored MIDI-FX slot is missing".to_string())?;
                slot.sync_to_descriptor(&descriptor, 0);
                slot.apply_authoring_values(&values[slot_idx])?;
            }
        }
        let live_values = patterns
            .iter()
            .find(|(pattern, _)| *pattern == effective)
            .map(|(_, values)| values)
            .ok_or_else(|| "effective MIDI-FX pattern is missing from history".to_string())?;
        self.pattern.track_params[track].set_midi_fx_chain(names.to_vec());
        for slot_idx in 0..crate::lisp_host::MAX_MIDI_FX_SLOTS {
            let descriptor = descriptors
                .get(slot_idx)
                .cloned()
                .unwrap_or_else(EffectDescriptor::empty_custom_slot);
            let slot = self.pattern.midi_fx_slots[track]
                .get(slot_idx)
                .ok_or_else(|| "live MIDI-FX slot is missing".to_string())?;
            let mut snapshot = EffectSlotSnapshot::capture(slot);
            snapshot.sync_to_descriptor(&descriptor, 0);
            snapshot.apply_authoring_values(&live_values[slot_idx])?;
            snapshot.restore(slot);
        }
        Ok(())
    }

    pub(crate) fn remap_track_midi_fx_references(
        &self,
        track: usize,
        old_to_new: &[Option<usize>],
    ) -> Result<(), String> {
        fn remap_chain(
            chain: &mut crate::process::TrackProcessChain,
            old_to_new: &[Option<usize>],
        ) {
            for process_slot in &mut chain.slots {
                for binding in process_slot.bindings.values_mut() {
                    let Some(crate::process::ParamTarget::MidiFxParam { slot, .. }) = binding.as_mut() else {
                        continue;
                    };
                    match old_to_new.get(*slot).copied().flatten() {
                        Some(new_slot) => *slot = new_slot,
                        None => *binding = None,
                    }
                }
            }
        }
        let mut scenes = self.pattern.scenes.lock().unwrap();
        let pool = scenes
            .track_pools
            .get_mut(track)
            .ok_or_else(|| format!("Track {} has no pattern pool", track + 1))?;
        for data in pool.patterns.values_mut() {
            remap_chain(&mut data.process_chain, old_to_new);
        }
        drop(scenes);
        let mut chains = self.pattern.process_chains.lock().unwrap();
        let chain = chains
            .get_mut(track)
            .ok_or_else(|| "live process chain is missing".to_string())?;
        remap_chain(chain, old_to_new);
        Ok(())
    }

    pub(crate) fn capture_track_process_chains(
        &self,
        track: usize,
    ) -> Result<Vec<(PatternId, crate::process::TrackProcessChain)>, String> {
        let scenes = self.pattern.scenes.lock().unwrap();
        let pool = scenes
            .track_pools
            .get(track)
            .ok_or_else(|| format!("Track {} has no pattern pool", track + 1))?;
        let effective = scenes
            .effective_pattern_id(track)
            .ok_or_else(|| format!("Track {} has no effective pattern", track + 1))?;
        let live = self
            .pattern
            .process_chains
            .lock()
            .unwrap()
            .get(track)
            .cloned()
            .ok_or_else(|| "live process chain is missing".to_string())?;
        Ok(pool
            .patterns
            .iter()
            .map(|(id, data)| {
                (*id, if *id == effective { live.clone() } else { data.process_chain.clone() })
            })
            .collect())
    }

    pub(crate) fn restore_track_process_chains(
        &self,
        track: usize,
        saved: &[(PatternId, crate::process::TrackProcessChain)],
    ) -> Result<(), String> {
        let mut scenes = self.pattern.scenes.lock().unwrap();
        let effective = scenes
            .effective_pattern_id(track)
            .ok_or_else(|| format!("Track {} has no effective pattern", track + 1))?;
        let pool = scenes
            .track_pools
            .get_mut(track)
            .ok_or_else(|| format!("Track {} has no pattern pool", track + 1))?;
        if pool.patterns.len() != saved.len()
            || saved.iter().any(|(id, _)| !pool.patterns.contains_key(id))
        {
            return Err("process pattern set changed before history replay".to_string());
        }
        let mut live = None;
        for (id, chain) in saved {
            pool.patterns
                .get_mut(id)
                .expect("pattern set was validated")
                .process_chain = chain.clone();
            if *id == effective {
                live = Some(chain.clone());
            }
        }
        drop(scenes);
        self.pattern.process_chains.lock().unwrap()[track] = live
            .ok_or_else(|| "effective process chain is missing from history".to_string())?;
        Ok(())
    }

    pub(crate) fn restore_pattern_midi_fx_device_values_no_publish(
        &self,
        track: usize,
        pattern_id: PatternId,
        slot_idx: usize,
        values: &EffectSlotValuesSnapshot,
    ) -> Result<bool, String> {
        self.restore_pattern_slot_device_values_no_publish(
            track,
            pattern_id,
            slot_idx,
            values,
            true,
        )
    }

    fn restore_pattern_slot_device_values_no_publish(
        &self,
        track: usize,
        pattern_id: PatternId,
        slot_idx: usize,
        values: &EffectSlotValuesSnapshot,
        midi_fx: bool,
    ) -> Result<bool, String> {
        let mut scenes = self.pattern.scenes.lock().unwrap();
        let is_effective = scenes.effective_pattern_id(track) == Some(pattern_id);
        let data = scenes
            .track_pools
            .get_mut(track)
            .and_then(|pool| pool.get_mut(pattern_id))
            .ok_or_else(|| "Track Pattern target no longer exists".to_string())?;
        let stored = if midi_fx {
            data.midi_fx_slots.get_mut(slot_idx)
        } else {
            data.effect_slots.get_mut(slot_idx)
        }
        .ok_or_else(|| "stored device target no longer exists".to_string())?;
        let mut stored_next = stored.clone();
        stored_next.apply_authoring_values(values)?;
        let live = if is_effective {
            let slot = if midi_fx {
                self.pattern
                    .midi_fx_slots
                    .get(track)
                    .and_then(|slots| slots.get(slot_idx))
            } else {
                self.pattern
                    .effect_chains
                    .get(track)
                    .and_then(|slots| slots.get(slot_idx))
            }
            .ok_or_else(|| "live device target no longer exists".to_string())?;
            let mut snapshot = EffectSlotSnapshot::capture(slot);
            snapshot.apply_authoring_values(values)?;
            Some((slot, snapshot))
        } else {
            None
        };
        *stored = stored_next;
        if let Some((slot, snapshot)) = live {
            snapshot.restore(slot);
        }
        Ok(is_effective)
    }

    pub(crate) fn capture_pattern_rack_slot_values(
        &self,
        track: usize,
        pattern_id: PatternId,
        slot_idx: usize,
    ) -> Result<RackSlotValuesSnapshot, String> {
        let scenes = self.pattern.scenes.lock().unwrap();
        if scenes.effective_pattern_id(track) == Some(pattern_id) {
            return self
                .pattern
                .rack_tracks
                .lock()
                .unwrap()
                .get(track)
                .and_then(Option::as_ref)
                .and_then(|rack| rack.slots.get(slot_idx))
                .map(RackSlotSnapshot::authoring_values)
                .ok_or_else(|| "live rack slot target no longer exists".to_string());
        }
        scenes
            .track_pools
            .get(track)
            .and_then(|pool| pool.get(pattern_id))
            .and_then(|data| data.rack_track.as_ref())
            .and_then(|rack| rack.slots.get(slot_idx))
            .map(RackSlotSnapshot::authoring_values)
            .ok_or_else(|| "Track Pattern rack slot target no longer exists".to_string())
    }

    pub(crate) fn restore_pattern_rack_slot_values_no_publish(
        &self,
        track: usize,
        pattern_id: PatternId,
        slot_idx: usize,
        values: &RackSlotValuesSnapshot,
    ) -> Result<bool, String> {
        let mut scenes = self.pattern.scenes.lock().unwrap();
        let is_effective = scenes.effective_pattern_id(track) == Some(pattern_id);
        let stored = scenes
            .track_pools
            .get_mut(track)
            .and_then(|pool| pool.get_mut(pattern_id))
            .and_then(|data| data.rack_track.as_mut())
            .and_then(|rack| rack.slots.get_mut(slot_idx))
            .ok_or_else(|| "Track Pattern rack slot target no longer exists".to_string())?;
        let mut stored_next = stored.clone();
        stored_next.apply_authoring_values(values)?;
        let live_next = if is_effective {
            let racks = self.pattern.rack_tracks.lock().unwrap();
            let live = racks
                .get(track)
                .and_then(Option::as_ref)
                .and_then(|rack| rack.slots.get(slot_idx))
                .ok_or_else(|| "live rack slot target no longer exists".to_string())?;
            let mut snapshot = live.clone();
            snapshot.apply_authoring_values(values)?;
            Some(snapshot)
        } else {
            None
        };
        *stored = stored_next;
        if let Some(snapshot) = live_next {
            let mut racks = self.pattern.rack_tracks.lock().unwrap();
            let live = racks
                .get_mut(track)
                .and_then(Option::as_mut)
                .and_then(|rack| rack.slots.get_mut(slot_idx))
                .ok_or_else(|| "live rack slot target no longer exists".to_string())?;
            *live = snapshot;
        }
        Ok(is_effective)
    }

    pub(crate) fn restore_pattern_num_steps_no_publish(
        &self,
        track: usize,
        pattern_id: PatternId,
        num_steps: usize,
    ) -> Result<bool, String> {
        let num_steps = num_steps.clamp(1, MAX_STEPS);
        let mut scenes = self.pattern.scenes.lock().unwrap();
        let is_effective = scenes.effective_pattern_id(track) == Some(pattern_id);
        let data = scenes
            .track_pools
            .get_mut(track)
            .and_then(|pool| pool.get_mut(pattern_id))
            .ok_or_else(|| "Track Pattern target no longer exists".to_string())?;
        data.track_params.num_steps = num_steps;
        if is_effective {
            self.pattern
                .track_params
                .get(track)
                .ok_or_else(|| "live track target no longer exists".to_string())?
                .set_num_steps(num_steps);
        }
        Ok(is_effective)
    }

    /// Restore a stable Track Pattern step batch without publishing.
    ///
    /// The pool is always updated. The live mirror is updated only if the
    /// same pattern remains effective, so scene changes cannot redirect replay.
    pub(crate) fn restore_pattern_step_cells_no_publish(
        &self,
        track: usize,
        pattern_id: PatternId,
        cells: &[(usize, StepSnapshot)],
        variant_registry: &PlockVariantRegistry,
    ) -> Result<bool, String> {
        if cells.iter().any(|(step, _)| *step >= MAX_STEPS) {
            return Err("step target is out of range".to_string());
        }
        let initially_effective = {
            let scenes = self.pattern.scenes.lock().unwrap();
            if scenes
                .track_pools
                .get(track)
                .and_then(|pool| pool.get(pattern_id))
                .is_none()
            {
                return Err("Track Pattern target no longer exists".to_string());
            }
            scenes.effective_pattern_id(track) == Some(pattern_id)
        };
        if initially_effective {
            self.validate_live_step_cell_target(track)?;
        }
        let is_effective = {
            let mut scenes = self.pattern.scenes.lock().unwrap();
            let is_effective = scenes.effective_pattern_id(track) == Some(pattern_id);
            if is_effective && !initially_effective {
                return Err("Track Pattern became active during step replay".to_string());
            }
            let data = scenes
                .track_pools
                .get_mut(track)
                .and_then(|pool| pool.get_mut(pattern_id))
                .ok_or_else(|| "Track Pattern target no longer exists".to_string())?;
            for (step, snapshot) in cells {
                if !data.restore_step_snapshot(*step, snapshot) {
                    return Err("stored step target is out of range".to_string());
                }
            }
            data.plock_variant_registry = variant_registry.clone();
            is_effective
        };

        if is_effective {
            if track >= self.pattern.patterns.len() {
                return Err("live track target no longer exists".to_string());
            }
            for (step, snapshot) in cells {
                self.restore_step_snapshot_inner(track, *step, snapshot);
            }
            let mut registries = self.pattern.plock_variant_registries.lock().unwrap();
            let registry = registries
                .get_mut(track)
                .ok_or_else(|| "live p-lock variant registry is missing".to_string())?;
            *registry = variant_registry.clone();
        }
        Ok(is_effective)
    }

    fn validate_live_step_cell_target(&self, track: usize) -> Result<(), String> {
        let lanes = [
            (self.pattern.patterns.len(), "step active bits"),
            (
                self.pattern.neural_reset_patterns.len(),
                "neural-reset bits",
            ),
            (self.pattern.step_data.len(), "step parameter data"),
            (self.pattern.chord_data.len(), "chord data"),
            (self.pattern.timebase_plocks.len(), "timebase p-locks"),
            (self.pattern.swing_plocks.len(), "swing p-locks"),
            (
                self.pattern.swing_resolution_plocks.len(),
                "swing-resolution p-locks",
            ),
            (self.pattern.midi_fx_slots.len(), "MIDI-FX slots"),
            (self.pattern.effect_chains.len(), "audio-effect slots"),
            (self.pattern.instrument_slots.len(), "instrument slots"),
        ];
        if let Some((len, name)) = lanes.into_iter().find(|(len, _)| track >= *len) {
            return Err(format!(
                "live track {track} is missing from {name} (length {len})"
            ));
        }
        if track >= self.pattern.rack_tracks.lock().unwrap().len() {
            return Err("live rack-track lane is missing".to_string());
        }
        if track
            >= self
                .pattern
                .plock_variant_registries
                .lock()
                .unwrap()
                .len()
        {
            return Err("live p-lock variant registry is missing".to_string());
        }
        Ok(())
    }

    pub(crate) fn clear_step_payload_inner(&self, track: usize, step: usize) {
        for param in StepParam::ALL {
            self.pattern.step_data[track].set(step, param, param.default_value());
        }

        self.pattern.patterns[track].clear_step(step);
        self.pattern.neural_reset_patterns[track].clear_step(step);

        self.pattern.chord_data[track].clear_step(step);
        self.pattern.timebase_plocks[track].clear(step);
        self.pattern.swing_plocks[track].clear(step);
        self.pattern.swing_resolution_plocks[track].clear(step);

        for slot in &self.pattern.midi_fx_slots[track] {
            slot.plocks.clear_step(step);
            for tensor_idx in 0..slot.tensor_params.num_params() {
                slot.tensor_params.clear_plock(step, tensor_idx);
            }
        }

        for slot in &self.pattern.effect_chains[track] {
            slot.plocks.clear_step(step);
            for tensor_idx in 0..slot.tensor_params.num_params() {
                slot.tensor_params.clear_plock(step, tensor_idx);
            }
        }

        let instrument_slot = &self.pattern.instrument_slots[track];
        instrument_slot.plocks.clear_step(step);
        for tensor_idx in 0..instrument_slot.tensor_params.num_params() {
            instrument_slot.tensor_params.clear_plock(step, tensor_idx);
        }
        self.clear_rack_macro_plocks_for_step(track, step);
        if let Some(Some(rack)) = self.pattern.rack_tracks.lock().unwrap().get_mut(track) {
            for slot in &mut rack.slots {
                slot.param_plocks.clear_step(step);
                slot.instrument_slot.clear_step_plocks(step);
                for tensor_idx in 0..slot.instrument_slot.tensor_params.len() {
                    slot.instrument_slot.clear_tensor_plock(step, tensor_idx);
                }
                for effect in &mut slot.effect_slots {
                    effect.clear_step_plocks(step);
                    for tensor_idx in 0..effect.tensor_params.len() {
                        effect.clear_tensor_plock(step, tensor_idx);
                    }
                }
            }
        }
    }

    pub fn clear_step_payload(&self, track: usize, step: usize) {
        self.clear_step_payload_inner(track, step);
        self.publish_scheduler_snapshot();
    }

    pub fn clear_step_payload_no_publish(&self, track: usize, step: usize) {
        self.clear_step_payload_inner(track, step);
    }

    pub fn reconcile_plock_variant_registry_for_track(
        &self,
        track: usize,
    ) -> Vec<Option<PlockVariantAssignment>> {
        let keys = live_track_variant_keys(self, track);
        let mut registries = self.pattern.plock_variant_registries.lock().unwrap();
        let Some(registry) = registries.get_mut(track) else {
            return vec![None; MAX_STEPS];
        };
        registry.reconcile(keys)
    }

    pub fn plock_variant_registry_snapshot(&self, track: usize) -> PlockVariantRegistry {
        let _ = self.reconcile_plock_variant_registry_for_track(track);
        self.pattern
            .plock_variant_registries
            .lock()
            .unwrap()
            .get(track)
            .cloned()
            .unwrap_or_default()
    }

    pub fn reconcile_key_lock_variant_registry_for_track(
        &self,
        track: usize,
    ) -> Vec<Option<PlockVariantAssignment>> {
        let keys = live_track_key_lock_variant_keys(self, track);
        let mut registries = self.pattern.key_lock_variant_registries.lock().unwrap();
        let Some(registry) = registries.get_mut(track) else {
            return vec![None; crate::effects::MAX_MIDI_NOTES];
        };
        registry.reconcile(keys)
    }

    pub fn key_lock_variant_registry_snapshot(&self, track: usize) -> PlockVariantRegistry {
        let _ = self.reconcile_key_lock_variant_registry_for_track(track);
        self.pattern
            .key_lock_variant_registries
            .lock()
            .unwrap()
            .get(track)
            .cloned()
            .unwrap_or_default()
    }

    pub fn clear_key_lock_variant_locks_for_notes(&self, track: usize, notes: &[u8]) -> bool {
        if track >= self.pattern.instrument_slots.len() {
            return false;
        }
        let slot = &self.pattern.instrument_slots[track];
        let num_params = slot.num_params.load(Ordering::Relaxed) as usize;
        let mut changed = false;
        for note in notes.iter().copied() {
            if slot.key_locks.note_has_any_lock(note, num_params) {
                slot.clear_note_key_locks(note);
                changed = true;
            }
        }
        if changed {
            let _ = self.reconcile_key_lock_variant_registry_for_track(track);
            self.publish_scheduler_snapshot();
        }
        changed
    }

    pub fn stamp_key_lock_variant_key_to_notes(
        &self,
        track: usize,
        key: &PlockVariantKey,
        notes: &[u8],
    ) -> bool {
        if track >= self.pattern.instrument_slots.len() {
            return false;
        }
        let mut changed = false;
        for note in notes.iter().copied() {
            if live_track_key_lock_variant_key(self, track, note)
                .as_ref()
                .is_some_and(|candidate| candidate == key)
            {
                continue;
            }
            let slot = &self.pattern.instrument_slots[track];
            slot.clear_note_key_locks(note);
            for entry in &key.entries {
                if entry.domain != crate::plock_variants::PlockVariantDomain::InstrumentKeyLock
                    || entry.slot != 0
                    || entry.cell.is_some()
                    || entry.param >= slot.num_params.load(Ordering::Relaxed) as usize
                {
                    continue;
                }
                slot.set_key_lock(note, entry.param, f32::from_bits(entry.value_bits));
            }
            changed = true;
        }
        if changed {
            let _ = self.reconcile_key_lock_variant_registry_for_track(track);
            self.publish_scheduler_snapshot();
        }
        changed
    }

    pub fn clear_variant_locks_for_steps(&self, track: usize, steps: &[usize]) -> bool {
        let changed = self.clear_variant_locks_for_steps_no_publish(track, steps);
        if changed {
            self.publish_scheduler_snapshot();
        }
        changed
    }

    pub fn clear_variant_locks_for_steps_no_publish(
        &self,
        track: usize,
        steps: &[usize],
    ) -> bool {
        if track >= self.pattern.instrument_slots.len() {
            return false;
        }
        let mut changed = false;
        for step in steps.iter().copied().filter(|step| *step < MAX_STEPS) {
            changed |= self.clear_variant_locks_for_step_inner(track, step);
        }
        if changed {
            let _ = self.reconcile_plock_variant_registry_for_track(track);
        }
        changed
    }

    pub fn stamp_variant_key_to_steps(
        &self,
        track: usize,
        key: &PlockVariantKey,
        steps: &[usize],
    ) -> bool {
        let changed = self.stamp_variant_key_to_steps_no_publish(track, key, steps);
        if changed {
            self.publish_scheduler_snapshot();
        }
        changed
    }

    pub fn stamp_variant_key_to_steps_no_publish(
        &self,
        track: usize,
        key: &PlockVariantKey,
        steps: &[usize],
    ) -> bool {
        let Some(source_step) = self.find_step_with_variant_key(track, key) else {
            return false;
        };
        self.copy_variant_locks_from_step_to_steps_no_publish(track, source_step, steps)
    }

    pub fn copy_variant_locks_from_step_to_steps(
        &self,
        track: usize,
        source_step: usize,
        steps: &[usize],
    ) -> bool {
        let changed = self.copy_variant_locks_from_step_to_steps_no_publish(
            track,
            source_step,
            steps,
        );
        if changed {
            self.publish_scheduler_snapshot();
        }
        changed
    }

    pub(crate) fn copy_variant_locks_from_step_to_steps_no_publish(
        &self,
        track: usize,
        source_step: usize,
        steps: &[usize],
    ) -> bool {
        if track >= self.pattern.instrument_slots.len() || source_step >= MAX_STEPS {
            return false;
        }
        let mut changed = false;
        for target_step in steps.iter().copied().filter(|step| *step < MAX_STEPS) {
            changed |= self.copy_variant_locks_between_steps_inner(track, source_step, target_step);
        }
        if changed {
            let _ = self.reconcile_plock_variant_registry_for_track(track);
        }
        changed
    }

    fn find_step_with_variant_key(&self, track: usize, key: &PlockVariantKey) -> Option<usize> {
        (0..MAX_STEPS).find(|step| {
            live_track_variant_key(self, track, *step)
                .as_ref()
                .is_some_and(|candidate| candidate == key)
        })
    }

    fn clear_rack_macro_plocks_for_step(&self, track: usize, step: usize) -> bool {
        let ids = self
            .pattern
            .rack_tracks
            .lock()
            .unwrap()
            .get(track)
            .and_then(Option::as_ref)
            .map(|rack| {
                rack.macros
                    .iter()
                    .filter(|rack_macro| rack_macro.plocks.get(step).is_some_and(Option::is_some))
                    .map(|rack_macro| rack_macro.id)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let mut changed = false;
        for id in ids {
            changed |= self.update_rack_macro_in_current_pattern(track, id, |rack_macro| {
                rack_macro.plocks[step] = None;
            });
        }
        changed
    }

    fn copy_rack_macro_plocks_between_steps(
        &self,
        track: usize,
        source_step: usize,
        target_step: usize,
    ) -> bool {
        let values = self
            .pattern
            .rack_tracks
            .lock()
            .unwrap()
            .get(track)
            .and_then(Option::as_ref)
            .map(|rack| {
                rack.macros
                    .iter()
                    .filter_map(|rack_macro| {
                        let source = rack_macro.plocks.get(source_step).copied().flatten();
                        let target = rack_macro.plocks.get(target_step).copied().flatten();
                        (source != target).then_some((rack_macro.id, source))
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let mut changed = false;
        for (id, value) in values {
            changed |= self.update_rack_macro_in_current_pattern(track, id, |rack_macro| {
                rack_macro.plocks[target_step] = value;
            });
        }
        changed
    }

    fn clear_variant_locks_for_step_inner(&self, track: usize, step: usize) -> bool {
        let mut changed = clear_track_variant_locks(self, track, step);
        for slot in &self.pattern.midi_fx_slots[track] {
            changed |= clear_live_slot_variant_locks(slot, step);
        }
        for slot in &self.pattern.effect_chains[track] {
            changed |= clear_live_slot_variant_locks(slot, step);
        }
        changed |= clear_live_slot_variant_locks(&self.pattern.instrument_slots[track], step);
        changed |= self.clear_rack_macro_plocks_for_step(track, step);
        if let Some(Some(rack)) = self.pattern.rack_tracks.lock().unwrap().get_mut(track) {
            for slot in &mut rack.slots {
                changed |= clear_rack_slot_variant_locks(slot, step);
            }
        }
        changed
    }

    fn copy_variant_locks_between_steps_inner(
        &self,
        track: usize,
        source_step: usize,
        target_step: usize,
    ) -> bool {
        let mut changed = copy_track_variant_locks(self, track, source_step, target_step);
        for slot in &self.pattern.midi_fx_slots[track] {
            changed |= copy_live_slot_variant_locks(slot, source_step, target_step);
        }
        for slot in &self.pattern.effect_chains[track] {
            changed |= copy_live_slot_variant_locks(slot, source_step, target_step);
        }
        changed |= copy_live_slot_variant_locks(
            &self.pattern.instrument_slots[track],
            source_step,
            target_step,
        );
        changed |= self.copy_rack_macro_plocks_between_steps(track, source_step, target_step);
        if let Some(Some(rack)) = self.pattern.rack_tracks.lock().unwrap().get_mut(track) {
            for slot in &mut rack.slots {
                changed |= copy_rack_slot_variant_locks(slot, source_step, target_step);
            }
        }
        changed
    }

    pub(crate) fn set_step_param_inner(
        &self,
        track: usize,
        step: usize,
        param: StepParam,
        value: f32,
    ) {
        let previous = self.pattern.step_data[track].get(step, param);
        self.pattern.step_data[track].set(step, param, value);

        if param != StepParam::Transpose {
            return;
        }

        let applied = self.pattern.step_data[track].get(step, param);
        let delta = applied - previous;
        if delta == 0.0 {
            return;
        }

        let chord_count = self.pattern.chord_data[track].count(step);
        if chord_count == 0 {
            return;
        }

        let mut notes = Vec::with_capacity(chord_count);
        for note_idx in 0..chord_count {
            notes.push((
                self.pattern.chord_data[track].get(step, note_idx) + delta,
                self.pattern.chord_data[track].get_duration(step, note_idx),
                self.pattern.chord_data[track].get_delay(step, note_idx),
            ));
        }
        self.pattern.chord_data[track].clear_step(step);
        for (transpose, duration, delay) in notes {
            self.pattern.chord_data[track].add_note_with_timing(step, transpose, duration, delay);
        }
    }

    pub fn set_step_param(&self, track: usize, step: usize, param: StepParam, value: f32) {
        self.set_step_param_inner(track, step, param, value);
        self.publish_scheduler_snapshot();
    }

    pub fn adjust_step_param(&self, track: usize, step: usize, param: StepParam, delta: f32) {
        let current = self.pattern.step_data[track].get(step, param);
        self.set_step_param(track, step, param, current + delta);
    }

    pub(crate) fn restore_step_snapshot_inner(
        &self,
        track: usize,
        step: usize,
        snapshot: &StepSnapshot,
    ) {
        for param in StepParam::ALL {
            self.pattern.step_data[track].set(step, param, snapshot.params[param.index()]);
        }

        self.pattern.patterns[track].set_step_active(step, snapshot.active);
        self.pattern.neural_reset_patterns[track].set_step_active(step, snapshot.neural_reset);

        self.pattern.chord_data[track].clear_step(step);
        for (idx, &transpose) in snapshot.chord.iter().enumerate() {
            self.pattern.chord_data[track].add_note_with_timing(
                step,
                transpose,
                snapshot.chord_durations.get(idx).copied().unwrap_or(0.0),
                snapshot.chord_delays.get(idx).copied().unwrap_or(0.0),
            );
        }

        match snapshot.timebase {
            Some(tb) => self.pattern.timebase_plocks[track].set(step, tb),
            None => self.pattern.timebase_plocks[track].clear(step),
        }
        match snapshot.swing {
            Some(swing) => self.pattern.swing_plocks[track].set(step, swing),
            None => self.pattern.swing_plocks[track].clear(step),
        }
        match snapshot.swing_resolution {
            Some(resolution) => self.pattern.swing_resolution_plocks[track].set(step, resolution),
            None => self.pattern.swing_resolution_plocks[track].clear(step),
        }

        for (slot_idx, slot) in self.pattern.midi_fx_slots[track].iter().enumerate() {
            restore_live_slot_step_plocks(slot, step, snapshot.midi_fx_plocks.get(slot_idx));
        }
        for (slot_idx, slot) in self.pattern.effect_chains[track].iter().enumerate() {
            restore_live_slot_step_plocks(slot, step, snapshot.effect_plocks.get(slot_idx));
        }

        restore_live_slot_step_plocks(
            &self.pattern.instrument_slots[track],
            step,
            Some(&snapshot.instrument_plocks),
        );

        if let Some(Some(rack)) = self.pattern.rack_tracks.lock().unwrap().get_mut(track) {
            for (macro_idx, rack_macro) in rack.macros.iter_mut().enumerate() {
                rack_macro.plocks[step] = snapshot
                    .rack_macro_plocks
                    .get(macro_idx)
                    .copied()
                    .flatten();
            }
            for (slot_idx, slot) in rack.slots.iter_mut().enumerate() {
                let saved_params = snapshot.rack_slot_param_plocks.get(slot_idx);
                for param in RackSlotParam::ALL {
                    let value = saved_params
                        .and_then(|plocks| plocks.params.get(param.index()))
                        .copied()
                        .flatten();
                    match value {
                        Some(value) => {
                            slot.param_plocks.set(step, param, value);
                        }
                        None => {
                            slot.param_plocks.clear(step, param);
                        }
                    }
                }

                restore_snapshot_slot_step_plocks(
                    &mut slot.instrument_slot,
                    step,
                    snapshot.rack_slot_instrument_plocks.get(slot_idx),
                );
                for (effect_idx, effect) in slot.effect_slots.iter_mut().enumerate() {
                    restore_snapshot_slot_step_plocks(
                        effect,
                        step,
                        snapshot
                            .rack_slot_effect_plocks
                            .get(slot_idx)
                            .and_then(|effects| effects.get(effect_idx)),
                    );
                }
            }
        }
    }

    pub fn restore_step_snapshot(&self, track: usize, step: usize, snapshot: &StepSnapshot) {
        self.restore_step_snapshot_inner(track, step, snapshot);
        self.publish_scheduler_snapshot();
    }

    pub fn restore_step_snapshot_no_publish(
        &self,
        track: usize,
        step: usize,
        snapshot: &StepSnapshot,
    ) {
        self.restore_step_snapshot_inner(track, step, snapshot);
    }

    /// Cyclically rotate `steps` (sorted) left (direction < 0) or right (direction > 0).
    pub(crate) fn rotate_steps_no_publish(
        &self,
        track: usize,
        steps: &[usize],
        direction: isize,
    ) {
        if steps.len() < 2 || direction == 0 {
            return;
        }
        let snapshots: Vec<_> = steps
            .iter()
            .map(|&s| self.capture_step_snapshot(track, s))
            .collect();
        let n = steps.len();
        for (i, &step) in steps.iter().enumerate() {
            let src = if direction > 0 {
                // Rotate right: slot i gets content from slot i-1 (last wraps to first)
                if i == 0 {
                    n - 1
                } else {
                    i - 1
                }
            } else {
                // Rotate left: slot i gets content from slot i+1 (first wraps to last)
                (i + 1) % n
            };
            self.restore_step_snapshot_inner(track, step, &snapshots[src]);
        }
    }

    pub fn rotate_steps(&self, track: usize, steps: &[usize], direction: isize) {
        self.rotate_steps_no_publish(track, steps, direction);
        self.publish_scheduler_snapshot();
    }

    pub(crate) fn move_step_range_no_publish(
        &self,
        track: usize,
        lo: usize,
        hi: usize,
        new_lo: usize,
    ) {
        if lo > hi || hi >= MAX_STEPS {
            return;
        }

        let count = hi - lo + 1;
        let new_hi = new_lo + count - 1;
        if new_lo == lo || new_hi >= MAX_STEPS {
            return;
        }

        let snapshots: Vec<_> = (lo..=hi)
            .map(|step| self.capture_step_snapshot(track, step))
            .collect();

        for step in lo..=hi {
            if step < new_lo || step > new_hi {
                self.clear_step_payload_inner(track, step);
            }
        }

        for (offset, step) in (new_lo..=new_hi).enumerate() {
            self.restore_step_snapshot_inner(track, step, &snapshots[offset]);
        }
    }

    pub fn move_step_range(&self, track: usize, lo: usize, hi: usize, new_lo: usize) {
        self.move_step_range_no_publish(track, lo, hi, new_lo);
        self.publish_scheduler_snapshot();
    }

    pub(crate) fn duplicate_track_pattern_no_publish(&self, track: usize) -> usize {
        let num_steps = self.pattern.track_params[track].get_num_steps();
        let new_len = (num_steps * 2).min(MAX_STEPS);
        if new_len == num_steps {
            return num_steps;
        }

        for step in num_steps..new_len {
            let src = step - num_steps;
            let snapshot = self.capture_step_snapshot(track, src);
            self.restore_step_snapshot_inner(track, step, &snapshot);
        }

        self.pattern.track_params[track].set_num_steps(new_len);
        new_len
    }

    pub fn duplicate_track_pattern(&self, track: usize) -> usize {
        let new_len = self.duplicate_track_pattern_no_publish(track);
        self.publish_scheduler_snapshot();
        new_len
    }

    pub(crate) fn halve_track_pattern_no_publish(&self, track: usize) -> usize {
        let num_steps = self.pattern.track_params[track].get_num_steps();
        let new_len = (num_steps / 2).max(1);
        if new_len == num_steps {
            return num_steps;
        }
        self.pattern.track_params[track].set_num_steps(new_len);
        new_len
    }

    pub fn halve_track_pattern(&self, track: usize) -> usize {
        let new_len = self.halve_track_pattern_no_publish(track);
        self.publish_scheduler_snapshot();
        new_len
    }
}

fn option_f32_bits_equal(a: Option<f32>, b: Option<f32>) -> bool {
    match (a, b) {
        (Some(a), Some(b)) => a.to_bits() == b.to_bits(),
        (None, None) => true,
        _ => false,
    }
}

fn f32_slices_bits_equal(a: &[f32], b: &[f32]) -> bool {
    a.len() == b.len()
        && a.iter()
            .zip(b)
            .all(|(left, right)| left.to_bits() == right.to_bits())
}

fn clear_track_variant_locks(state: &SequencerState, track: usize, step: usize) -> bool {
    let mut changed = false;
    if state.pattern.timebase_plocks[track].has_plock(step) {
        state.pattern.timebase_plocks[track].clear(step);
        changed = true;
    }
    if state.pattern.swing_plocks[track].has_plock(step) {
        state.pattern.swing_plocks[track].clear(step);
        changed = true;
    }
    if state.pattern.swing_resolution_plocks[track].has_plock(step) {
        state.pattern.swing_resolution_plocks[track].clear(step);
        changed = true;
    }
    changed
}

fn copy_track_variant_locks(
    state: &SequencerState,
    track: usize,
    source_step: usize,
    target_step: usize,
) -> bool {
    let mut changed = false;
    let source_timebase = state.pattern.timebase_plocks[track].get(source_step);
    let target_timebase = state.pattern.timebase_plocks[track].get(target_step);
    if source_timebase != target_timebase {
        match source_timebase {
            Some(value) => state.pattern.timebase_plocks[track].set(target_step, value),
            None => state.pattern.timebase_plocks[track].clear(target_step),
        }
        changed = true;
    }

    let source_swing = state.pattern.swing_plocks[track].get(source_step);
    let target_swing = state.pattern.swing_plocks[track].get(target_step);
    if !option_f32_bits_equal(source_swing, target_swing) {
        match source_swing {
            Some(value) => state.pattern.swing_plocks[track].set(target_step, value),
            None => state.pattern.swing_plocks[track].clear(target_step),
        }
        changed = true;
    }

    let source_resolution = state.pattern.swing_resolution_plocks[track].get(source_step);
    let target_resolution = state.pattern.swing_resolution_plocks[track].get(target_step);
    if source_resolution != target_resolution {
        match source_resolution {
            Some(value) => state.pattern.swing_resolution_plocks[track].set(target_step, value),
            None => state.pattern.swing_resolution_plocks[track].clear(target_step),
        }
        changed = true;
    }
    changed
}

fn clear_live_slot_variant_locks(slot: &EffectSlotState, step: usize) -> bool {
    let mut changed = false;
    let num_params = (slot.num_params.load(Ordering::Relaxed) as usize).min(MAX_SLOT_PARAMS);
    for param_idx in 0..num_params {
        if slot.plocks.get(step, param_idx).is_some() {
            slot.plocks.clear_param(step, param_idx);
            changed = true;
        }
    }
    for tensor_idx in 0..slot.tensor_params.num_params() {
        if slot.tensor_params.plock_values(step, tensor_idx).is_some() {
            slot.tensor_params.clear_plock(step, tensor_idx);
            changed = true;
        }
    }
    changed
}

fn copy_live_slot_variant_locks(
    slot: &EffectSlotState,
    source_step: usize,
    target_step: usize,
) -> bool {
    let mut changed = false;
    let num_params = (slot.num_params.load(Ordering::Relaxed) as usize).min(MAX_SLOT_PARAMS);
    for param_idx in 0..num_params {
        let source = slot.plocks.get(source_step, param_idx);
        let target = slot.plocks.get(target_step, param_idx);
        if option_f32_bits_equal(source, target) {
            continue;
        }
        match source {
            Some(value) => slot.set_plock(target_step, param_idx, value),
            None => slot.plocks.clear_param(target_step, param_idx),
        }
        changed = true;
    }

    for tensor_idx in 0..slot.tensor_params.num_params() {
        let source = slot.tensor_params.plock_values(source_step, tensor_idx);
        let target = slot.tensor_params.plock_values(target_step, tensor_idx);
        let equal = match (&source, &target) {
            (Some(source), Some(target)) => f32_slices_bits_equal(source, target),
            (None, None) => true,
            _ => false,
        };
        if equal {
            continue;
        }
        match source {
            Some(values) => {
                slot.tensor_params
                    .set_plock(target_step, tensor_idx, &values);
            }
            None => {
                slot.tensor_params.clear_plock(target_step, tensor_idx);
            }
        }
        changed = true;
    }
    changed
}

fn clear_rack_slot_variant_locks(slot: &mut RackSlotSnapshot, step: usize) -> bool {
    let mut changed = false;
    for param in RackSlotParam::ALL {
        if slot.param_plocks.get(step, param).is_some() {
            slot.param_plocks.clear(step, param);
            changed = true;
        }
    }
    changed |= clear_snapshot_slot_variant_locks(&mut slot.instrument_slot, step);
    changed
}

fn copy_rack_slot_variant_locks(
    slot: &mut RackSlotSnapshot,
    source_step: usize,
    target_step: usize,
) -> bool {
    let mut changed = false;
    for param in RackSlotParam::ALL {
        let source = slot.param_plocks.get(source_step, param);
        let target = slot.param_plocks.get(target_step, param);
        if option_f32_bits_equal(source, target) {
            continue;
        }
        match source {
            Some(value) => {
                slot.param_plocks.set(target_step, param, value);
            }
            None => {
                slot.param_plocks.clear(target_step, param);
            }
        }
        changed = true;
    }
    changed |=
        copy_snapshot_slot_variant_locks(&mut slot.instrument_slot, source_step, target_step);
    changed
}

fn clear_snapshot_slot_variant_locks(slot: &mut EffectSlotSnapshot, step: usize) -> bool {
    let mut changed = false;
    let num_params = slot.num_params as usize;
    let params_to_clear = slot
        .plocks
        .get(step)
        .map(|row| {
            (0..num_params.min(row.len()))
                .filter(|param_idx| row[*param_idx].is_some())
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    for param_idx in params_to_clear {
        slot.clear_plock(step, param_idx);
        changed = true;
    }
    for tensor_idx in 0..slot.tensor_params.len() {
        if slot.tensor_plock_values(step, tensor_idx).is_some() {
            slot.clear_tensor_plock(step, tensor_idx);
            changed = true;
        }
    }
    changed
}

fn copy_snapshot_slot_variant_locks(
    slot: &mut EffectSlotSnapshot,
    source_step: usize,
    target_step: usize,
) -> bool {
    let mut changed = false;
    let num_params = slot.num_params as usize;
    for param_idx in 0..num_params {
        let source = slot
            .plocks
            .get(source_step)
            .and_then(|row| row.get(param_idx))
            .copied()
            .flatten();
        let target = slot
            .plocks
            .get(target_step)
            .and_then(|row| row.get(param_idx))
            .copied()
            .flatten();
        if option_f32_bits_equal(source, target) {
            continue;
        }
        match source {
            Some(value) => {
                slot.set_plock(target_step, param_idx, value);
            }
            None => {
                slot.clear_plock(target_step, param_idx);
            }
        }
        changed = true;
    }

    for tensor_idx in 0..slot.tensor_params.len() {
        let source = slot
            .tensor_plock_values(source_step, tensor_idx)
            .map(|values| values.to_vec());
        let target = slot
            .tensor_plock_values(target_step, tensor_idx)
            .map(|values| values.to_vec());
        let equal = match (&source, &target) {
            (Some(source), Some(target)) => f32_slices_bits_equal(source, target),
            (None, None) => true,
            _ => false,
        };
        if equal {
            continue;
        }
        match source {
            Some(values) => {
                slot.set_tensor_plock(target_step, tensor_idx, values);
            }
            None => {
                slot.clear_tensor_plock(target_step, tensor_idx);
            }
        }
        changed = true;
    }
    changed
}

#[cfg(test)]
#[path = "state_tests.rs"]
mod tests;
