use crate::macro_engine::{Macro, MacroMapping};
use crate::effects::EffectSlotSnapshot;
use crate::plock_variants::PlockVariantRegistry;
use crate::sequencer::{
    StepCellSnapshot, TrackId, TrackParamsSnapshot, TrackPatternId, MAX_STEPS,
};
use std::collections::BTreeMap;
use std::time::Instant;

use super::command::{history_policy, sanitize_pasted_step_snapshot, AppCommand};
use super::history::{
    step_snapshot_bit_exact_eq, ApplyMode, EditPatch, HistoryMove, HistoryPolicy, HistoryReplay,
    StepCellDelta, StepCellsPatch,
};
use super::App;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EditError {
    UnsupportedCommand,
    TrackOutOfRange { track: usize },
    MissingStableTrack { track: TrackId },
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
    bytes.f32(snapshot.accum_limit);
    bytes.u32(snapshot.accum_mode);
    bytes.usize(snapshot.fts_scale);
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
        AppCommand::SetInstrumentParam {
            track, param_idx, ..
        }
        | AppCommand::SetInstrumentPlock {
            track, param_idx, ..
        }
        | AppCommand::SetInstrumentPlockMulti {
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
        | AppCommand::SetRackSlotParamPlockMulti { track, steps, .. }
        | AppCommand::SetRackSlotInstrumentPlockMulti { track, steps, .. } => {
            capture_step_witness(app, *track, normalized_steps(steps), false)
        }

        AppCommand::ToggleTrackGate { track }
        | AppCommand::ToggleTrackPolyphonic { track }
        | AppCommand::AdjustTrackMaxPolyphony { track, .. }
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
        | AppCommand::SetTrackAccumMode { track, .. } => {
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
        | AppCommand::TogglePlay => Err(EditError::UnsupportedCommand),
    }
}

pub fn commit_history_barrier(app: &mut App) {
    let cleared_entries = app.history.undo_len() + app.history.redo_len();
    app.history.barrier();
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
}

impl ResolvedStepCommand<'_> {
    fn affected_steps(&self) -> &[usize] {
        match self {
            Self::Toggle { step }
            | Self::SetActive { step, .. }
            | Self::SetParam { step, .. }
            | Self::AdjustParam { step, .. }
            | Self::TogglePianoNote { step, .. } => std::slice::from_ref(step),
            Self::Clear { steps } | Self::Rotate { steps, .. } => steps,
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
    let history_move = app.history.commit(
        label,
        None,
        EditPatch::StepCells(patch),
        retained_bytes,
    );
    Ok(EditOutcome::Applied(history_move))
}

pub fn try_apply_command(app: &mut App, cmd: AppCommand) -> Result<EditOutcome, EditError> {
    match history_policy(&cmd) {
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

fn replay_patch(app: &mut App, patch: &EditPatch, mode: ApplyMode) -> Result<(), EditError> {
    match patch {
        EditPatch::StepCells(patch) => replay_step_patch(app, patch, mode).map(|_| ()),
    }
}

pub fn undo(app: &mut App) -> HistoryReplay<EditError> {
    let mut history = std::mem::take(&mut app.history);
    let result = history.undo(|patch| replay_patch(app, patch, ApplyMode::Undo));
    app.history = history;
    result
}

pub fn redo(app: &mut App) -> HistoryReplay<EditError> {
    let mut history = std::mem::take(&mut app.history);
    let result = history.redo(|patch| replay_patch(app, patch, ApplyMode::Redo));
    app.history = history;
    result
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use super::*;
    use crate::audiograph::LiveGraphPtr;
    use crate::recorder::MasterRecorder;
    use crate::sequencer::{
        default_empty_effect_chain, InstrumentType, PatternSnapshot, SequencerState, Timebase,
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
        assert_eq!(
            try_apply_command(
                &mut app,
                AppCommand::SetTrackAttack {
                    track: 0,
                    ms: 12.0,
                },
            ),
            Ok(EditOutcome::AppliedUnrecorded)
        );
        assert_eq!((app.history.undo_len(), app.history.redo_len()), (0, 0));
        assert_eq!(app.state.scheduler_snapshot_version(), version + 1);
        assert!(app
            .editor
            .status_message
            .as_ref()
            .is_some_and(|(message, _)| message.starts_with("Undo history cleared")));
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
}
