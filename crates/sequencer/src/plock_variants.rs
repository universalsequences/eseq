use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::atomic::Ordering;

use serde::{Deserialize, Serialize};

use crate::effects::{EffectSlotSnapshot, EffectSlotState, MAX_MIDI_NOTES, MAX_SLOT_PARAMS};
use crate::sequencer::{RackSlotParam, RackSlotSnapshot, SequencerState, StepParam, MAX_STEPS};

pub const VARIANT_PALETTE: [[f32; 3]; 6] = [
    [0.270_588_25, 0.784_313_74, 0.862_745_1],
    [0.909_803_9, 0.643_137_3, 0.309_803_93],
    [0.662_745_1, 0.494_117_65, 0.909_803_9],
    [0.435_294_12, 0.807_843_15, 0.541_176_5],
    [0.909_803_9, 0.415_686_28, 0.415_686_28],
    [0.850_980_4, 0.788_235_3, 0.352_941_2],
];

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum PlockVariantDomain {
    Instrument,
    InstrumentTensor,
    Effect,
    EffectTensor,
    RackSlotParam,
    RackSlotInstrument,
    RackSlotInstrumentTensor,
    InstrumentKeyLock,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct PlockVariantEntry {
    pub domain: PlockVariantDomain,
    pub slot: usize,
    pub param: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cell: Option<usize>,
    pub value_bits: u32,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct PlockVariantKey {
    pub entries: Vec<PlockVariantEntry>,
}

impl PlockVariantKey {
    pub fn new(mut entries: Vec<PlockVariantEntry>) -> Option<Self> {
        entries.sort();
        (!entries.is_empty()).then_some(Self { entries })
    }

    pub fn param_count(&self) -> usize {
        self.entries.len()
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PlockVariantRegistryEntry {
    pub key: PlockVariantKey,
    pub label: String,
    pub color: [f32; 3],
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default)]
    pub color_index: usize,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct PlockVariantRegistry {
    pub entries: Vec<PlockVariantRegistryEntry>,
    #[serde(default)]
    pub previous_step_keys: Vec<Option<PlockVariantKey>>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PlockVariantAssignment {
    pub key: PlockVariantKey,
    pub label: String,
    pub color: [f32; 3],
    pub name: Option<String>,
    pub param_count: usize,
}

impl From<&PlockVariantRegistryEntry> for PlockVariantAssignment {
    fn from(value: &PlockVariantRegistryEntry) -> Self {
        Self {
            key: value.key.clone(),
            label: value.label.clone(),
            color: value.color,
            name: value.name.clone(),
            param_count: value.key.param_count(),
        }
    }
}

impl PlockVariantRegistry {
    pub fn reconcile(
        &mut self,
        current_step_keys: Vec<Option<PlockVariantKey>>,
    ) -> Vec<Option<PlockVariantAssignment>> {
        let mut active_counts: BTreeMap<PlockVariantKey, usize> = BTreeMap::new();
        for key in current_step_keys.iter().flatten() {
            *active_counts.entry(key.clone()).or_default() += 1;
        }

        let mut previous_counts: BTreeMap<PlockVariantKey, usize> = BTreeMap::new();
        for key in self.previous_step_keys.iter().flatten() {
            *previous_counts.entry(key.clone()).or_default() += 1;
        }

        let mut entry_by_key: HashMap<PlockVariantKey, usize> = self
            .entries
            .iter()
            .enumerate()
            .map(|(idx, entry)| (entry.key.clone(), idx))
            .collect();

        for (step, new_key) in current_step_keys.iter().enumerate() {
            let old_key = self.previous_step_keys.get(step).and_then(Clone::clone);
            let (Some(old_key), Some(new_key)) = (old_key, new_key) else {
                continue;
            };
            if old_key == *new_key
                || previous_counts.get(&old_key).copied().unwrap_or(0) != 1
                || active_counts.contains_key(&old_key)
                || entry_by_key.contains_key(new_key)
            {
                continue;
            }
            if let Some(entry_idx) = entry_by_key.remove(&old_key) {
                self.entries[entry_idx].key = new_key.clone();
                entry_by_key.insert(new_key.clone(), entry_idx);
            }
        }

        self.entries
            .retain(|entry| active_counts.contains_key(&entry.key));

        let mut used_labels: BTreeSet<String> = self
            .entries
            .iter()
            .map(|entry| entry.label.clone())
            .collect();
        let mut used_color_indices: BTreeSet<usize> =
            self.entries.iter().map(|entry| entry.color_index).collect();

        let existing: BTreeSet<PlockVariantKey> =
            self.entries.iter().map(|entry| entry.key.clone()).collect();
        for key in current_step_keys.iter().flatten() {
            if existing.contains(key) || self.entries.iter().any(|entry| entry.key == *key) {
                continue;
            }
            let label_index = lowest_unused_label_index(&used_labels);
            let label = label_for_index(label_index);
            used_labels.insert(label.clone());

            let color_index = lowest_unused_color_index(&used_color_indices);
            used_color_indices.insert(color_index);
            let color = VARIANT_PALETTE[color_index % VARIANT_PALETTE.len()];

            self.entries.push(PlockVariantRegistryEntry {
                key: key.clone(),
                label,
                color,
                name: None,
                color_index,
            });
        }

        self.entries.sort_by(|a, b| {
            label_sort_index(&a.label)
                .cmp(&label_sort_index(&b.label))
                .then_with(|| a.key.cmp(&b.key))
        });

        self.previous_step_keys = current_step_keys.clone();
        let assignment_by_key: HashMap<PlockVariantKey, PlockVariantAssignment> = self
            .entries
            .iter()
            .map(|entry| (entry.key.clone(), PlockVariantAssignment::from(entry)))
            .collect();

        current_step_keys
            .into_iter()
            .map(|key| key.and_then(|key| assignment_by_key.get(&key).cloned()))
            .collect()
    }

    pub fn prune_to_keys(&mut self, keys: &[Option<PlockVariantKey>]) {
        let active: BTreeSet<PlockVariantKey> = keys.iter().flatten().cloned().collect();
        self.entries.retain(|entry| active.contains(&entry.key));
        self.previous_step_keys = keys.to_vec();
    }

    pub fn assignment_for_label(&self, label: &str) -> Option<PlockVariantAssignment> {
        self.entries
            .iter()
            .find(|entry| entry.label == label)
            .map(PlockVariantAssignment::from)
    }
}

fn lowest_unused_label_index(used: &BTreeSet<String>) -> usize {
    (0..)
        .find(|idx| !used.contains(&label_for_index(*idx)))
        .unwrap_or(0)
}

fn lowest_unused_color_index(used: &BTreeSet<usize>) -> usize {
    (0..).find(|idx| !used.contains(idx)).unwrap_or(0)
}

fn label_for_index(index: usize) -> String {
    let base = (b'A' + (index % 26) as u8) as char;
    let generation = index / 26;
    if generation == 0 {
        base.to_string()
    } else {
        format!("{base}{}", "'".repeat(generation))
    }
}

fn label_sort_index(label: &str) -> usize {
    let mut chars = label.chars();
    let Some(base) = chars.next() else {
        return usize::MAX;
    };
    if !base.is_ascii_uppercase() {
        return usize::MAX;
    }
    let generation = chars.filter(|ch| *ch == '\'').count();
    generation * 26 + (base as usize - 'A' as usize)
}

pub fn live_track_variant_keys(
    state: &SequencerState,
    track: usize,
) -> Vec<Option<PlockVariantKey>> {
    (0..MAX_STEPS)
        .map(|step| live_track_variant_key(state, track, step))
        .collect()
}

pub fn live_track_variant_key(
    state: &SequencerState,
    track: usize,
    step: usize,
) -> Option<PlockVariantKey> {
    if track >= state.pattern.instrument_slots.len() || step >= MAX_STEPS {
        return None;
    }

    let mut entries = Vec::new();
    collect_live_slot_entries(
        &mut entries,
        &state.pattern.instrument_slots[track],
        step,
        PlockVariantDomain::Instrument,
        PlockVariantDomain::InstrumentTensor,
        0,
    );

    if let Some(chain) = state.pattern.effect_chains.get(track) {
        for (slot_idx, slot) in chain.iter().enumerate() {
            collect_live_slot_entries(
                &mut entries,
                slot,
                step,
                PlockVariantDomain::Effect,
                PlockVariantDomain::EffectTensor,
                slot_idx,
            );
        }
    }

    if let Some(Some(rack)) = state.pattern.rack_tracks.lock().unwrap().get(track) {
        for (slot_idx, slot) in rack.slots.iter().enumerate() {
            collect_rack_slot_entries(&mut entries, slot, step, slot_idx);
        }
    }

    PlockVariantKey::new(entries)
}

pub fn live_track_key_lock_variant_keys(
    state: &SequencerState,
    track: usize,
) -> Vec<Option<PlockVariantKey>> {
    (0..MAX_MIDI_NOTES)
        .map(|note| live_track_key_lock_variant_key(state, track, note as u8))
        .collect()
}

pub fn live_track_key_lock_variant_key(
    state: &SequencerState,
    track: usize,
    note: u8,
) -> Option<PlockVariantKey> {
    let slot = state.pattern.instrument_slots.get(track)?;
    let mut entries = Vec::new();
    let num_params = (slot.num_params.load(Ordering::Relaxed) as usize).min(MAX_SLOT_PARAMS);
    if !slot.key_locks.note_has_any_lock(note, num_params) {
        return None;
    }
    for param_idx in 0..num_params {
        let Some(value) = slot.key_locks.get(note, param_idx) else {
            continue;
        };
        if slot.key_locks.get_id(note, param_idx) != slot.param_node_id(param_idx) {
            continue;
        }
        entries.push(PlockVariantEntry {
            domain: PlockVariantDomain::InstrumentKeyLock,
            slot: 0,
            param: param_idx,
            cell: None,
            value_bits: value.to_bits(),
        });
    }

    PlockVariantKey::new(entries)
}

fn collect_live_slot_entries(
    out: &mut Vec<PlockVariantEntry>,
    slot: &EffectSlotState,
    step: usize,
    scalar_domain: PlockVariantDomain,
    tensor_domain: PlockVariantDomain,
    slot_idx: usize,
) {
    let num_params = (slot.num_params.load(Ordering::Relaxed) as usize).min(MAX_SLOT_PARAMS);
    for param_idx in 0..num_params {
        if let Some(value) = slot.plocks.get(step, param_idx) {
            out.push(PlockVariantEntry {
                domain: scalar_domain,
                slot: slot_idx,
                param: param_idx,
                cell: None,
                value_bits: value.to_bits(),
            });
        }
    }

    for tensor_idx in 0..slot.tensor_params.num_params() {
        if let Some(values) = slot.tensor_params.plock_values(step, tensor_idx) {
            for (cell_idx, value) in values.into_iter().enumerate() {
                out.push(PlockVariantEntry {
                    domain: tensor_domain,
                    slot: slot_idx,
                    param: tensor_idx,
                    cell: Some(cell_idx),
                    value_bits: value.to_bits(),
                });
            }
        }
    }
}

fn collect_rack_slot_entries(
    out: &mut Vec<PlockVariantEntry>,
    slot: &RackSlotSnapshot,
    step: usize,
    slot_idx: usize,
) {
    for param in RackSlotParam::ALL {
        if let Some(value) = slot.param_plocks.get(step, param) {
            out.push(PlockVariantEntry {
                domain: PlockVariantDomain::RackSlotParam,
                slot: slot_idx,
                param: param.index(),
                cell: None,
                value_bits: value.to_bits(),
            });
        }
    }

    let num_params = slot.instrument_slot.num_params as usize;
    if let Some(step_plocks) = slot.instrument_slot.plocks.get(step) {
        for (param_idx, value) in step_plocks.iter().copied().take(num_params).enumerate() {
            if let Some(value) = value {
                out.push(PlockVariantEntry {
                    domain: PlockVariantDomain::RackSlotInstrument,
                    slot: slot_idx,
                    param: param_idx,
                    cell: None,
                    value_bits: value.to_bits(),
                });
            }
        }
    }

    for (tensor_idx, tensor) in slot.instrument_slot.tensor_params.iter().enumerate() {
        if let Some(Some(values)) = tensor.plocks.get(step) {
            for (cell_idx, value) in values.iter().copied().enumerate() {
                out.push(PlockVariantEntry {
                    domain: PlockVariantDomain::RackSlotInstrumentTensor,
                    slot: slot_idx,
                    param: tensor_idx,
                    cell: Some(cell_idx),
                    value_bits: value.to_bits(),
                });
            }
        }
    }
}

pub fn live_track_has_seq_lock(state: &SequencerState, track: usize, step: usize) -> bool {
    if track >= state.pattern.step_data.len() || step >= MAX_STEPS {
        return false;
    }

    StepParam::ALL.iter().any(|param| {
        state.pattern.step_data[track].get(step, *param).to_bits()
            != param.default_value().to_bits()
    }) || state.pattern.timebase_plocks[track].has_plock(step)
        || state.pattern.swing_plocks[track].has_plock(step)
        || state.pattern.swing_resolution_plocks[track].has_plock(step)
        || state
            .pattern
            .midi_fx_slots
            .get(track)
            .is_some_and(|slots| slots.iter().any(|slot| live_slot_has_plock(slot, step)))
}

fn live_slot_has_plock(slot: &EffectSlotState, step: usize) -> bool {
    let num_params = (slot.num_params.load(Ordering::Relaxed) as usize).min(MAX_SLOT_PARAMS);
    (0..num_params).any(|param_idx| slot.plocks.get(step, param_idx).is_some())
        || (0..slot.tensor_params.num_params())
            .any(|tensor_idx| slot.tensor_params.plock_values(step, tensor_idx).is_some())
}

pub fn snapshot_slot_has_step_plock(slot: &EffectSlotSnapshot, step: usize) -> bool {
    let num_params = slot.num_params as usize;
    slot.plocks
        .get(step)
        .is_some_and(|row| row.iter().take(num_params).any(|value| value.is_some()))
        || slot.tensor_params.iter().any(|tensor| {
            tensor
                .plocks
                .get(step)
                .is_some_and(|values| values.is_some())
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::effects::{EffectDescriptor, EffectSlotState};
    use crate::sequencer::{SequencerState, StepParam};

    fn test_key(value_bits: u32) -> PlockVariantKey {
        PlockVariantKey::new(vec![PlockVariantEntry {
            domain: PlockVariantDomain::Instrument,
            slot: 0,
            param: 0,
            cell: None,
            value_bits,
        }])
        .unwrap()
    }

    #[test]
    fn registry_migrates_singleton_edit_without_reassigning_label_or_color() {
        let first = test_key(0.25f32.to_bits());
        let edited = test_key(0.5f32.to_bits());
        let mut registry = PlockVariantRegistry::default();

        let first_assignment = registry.reconcile(vec![Some(first)]);
        let assigned = first_assignment[0].as_ref().unwrap().clone();
        assert_eq!(assigned.label, "A");

        let edited_assignment = registry.reconcile(vec![Some(edited.clone())]);
        let edited_assigned = edited_assignment[0].as_ref().unwrap();
        assert_eq!(edited_assigned.label, assigned.label);
        assert_eq!(edited_assigned.color, assigned.color);
        assert_eq!(registry.entries.len(), 1);
        assert_eq!(registry.entries[0].key, edited);
    }

    #[test]
    fn variant_key_excludes_seq_domain_step_expression() {
        let desc = EffectDescriptor::builtin_filter();
        let state = SequencerState::new(1, vec![vec![EffectSlotState::new(&desc, 1)]]);
        state.pattern.effect_chains[0][0].set_plock(0, 0, 220.0);
        state.pattern.effect_chains[0][0].set_plock(1, 0, 220.0);
        state.pattern.step_data[0].set(1, StepParam::Velocity, 0.5);

        let key_a = live_track_variant_key(&state, 0, 0).unwrap();
        let key_b = live_track_variant_key(&state, 0, 1).unwrap();
        assert_eq!(key_a, key_b);
        assert!(live_track_has_seq_lock(&state, 0, 1));
    }
}
