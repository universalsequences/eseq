use std::collections::VecDeque;
use std::time::{Duration, Instant};

use crate::plock_variants::PlockVariantRegistry;
use crate::sequencer::{
    BusId, StepCellSnapshot, StepSlotPlocks, TrackParamsSnapshot, TrackPatternId,
};

pub const DEFAULT_HISTORY_ENTRY_LIMIT: usize = 256;
pub const DEFAULT_HISTORY_BYTE_LIMIT: usize = 64 * 1024 * 1024;
pub const FALLBACK_GESTURE_IDLE_TIMEOUT: Duration = Duration::from_millis(250);

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
    PatternGeometry(PatternGeometryPatch),
    TrackParams(TrackParamsPatch),
    TrackParamsBatch(TrackParamsBatchPatch),
    BusMixer(BusMixerPatch),
    TransportParams(TransportParamsPatch),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BusMixerSnapshot {
    pub volume_bits: u32,
    pub mute: bool,
    pub solo: bool,
}

#[derive(Clone, Debug)]
pub struct BusMixerPatch {
    pub target: BusId,
    pub before: BusMixerSnapshot,
    pub after: BusMixerSnapshot,
}

impl BusMixerPatch {
    pub fn retained_bytes(&self) -> usize {
        std::mem::size_of::<Self>()
    }
}

#[derive(Clone, Debug)]
pub struct TrackParamsBatchPatch {
    pub tracks: Vec<TrackParamsPatch>,
}

impl TrackParamsBatchPatch {
    pub fn retained_bytes(&self) -> usize {
        std::mem::size_of::<Self>()
            + self.tracks.capacity() * std::mem::size_of::<TrackParamsPatch>()
            + self
                .tracks
                .iter()
                .map(|patch| {
                    patch
                        .retained_bytes()
                        .saturating_sub(std::mem::size_of::<TrackParamsPatch>())
                })
                .sum::<usize>()
    }
}

#[derive(Clone, Debug)]
pub struct TrackParamsPatch {
    pub target: TrackPatternId,
    pub before: TrackParamsSnapshot,
    pub after: TrackParamsSnapshot,
    pub instrument_base_note_offset_before: u32,
    pub instrument_base_note_offset_after: u32,
}

impl TrackParamsPatch {
    pub fn retained_bytes(&self) -> usize {
        std::mem::size_of::<Self>()
            + track_params_heap_bytes(&self.before)
            + track_params_heap_bytes(&self.after)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TransportAuthoringSnapshot {
    pub bpm: u32,
    pub master_volume_bits: u32,
}

#[derive(Clone, Debug)]
pub struct TransportParamsPatch {
    pub before: TransportAuthoringSnapshot,
    pub after: TransportAuthoringSnapshot,
}

impl TransportParamsPatch {
    pub fn retained_bytes(&self) -> usize {
        std::mem::size_of::<Self>()
    }
}

#[derive(Clone, Debug)]
pub struct PatternGeometryPatch {
    pub target: TrackPatternId,
    pub num_steps_before: usize,
    pub num_steps_after: usize,
    pub cells: StepCellsPatch,
}

impl PatternGeometryPatch {
    pub fn retained_bytes(&self) -> usize {
        std::mem::size_of::<Self>() + self.cells.retained_bytes()
    }
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

struct PendingGesture<P> {
    label: String,
    merge_key: MergeKey,
    patch: P,
    retained_bytes: usize,
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
    active_gesture_updated_at: Option<Instant>,
    pending_gesture: Option<PendingGesture<P>>,
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
            active_gesture_updated_at: None,
            pending_gesture: None,
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
        self.pending_gesture.is_none() && self.saved_revision == Some(self.current_revision)
    }

    pub fn begin_gesture(&mut self, gesture: ActiveGesture) -> Result<(), ActiveGesture> {
        if self.active_gesture.is_some() {
            return Err(gesture);
        }
        self.active_gesture = Some(gesture);
        self.active_gesture_updated_at = Some(Instant::now());
        Ok(())
    }

    pub fn finish_gesture(&mut self, id: GestureId) -> Option<ActiveGesture> {
        if self.active_gesture.as_ref().map(|gesture| gesture.id) != Some(id) {
            return None;
        }
        self.finish_active_gesture()
    }

    pub fn finish_active_gesture(&mut self) -> Option<ActiveGesture> {
        self.active_gesture_updated_at = None;
        let gesture = self.active_gesture.take();
        if let Some(pending) = self.pending_gesture.take() {
            self.commit(
                pending.label,
                Some(pending.merge_key),
                pending.patch,
                pending.retained_bytes,
            );
        }
        gesture
    }

    pub fn finish_active_gesture_if_idle(&mut self, idle_for: Duration) -> Option<ActiveGesture> {
        if !self.active_gesture_is_idle(idle_for) {
            return None;
        }
        self.finish_active_gesture()
    }

    pub fn active_gesture_is_idle(&self, idle_for: Duration) -> bool {
        self.active_gesture_updated_at
            .is_some_and(|updated| updated.elapsed() >= idle_for)
    }

    pub fn active_gesture_patch(&self, merge_key: &MergeKey) -> Option<&P> {
        if self.active_gesture.as_ref().map(|gesture| &gesture.merge_key) != Some(merge_key) {
            return None;
        }
        self.pending_gesture
            .as_ref()
            .filter(|pending| &pending.merge_key == merge_key)
            .map(|pending| &pending.patch)
    }

    pub fn stage_active_gesture(
        &mut self,
        label: impl Into<String>,
        merge_key: &MergeKey,
        patch: P,
        retained_bytes: usize,
    ) -> Option<HistoryMove> {
        if self.active_gesture.as_ref().map(|gesture| &gesture.merge_key) != Some(merge_key) {
            return None;
        }
        let label = label.into();
        self.pending_gesture = Some(PendingGesture {
            label: label.clone(),
            merge_key: merge_key.clone(),
            patch,
            retained_bytes,
        });
        self.active_gesture_updated_at = Some(Instant::now());
        Some(HistoryMove {
            label,
            revision: self.current_revision,
        })
    }

    pub fn discard_active_gesture_entry(&mut self, merge_key: &MergeKey) -> bool {
        if self.active_gesture.as_ref().map(|gesture| &gesture.merge_key) != Some(merge_key)
            || self
                .pending_gesture
                .as_ref()
                .map(|pending| &pending.merge_key)
                != Some(merge_key)
        {
            return false;
        }
        self.pending_gesture = None;
        self.active_gesture = None;
        self.active_gesture_updated_at = None;
        true
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
        self.active_gesture_updated_at = None;
        self.pending_gesture = None;
        self.current_revision = self.take_revision();
    }

    /// Reset history after a successful project replacement.
    pub fn reset(&mut self) {
        self.clear_entries();
        self.current_revision = 0;
        self.next_revision = 1;
        self.saved_revision = None;
        self.active_gesture = None;
        self.active_gesture_updated_at = None;
        self.pending_gesture = None;
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

fn track_params_heap_bytes(snapshot: &TrackParamsSnapshot) -> usize {
    snapshot.sends.capacity() * std::mem::size_of::<crate::sequencer::TrackSendSnapshot>()
        + snapshot
            .script_accumulator_name
            .as_ref()
            .map(String::capacity)
            .unwrap_or(0)
        + snapshot.midi_fx_chain.capacity() * std::mem::size_of::<String>()
        + snapshot
            .midi_fx_chain
            .iter()
            .map(String::capacity)
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
    fn fallback_idle_boundary_finishes_sources_without_end_events() {
        let mut history = manager(8, 1024);
        history
            .begin_gesture(ActiveGesture {
                id: GestureId(9),
                merge_key: MergeKey::new("wheel-volume"),
            })
            .expect("begin wheel gesture");
        assert!(history
            .finish_active_gesture_if_idle(Duration::ZERO)
            .is_some());
        assert_eq!(history.active_gesture(), None);
    }

    #[test]
    fn staged_gesture_commits_once_at_end_and_marks_pending_state_dirty() {
        let mut history = manager(8, 1024);
        let key = MergeKey::new("volume-drag");
        history
            .begin_gesture(ActiveGesture {
                id: GestureId(10),
                merge_key: key.clone(),
            })
            .expect("begin volume gesture");
        assert!(history
            .stage_active_gesture("Volume", &key, 1, 8)
            .is_some());
        assert!(history
            .stage_active_gesture("Volume", &key, 2, 8)
            .is_some());
        assert_eq!(history.undo_len(), 0);
        history.finish_active_gesture();
        assert_eq!(history.undo_len(), 1);
        history.mark_saved();
        assert!(history.is_at_saved_revision());

        history
            .begin_gesture(ActiveGesture {
                id: GestureId(11),
                merge_key: key.clone(),
            })
            .expect("begin second volume gesture");
        history.stage_active_gesture("Volume", &key, 3, 8);
        assert!(!history.is_at_saved_revision());
        history.finish_active_gesture();
        assert_eq!(history.undo_len(), 2);
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
