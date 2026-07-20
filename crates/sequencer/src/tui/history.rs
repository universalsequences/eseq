use std::collections::VecDeque;

use crate::plock_variants::PlockVariantRegistry;
use crate::sequencer::{StepCellSnapshot, StepSlotPlocks, TrackPatternId};

pub const DEFAULT_HISTORY_ENTRY_LIMIT: usize = 256;
pub const DEFAULT_HISTORY_BYTE_LIMIT: usize = 64 * 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ApplyMode {
    UserEdit,
    Undo,
    Redo,
    ProjectLoad,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HistoryPolicy {
    Record,
    Coalesce(MergeKey),
    Ignore,
    Barrier,
    Reset,
}

#[derive(Clone, Debug)]
pub enum EditPatch {
    StepCells(StepCellsPatch),
}

#[derive(Clone, Debug)]
pub struct StepCellsPatch {
    pub target: TrackPatternId,
    pub cells: Vec<StepCellDelta>,
    pub variant_registry_before: PlockVariantRegistry,
    pub variant_registry_after: PlockVariantRegistry,
}

#[derive(Clone, Debug)]
pub struct StepCellDelta {
    pub step: usize,
    pub before: StepCellSnapshot,
    pub after: StepCellSnapshot,
}

impl StepCellsPatch {
    pub fn retained_bytes(&self) -> usize {
        std::mem::size_of::<Self>()
            + self
                .cells
                .iter()
                .map(|cell| {
                    std::mem::size_of::<StepCellDelta>()
                        + step_snapshot_heap_bytes(&cell.before)
                        + step_snapshot_heap_bytes(&cell.after)
                })
                .sum::<usize>()
            + registry_heap_bytes(&self.variant_registry_before)
            + registry_heap_bytes(&self.variant_registry_after)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct MergeKey(String);

impl MergeKey {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct GestureId(pub u64);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ActiveGesture {
    pub id: GestureId,
    pub merge_key: MergeKey,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HistoryBudget {
    pub max_entries: usize,
    pub max_bytes: usize,
}

impl Default for HistoryBudget {
    fn default() -> Self {
        Self {
            max_entries: DEFAULT_HISTORY_ENTRY_LIMIT,
            max_bytes: DEFAULT_HISTORY_BYTE_LIMIT,
        }
    }
}

#[derive(Clone, Debug)]
pub struct HistoryEntry<P> {
    pub revision_before: u64,
    pub revision_after: u64,
    pub label: String,
    pub merge_key: Option<MergeKey>,
    pub patch: P,
    pub retained_bytes: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HistoryMove {
    pub label: String,
    pub revision: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HistoryReplay<E> {
    Unavailable,
    Applied(HistoryMove),
    Failed(E),
}

/// Session-local linear undo history.
///
/// Patch replay is supplied by the edit executor. An entry changes stacks only
/// after that replay succeeds, which makes a failed undo or redo non-destructive.
pub struct UndoManager<P> {
    undo: VecDeque<HistoryEntry<P>>,
    redo: Vec<HistoryEntry<P>>,
    current_revision: u64,
    next_revision: u64,
    saved_revision: Option<u64>,
    retained_bytes: usize,
    budget: HistoryBudget,
    active_gesture: Option<ActiveGesture>,
    newest_entry_exceeds_byte_budget: bool,
}

impl<P> Default for UndoManager<P> {
    fn default() -> Self {
        Self::new(HistoryBudget::default())
    }
}

impl<P> UndoManager<P> {
    pub fn new(budget: HistoryBudget) -> Self {
        Self {
            undo: VecDeque::new(),
            redo: Vec::new(),
            current_revision: 0,
            next_revision: 1,
            saved_revision: None,
            retained_bytes: 0,
            budget,
            active_gesture: None,
            newest_entry_exceeds_byte_budget: false,
        }
    }

    pub fn undo_len(&self) -> usize {
        self.undo.len()
    }

    pub fn redo_len(&self) -> usize {
        self.redo.len()
    }

    pub fn current_revision(&self) -> u64 {
        self.current_revision
    }

    pub fn saved_revision(&self) -> Option<u64> {
        self.saved_revision
    }

    pub fn retained_bytes(&self) -> usize {
        self.retained_bytes
    }

    pub fn active_gesture(&self) -> Option<&ActiveGesture> {
        self.active_gesture.as_ref()
    }

    pub fn newest_entry_exceeds_byte_budget(&self) -> bool {
        self.newest_entry_exceeds_byte_budget
    }

    pub fn mark_saved(&mut self) {
        self.saved_revision = Some(self.current_revision);
    }

    pub fn is_at_saved_revision(&self) -> bool {
        self.saved_revision == Some(self.current_revision)
    }

    pub fn begin_gesture(&mut self, gesture: ActiveGesture) -> Result<(), ActiveGesture> {
        if self.active_gesture.is_some() {
            return Err(gesture);
        }
        self.active_gesture = Some(gesture);
        Ok(())
    }

    pub fn finish_gesture(&mut self, id: GestureId) -> Option<ActiveGesture> {
        if self.active_gesture.as_ref().map(|gesture| gesture.id) != Some(id) {
            return None;
        }
        self.active_gesture.take()
    }

    pub fn commit(
        &mut self,
        label: impl Into<String>,
        merge_key: Option<MergeKey>,
        patch: P,
        retained_bytes: usize,
    ) -> HistoryMove {
        self.clear_redo();
        let revision_before = self.current_revision;
        let revision_after = self.take_revision();
        let label = label.into();
        self.undo.push_back(HistoryEntry {
            revision_before,
            revision_after,
            label: label.clone(),
            merge_key,
            patch,
            retained_bytes,
        });
        self.retained_bytes = self.retained_bytes.saturating_add(retained_bytes);
        self.current_revision = revision_after;
        self.enforce_budget(Some(revision_after));
        HistoryMove {
            label,
            revision: revision_after,
        }
    }

    pub fn undo<E>(
        &mut self,
        apply: impl FnOnce(&P) -> Result<(), E>,
    ) -> HistoryReplay<E> {
        let Some(entry) = self.undo.back() else {
            return HistoryReplay::Unavailable;
        };
        if let Err(error) = apply(&entry.patch) {
            return HistoryReplay::Failed(error);
        }
        let entry = self.undo.pop_back().expect("undo entry disappeared");
        self.current_revision = entry.revision_before;
        let result = HistoryMove {
            label: entry.label.clone(),
            revision: self.current_revision,
        };
        self.redo.push(entry);
        HistoryReplay::Applied(result)
    }

    pub fn redo<E>(
        &mut self,
        apply: impl FnOnce(&P) -> Result<(), E>,
    ) -> HistoryReplay<E> {
        let Some(entry) = self.redo.last() else {
            return HistoryReplay::Unavailable;
        };
        if let Err(error) = apply(&entry.patch) {
            return HistoryReplay::Failed(error);
        }
        let entry = self.redo.pop().expect("redo entry disappeared");
        self.current_revision = entry.revision_after;
        let result = HistoryMove {
            label: entry.label.clone(),
            revision: self.current_revision,
        };
        self.undo.push_back(entry);
        HistoryReplay::Applied(result)
    }

    /// Record a successful unsupported authoring mutation.
    pub fn barrier(&mut self) {
        self.clear_entries();
        self.active_gesture = None;
        self.current_revision = self.take_revision();
    }

    /// Reset history after a successful project replacement.
    pub fn reset(&mut self) {
        self.clear_entries();
        self.current_revision = 0;
        self.next_revision = 1;
        self.saved_revision = None;
        self.active_gesture = None;
    }

    fn take_revision(&mut self) -> u64 {
        let revision = self.next_revision;
        self.next_revision = self.next_revision.saturating_add(1);
        revision
    }

    fn clear_redo(&mut self) {
        for entry in self.redo.drain(..) {
            self.retained_bytes = self.retained_bytes.saturating_sub(entry.retained_bytes);
        }
    }

    fn clear_entries(&mut self) {
        self.undo.clear();
        self.redo.clear();
        self.retained_bytes = 0;
        self.newest_entry_exceeds_byte_budget = false;
    }

    fn enforce_budget(&mut self, protected_revision: Option<u64>) {
        self.newest_entry_exceeds_byte_budget = protected_revision
            .and_then(|revision| {
                self.undo
                    .back()
                    .filter(|entry| entry.revision_after == revision)
            })
            .is_some_and(|entry| entry.retained_bytes > self.budget.max_bytes);

        while self.undo.len() + self.redo.len() > self.budget.max_entries
            || self.retained_bytes > self.budget.max_bytes
        {
            let undo_revision = self.undo.front().map(|entry| entry.revision_after);
            let redo_revision = self.redo.last().map(|entry| entry.revision_after);
            let remove_undo = match (undo_revision, redo_revision) {
                (Some(undo_revision), Some(redo_revision)) => undo_revision <= redo_revision,
                (Some(_), None) => true,
                (None, Some(_)) => false,
                (None, None) => break,
            };
            let candidate_revision = if remove_undo {
                undo_revision
            } else {
                redo_revision
            };
            if candidate_revision == protected_revision {
                break;
            }
            let entry = if remove_undo {
                self.undo.pop_front().expect("oldest undo entry disappeared")
            } else {
                self.redo.pop().expect("oldest redo entry disappeared")
            };
            self.retained_bytes = self.retained_bytes.saturating_sub(entry.retained_bytes);
        }
    }
}

pub fn bit_exact_f32_eq(left: f32, right: f32) -> bool {
    left.to_bits() == right.to_bits()
}

fn bit_exact_f32_slice_eq(left: &[f32], right: &[f32]) -> bool {
    left.len() == right.len()
        && left
            .iter()
            .zip(right)
            .all(|(left, right)| bit_exact_f32_eq(*left, *right))
}

fn optional_f32_eq(left: Option<f32>, right: Option<f32>) -> bool {
    match (left, right) {
        (Some(left), Some(right)) => bit_exact_f32_eq(left, right),
        (None, None) => true,
        _ => false,
    }
}

fn slot_plocks_eq(left: &StepSlotPlocks, right: &StepSlotPlocks) -> bool {
    left.params.len() == right.params.len()
        && left
            .params
            .iter()
            .zip(&right.params)
            .all(|(left, right)| optional_f32_eq(*left, *right))
        && left.tensor_params.len() == right.tensor_params.len()
        && left
            .tensor_params
            .iter()
            .zip(&right.tensor_params)
            .all(|(left, right)| match (left, right) {
                (Some(left), Some(right)) => bit_exact_f32_slice_eq(left, right),
                (None, None) => true,
                _ => false,
            })
}

fn slot_plock_slice_eq(left: &[StepSlotPlocks], right: &[StepSlotPlocks]) -> bool {
    left.len() == right.len()
        && left
            .iter()
            .zip(right)
            .all(|(left, right)| slot_plocks_eq(left, right))
}

pub fn step_snapshot_bit_exact_eq(
    left: &StepCellSnapshot,
    right: &StepCellSnapshot,
) -> bool {
    left.active == right.active
        && left.neural_reset == right.neural_reset
        && bit_exact_f32_slice_eq(&left.params, &right.params)
        && bit_exact_f32_slice_eq(&left.chord, &right.chord)
        && bit_exact_f32_slice_eq(&left.chord_durations, &right.chord_durations)
        && bit_exact_f32_slice_eq(&left.chord_delays, &right.chord_delays)
        && left.timebase == right.timebase
        && optional_f32_eq(left.swing, right.swing)
        && left.swing_resolution == right.swing_resolution
        && slot_plock_slice_eq(&left.midi_fx_plocks, &right.midi_fx_plocks)
        && slot_plock_slice_eq(&left.effect_plocks, &right.effect_plocks)
        && slot_plocks_eq(&left.instrument_plocks, &right.instrument_plocks)
        && left.rack_macro_plocks.len() == right.rack_macro_plocks.len()
        && left
            .rack_macro_plocks
            .iter()
            .zip(&right.rack_macro_plocks)
            .all(|(left, right)| optional_f32_eq(*left, *right))
        && slot_plock_slice_eq(
            &left.rack_slot_param_plocks,
            &right.rack_slot_param_plocks,
        )
        && slot_plock_slice_eq(
            &left.rack_slot_instrument_plocks,
            &right.rack_slot_instrument_plocks,
        )
        && left.rack_slot_effect_plocks.len() == right.rack_slot_effect_plocks.len()
        && left
            .rack_slot_effect_plocks
            .iter()
            .zip(&right.rack_slot_effect_plocks)
            .all(|(left, right)| slot_plock_slice_eq(left, right))
}

pub fn registry_bit_exact_eq(
    left: &PlockVariantRegistry,
    right: &PlockVariantRegistry,
) -> bool {
    left.previous_step_keys == right.previous_step_keys
        && left.entries.len() == right.entries.len()
        && left.entries.iter().zip(&right.entries).all(|(left, right)| {
            left.key == right.key
                && left.label == right.label
                && left.name == right.name
                && left.color_index == right.color_index
                && bit_exact_f32_slice_eq(&left.color, &right.color)
        })
}

fn slot_plocks_heap_bytes(plocks: &StepSlotPlocks) -> usize {
    plocks.params.capacity() * std::mem::size_of::<Option<f32>>()
        + plocks.tensor_params.capacity() * std::mem::size_of::<Option<Vec<f32>>>()
        + plocks
            .tensor_params
            .iter()
            .flatten()
            .map(|values| values.capacity() * std::mem::size_of::<f32>())
            .sum::<usize>()
}

fn step_snapshot_heap_bytes(snapshot: &StepCellSnapshot) -> usize {
    let slot_slice_bytes = |slots: &Vec<StepSlotPlocks>| {
        slots.capacity() * std::mem::size_of::<StepSlotPlocks>()
            + slots.iter().map(slot_plocks_heap_bytes).sum::<usize>()
    };
    snapshot.chord.capacity() * std::mem::size_of::<f32>()
        + snapshot.chord_durations.capacity() * std::mem::size_of::<f32>()
        + snapshot.chord_delays.capacity() * std::mem::size_of::<f32>()
        + slot_slice_bytes(&snapshot.midi_fx_plocks)
        + slot_slice_bytes(&snapshot.effect_plocks)
        + slot_plocks_heap_bytes(&snapshot.instrument_plocks)
        + snapshot.rack_macro_plocks.capacity() * std::mem::size_of::<Option<f32>>()
        + slot_slice_bytes(&snapshot.rack_slot_param_plocks)
        + slot_slice_bytes(&snapshot.rack_slot_instrument_plocks)
        + snapshot.rack_slot_effect_plocks.capacity()
            * std::mem::size_of::<Vec<StepSlotPlocks>>()
        + snapshot
            .rack_slot_effect_plocks
            .iter()
            .map(|slots| slot_slice_bytes(slots))
            .sum::<usize>()
}

fn registry_heap_bytes(registry: &PlockVariantRegistry) -> usize {
    registry.entries.capacity()
        * std::mem::size_of::<crate::plock_variants::PlockVariantRegistryEntry>()
        + registry
            .entries
            .iter()
            .map(|entry| {
                entry.key.entries.capacity()
                    * std::mem::size_of::<crate::plock_variants::PlockVariantEntry>()
                    + entry.label.capacity()
                    + entry.name.as_ref().map(String::capacity).unwrap_or(0)
            })
            .sum::<usize>()
        + registry.previous_step_keys.capacity()
            * std::mem::size_of::<Option<crate::plock_variants::PlockVariantKey>>()
        + registry
            .previous_step_keys
            .iter()
            .flatten()
            .map(|key| {
                key.entries.capacity()
                    * std::mem::size_of::<crate::plock_variants::PlockVariantEntry>()
            })
            .sum::<usize>()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manager(max_entries: usize, max_bytes: usize) -> UndoManager<i32> {
        UndoManager::new(HistoryBudget {
            max_entries,
            max_bytes,
        })
    }

    #[test]
    fn stack_transitions_preserve_failed_replay_and_clear_redo_on_new_edit() {
        let mut history = manager(8, 1024);
        history.commit("one", None, 1, 10);
        history.commit("two", None, 2, 20);

        assert_eq!(history.undo(|_| Err("rejected")), HistoryReplay::Failed("rejected"));
        assert_eq!((history.undo_len(), history.redo_len()), (2, 0));
        assert_eq!(history.current_revision(), 2);

        assert!(matches!(history.undo(|_| Ok::<_, ()>(())), HistoryReplay::Applied(_)));
        assert_eq!((history.undo_len(), history.redo_len()), (1, 1));
        assert_eq!(history.current_revision(), 1);

        history.commit("three", None, 3, 30);
        assert_eq!((history.undo_len(), history.redo_len()), (2, 0));
        assert_eq!(history.retained_bytes(), 40);
        assert_eq!(history.redo(|_| Ok::<_, ()>(())), HistoryReplay::Unavailable);
    }

    #[test]
    fn entry_and_byte_budgets_evict_oldest_but_keep_oversized_newest_entry() {
        let mut history = manager(2, 50);
        history.commit("one", None, 1, 20);
        history.commit("two", None, 2, 20);
        history.commit("three", None, 3, 20);
        assert_eq!(history.undo_len(), 2);
        assert_eq!(history.retained_bytes(), 40);

        history.commit("oversized", None, 4, 80);
        assert_eq!(history.undo_len(), 1);
        assert_eq!(history.retained_bytes(), 80);
        assert!(history.newest_entry_exceeds_byte_budget());
    }

    #[test]
    fn barrier_and_project_reset_advance_or_restart_revision_lifetime() {
        let mut history = manager(8, 1024);
        history.commit("edit", None, 1, 10);
        history.mark_saved();
        history
            .begin_gesture(ActiveGesture {
                id: GestureId(7),
                merge_key: MergeKey::new("step-drag"),
            })
            .expect("start gesture");
        assert!(history.is_at_saved_revision());

        history.barrier();
        assert_eq!((history.undo_len(), history.redo_len()), (0, 0));
        assert_eq!(history.current_revision(), 2);
        assert!(!history.is_at_saved_revision());
        assert_eq!(history.active_gesture(), None);

        history.reset();
        assert_eq!(history.current_revision(), 0);
        assert_eq!(history.saved_revision(), None);
    }

    #[test]
    fn float_equality_is_bit_exact_for_signed_zero_and_nan_payloads() {
        assert!(!bit_exact_f32_eq(0.0, -0.0));
        let first_nan = f32::from_bits(0x7fc0_0001);
        let same_nan = f32::from_bits(0x7fc0_0001);
        let other_nan = f32::from_bits(0x7fc0_0002);
        assert!(bit_exact_f32_eq(first_nan, same_nan));
        assert!(!bit_exact_f32_eq(first_nan, other_nan));
    }
}
