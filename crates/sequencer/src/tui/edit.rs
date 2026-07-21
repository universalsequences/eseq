use crate::macro_engine::{Macro, MacroMapping};
use crate::effects::EffectSlotSnapshot;
use crate::plock_variants::PlockVariantRegistry;
use crate::sequencer::{
    BusId, StepCellSnapshot, TrackId, TrackParamsSnapshot, TrackPatternId, MAX_STEPS,
};
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

use super::command::{history_policy, sanitize_pasted_step_snapshot, AppCommand};
use super::history::{
    step_snapshot_bit_exact_eq, ActiveGesture, ApplyMode, BusMixerPatch, BusMixerSnapshot,
    EditPatch, GestureId, HistoryMove, HistoryPolicy, HistoryReplay, MergeKey,
    PatternGeometryPatch, StepCellDelta, StepCellsPatch, TrackParamsBatchPatch, TrackParamsPatch,
    TransportAuthoringSnapshot, TransportParamsPatch,
};
use super::App;

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
    let policy = history_policy(&cmd);
    if matches!(policy, HistoryPolicy::Record) {
        finish_active_gesture(app);
    }
    match policy {
        HistoryPolicy::Record if is_pattern_geometry_command(&cmd) => {
            apply_recorded_pattern_geometry_command(app, &cmd)
        }
        HistoryPolicy::Record if track_params_command_track(&cmd).is_some() => {
            apply_recorded_track_params_command(app, &cmd, None)
        }
        HistoryPolicy::Record if bus_mixer_command_bus(&cmd).is_some() => {
            apply_recorded_bus_mixer_command(app, &cmd, None)
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
    let mut history = std::mem::take(&mut app.history);
    let result = history.undo(|patch| replay_patch(app, patch, ApplyMode::Undo));
    app.history = history;
    result
}

pub fn redo(app: &mut App) -> HistoryReplay<EditError> {
    finish_active_gesture(app);
    let mut history = std::mem::take(&mut app.history);
    let result = history.redo(|patch| replay_patch(app, patch, ApplyMode::Redo));
    app.history = history;
    result
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
