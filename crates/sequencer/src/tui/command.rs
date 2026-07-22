//! `AppCommand` — unified mutation boundary for all sequencer state changes.
//!
//! All mutations to `app.state.pattern.*` or `app.state.transport.*` that need
//! to be visible to the audio thread should go through `apply_command`.  After
//! executing a command that affects event scheduling, `apply_command` publishes
//! a scheduler snapshot. Continuous live controls that are pushed directly to
//! the audio graph intentionally skip that publish path.
//!
//! Pure UI-state changes (cursor movement, mode changes, etc.) can also be
//! routed through `apply_command` for uniformity — they just don't trigger a
//! publish.

use std::sync::atomic::Ordering;
use std::time::Instant;

use crate::effects::HostControl;
use crate::macro_engine::{MacroCurve, MacroId};
use crate::plock_variants::{PlockVariantDomain, PlockVariantKey};
use crate::process::ParamTarget;
use crate::sequencer::{
    BusId, RackSlotParam, StepParam, StepSnapshot, SwingResolution, Timebase, TrackOutput,
    TrackSendSnapshot,
};

use super::App;

fn sync_instrument_mod_active_default(app: &mut App, track: usize, changed_param_idx: usize) {
    let Some(desc) = app.graph.instrument_descriptors.get(track) else {
        return;
    };
    let active_param_idx = desc
        .instrument_modulation_targets
        .iter()
        .find(|target| target.depth_param_idx == changed_param_idx)
        .and_then(|target| target.active_param_idx);
    let Some(active_param_idx) = active_param_idx else {
        return;
    };
    let active = desc
        .instrument_modulation_targets
        .iter()
        .filter(|target| target.active_param_idx == Some(active_param_idx))
        .any(|target| {
            app.state.pattern.instrument_slots[track]
                .defaults
                .get(target.depth_param_idx)
                .abs()
                > f32::EPSILON
        });
    let value = if active { 1.0 } else { 0.0 };
    let slot = &app.state.pattern.instrument_slots[track];
    slot.defaults.set(active_param_idx, value);
    app.send_instrument_param(track, active_param_idx, value);
}

fn sync_instrument_mod_active_plock(
    app: &mut App,
    track: usize,
    step: usize,
    changed_param_idx: usize,
) {
    let Some(desc) = app.graph.instrument_descriptors.get(track) else {
        return;
    };
    let active_param_idx = desc
        .instrument_modulation_targets
        .iter()
        .find(|target| target.depth_param_idx == changed_param_idx)
        .and_then(|target| target.active_param_idx);
    let Some(active_param_idx) = active_param_idx else {
        return;
    };
    let slot = &app.state.pattern.instrument_slots[track];
    let targets = desc
        .instrument_modulation_targets
        .iter()
        .filter(|target| target.active_param_idx == Some(active_param_idx))
        .collect::<Vec<_>>();
    if !targets
        .iter()
        .any(|target| slot.plocks.get(step, target.depth_param_idx).is_some())
    {
        slot.plocks.clear_param(step, active_param_idx);
        return;
    }
    let active = targets.iter().any(|target| {
            slot.plocks
                .get(step, target.depth_param_idx)
                .unwrap_or_else(|| slot.defaults.get(target.depth_param_idx))
                .abs()
                > f32::EPSILON
        });
    slot.set_plock(step, active_param_idx, if active { 1.0 } else { 0.0 });
}

fn sync_instrument_mod_active_key_lock(
    app: &mut App,
    track: usize,
    note: u8,
    changed_param_idx: usize,
) {
    let Some(desc) = app.graph.instrument_descriptors.get(track) else {
        return;
    };
    let active_param_idx = desc
        .instrument_modulation_targets
        .iter()
        .find(|target| target.depth_param_idx == changed_param_idx)
        .and_then(|target| target.active_param_idx);
    let Some(active_param_idx) = active_param_idx else {
        return;
    };
    let slot = &app.state.pattern.instrument_slots[track];
    let targets = desc
        .instrument_modulation_targets
        .iter()
        .filter(|target| target.active_param_idx == Some(active_param_idx))
        .collect::<Vec<_>>();
    if !targets
        .iter()
        .any(|target| slot.key_locks.get(note, target.depth_param_idx).is_some())
    {
        slot.key_locks.clear_param(note, active_param_idx);
        return;
    }
    let active = targets.iter().any(|target| {
            slot.key_locks
                .get(note, target.depth_param_idx)
                .unwrap_or_else(|| slot.defaults.get(target.depth_param_idx))
                .abs()
                > f32::EPSILON
        });
    slot.set_key_lock(note, active_param_idx, if active { 1.0 } else { 0.0 });
}

fn effect_mod_active_param_idx(
    desc: &crate::effects::EffectDescriptor,
    changed_param_idx: usize,
) -> Option<usize> {
    desc.instrument_modulation_targets
        .iter()
        .find(|target| target.depth_param_idx == changed_param_idx)
        .and_then(|target| target.active_param_idx)
}

fn sync_effect_mod_active_default(
    app: &mut App,
    track: usize,
    slot_idx: usize,
    changed_param_idx: usize,
) {
    let Some(desc) = app
        .graph
        .effect_descriptors
        .get(track)
        .and_then(|track_descs| track_descs.get(slot_idx))
    else {
        return;
    };
    let Some(active_param_idx) = effect_mod_active_param_idx(desc, changed_param_idx) else {
        return;
    };
    let Some(slot) = app
        .state
        .pattern
        .effect_chains
        .get(track)
        .and_then(|chain| chain.get(slot_idx))
    else {
        return;
    };
    let active = desc
        .instrument_modulation_targets
        .iter()
        .filter(|target| target.active_param_idx == Some(active_param_idx))
        .any(|target| slot.defaults.get(target.depth_param_idx).abs() > f32::EPSILON);
    let value = if active { 1.0 } else { 0.0 };
    slot.defaults.set(active_param_idx, value);
    app.send_slot_param(track, slot_idx, active_param_idx, value);
}

fn sync_effect_mod_active_plock(
    app: &mut App,
    track: usize,
    step: usize,
    slot_idx: usize,
    changed_param_idx: usize,
) {
    let Some(desc) = app
        .graph
        .effect_descriptors
        .get(track)
        .and_then(|track_descs| track_descs.get(slot_idx))
    else {
        return;
    };
    let Some(active_param_idx) = effect_mod_active_param_idx(desc, changed_param_idx) else {
        return;
    };
    let Some(slot) = app
        .state
        .pattern
        .effect_chains
        .get(track)
        .and_then(|chain| chain.get(slot_idx))
    else {
        return;
    };
    let targets = desc
        .instrument_modulation_targets
        .iter()
        .filter(|target| target.active_param_idx == Some(active_param_idx))
        .collect::<Vec<_>>();
    if !targets
        .iter()
        .any(|target| slot.plocks.get(step, target.depth_param_idx).is_some())
    {
        slot.plocks.clear_param(step, active_param_idx);
        return;
    }
    let active = targets.iter().any(|target| {
            slot.plocks
                .get(step, target.depth_param_idx)
                .unwrap_or_else(|| slot.defaults.get(target.depth_param_idx))
                .abs()
                > f32::EPSILON
        });
    slot.set_plock(step, active_param_idx, if active { 1.0 } else { 0.0 });
}

pub(crate) fn sanitize_pasted_step_snapshot(
    snapshot: &StepSnapshot,
    preserve_audio_plocks: bool,
) -> StepSnapshot {
    if preserve_audio_plocks {
        snapshot.clone()
    } else {
        snapshot.without_audio_plocks()
    }
}

/// Every mutation the UI layer can make to sequencer or transport state.
///
/// Variants are grouped loosely:
///   - Pattern / step mutations  (always publish)
///   - Track params              (always publish; some also push to audio graph)
///   - Effect params             (always publish; some also push to audio graph)
///   - Instrument params         (always publish; some also push to audio graph)
///   - Reverb                    (no publish needed — push only, no snapshot)
///   - Transport                 (always publish)
///   - Pure UI                   (no publish)
#[allow(dead_code)]
#[derive(Clone)]
pub enum AppCommand {
    // ── Pattern / step mutations ──────────────────────────────────────────────
    /// Toggle a step on/off and clear its plocks if it was active.
    ToggleStep {
        track: usize,
        step: usize,
    },

    /// Explicitly set a step's active flag.
    SetStepActive {
        track: usize,
        step: usize,
        active: bool,
    },

    /// Set one parameter value on a step.
    SetStepParam {
        track: usize,
        step: usize,
        param: StepParam,
        value: f32,
    },

    /// Adjust one parameter value on a step by a delta.
    AdjustStepParam {
        track: usize,
        step: usize,
        param: StepParam,
        delta: f32,
    },

    /// Clear all payload (params, active flag, plocks) for a step.
    ClearStepPayload {
        track: usize,
        step: usize,
    },

    /// Clear payload for multiple steps.
    ClearSteps {
        track: usize,
        steps: Vec<usize>,
    },

    /// Rotate steps cyclically left (-1) or right (+1).
    RotateSteps {
        track: usize,
        steps: Vec<usize>,
        direction: isize,
    },

    /// Paste clipboard snapshots into destination positions.
    PasteSteps {
        track: usize,
        source_track: usize,
        /// (relative_offset_from_dest_start, snapshot)
        clipboard: Vec<(usize, StepSnapshot)>,
        dest_start: usize,
        num_steps: usize,
    },

    /// Shift a contiguous range of steps by `direction` positions, clearing
    /// the vacated slots.
    ShiftStepRange {
        track: usize,
        lo: usize,
        hi: usize,
        new_lo: usize,
    },

    /// Toggle one piano-roll note while preserving the compact single-note
    /// representation in `StepParam::Transpose`.
    TogglePianoNote {
        track: usize,
        step: usize,
        semitone: i32,
    },

    /// Double track pattern length by duplicating existing steps.
    DuplicateTrackPattern {
        track: usize,
    },

    /// Halve track pattern length.
    HalveTrackPattern {
        track: usize,
    },

    /// Set or clear the per-step timebase p-lock.
    SetTimebasePlock {
        track: usize,
        step: usize,
        timebase: Option<Timebase>,
    },

    /// Set the same timebase p-lock on multiple steps.
    SetTimebasePlockMulti {
        track: usize,
        steps: Vec<usize>,
        timebase: Timebase,
    },

    /// Clear the timebase p-lock on multiple steps.
    ClearTimebasePlockMulti {
        track: usize,
        steps: Vec<usize>,
    },

    // ── Track params ──────────────────────────────────────────────────────────
    /// Toggle the gate (mute) flag for a track.
    ToggleTrackGate {
        track: usize,
    },

    /// Toggle the polyphonic flag for a track.
    ToggleTrackPolyphonic {
        track: usize,
    },
    ToggleTrackMute {
        track: usize,
    },
    ToggleTrackSolo {
        track: usize,
    },

    AdjustTrackMaxPolyphony {
        track: usize,
        delta: isize,
    },
    SetTrackMaxPolyphony {
        track: usize,
        value: usize,
    },

    SetTrackAttack {
        track: usize,
        ms: f32,
    },
    AdjustTrackAttack {
        track: usize,
        delta: f32,
    },

    SetTrackRelease {
        track: usize,
        ms: f32,
    },
    AdjustTrackRelease {
        track: usize,
        delta: f32,
    },

    SetTrackSwing {
        track: usize,
        value: f32,
    },
    SetTrackSwingPlock {
        track: usize,
        step: usize,
        value: Option<f32>,
    },
    SetTrackSwingPlockMulti {
        track: usize,
        steps: Vec<usize>,
        value: f32,
    },
    ClearTrackSwingPlockMulti {
        track: usize,
        steps: Vec<usize>,
    },
    AdjustTrackSwing {
        track: usize,
        delta: f32,
    },

    SetTrackSwingResolution {
        track: usize,
        resolution: SwingResolution,
    },
    SetTrackSwingResolutionPlock {
        track: usize,
        step: usize,
        resolution: Option<SwingResolution>,
    },
    SetTrackSwingResolutionPlockMulti {
        track: usize,
        steps: Vec<usize>,
        resolution: SwingResolution,
    },
    ClearTrackSwingResolutionPlockMulti {
        track: usize,
        steps: Vec<usize>,
    },
    NextTrackSwingResolution {
        track: usize,
    },
    PrevTrackSwingResolution {
        track: usize,
    },

    SetTrackNumSteps {
        track: usize,
        n: usize,
    },
    AdjustTrackNumSteps {
        track: usize,
        delta: isize,
    },

    /// Set track volume; also pushes to the live audio graph.
    SetTrackVolume {
        track: usize,
        value: f32,
    },
    /// Adjust track volume by a delta; also pushes to the live audio graph.
    AdjustTrackVolume {
        track: usize,
        delta: f32,
    },

    /// Set track pan; also pushes to the live audio graph.
    SetTrackPan {
        track: usize,
        value: f32,
    },
    /// Adjust track pan; also pushes.
    AdjustTrackPan {
        track: usize,
        delta: f32,
    },

    /// Set track send level; also pushes to the live audio graph.
    SetTrackSend {
        track: usize,
        value: f32,
    },
    /// Adjust track send; also pushes.
    AdjustTrackSend {
        track: usize,
        delta: f32,
    },

    SetTrackOutput {
        track: usize,
        output: TrackOutput,
    },
    SetTrackSends {
        track: usize,
        sends: Vec<TrackSendSnapshot>,
    },

    SetBusVolume {
        bus: BusId,
        value: f32,
    },
    ToggleBusMute {
        bus: BusId,
    },
    ToggleBusSolo {
        bus: BusId,
    },

    /// Set master volume; also pushes to the live audio graph.
    SetMasterVolume {
        value: f32,
    },
    AdjustMasterVolume {
        delta: f32,
    },
    /// Set one persisted global reverb parameter; also pushes to the live graph.
    SetReverbParam {
        param_idx: usize,
        value: f32,
    },

    SetTrackTimebase {
        track: usize,
        timebase: Timebase,
    },
    NextTrackTimebase {
        track: usize,
    },
    PrevTrackTimebase {
        track: usize,
    },

    SetTrackFtsScale {
        track: usize,
        scale_idx: usize,
    },

    SetTrackAccumIdx {
        track: usize,
        idx: usize,
        default_limit: Option<f32>,
        script_name: Option<String>,
    },
    SetTrackAccumLimit {
        track: usize,
        value: f32,
    },
    AdjustTrackAccumLimit {
        track: usize,
        delta: f32,
    },
    SetTrackAccumMode {
        track: usize,
        mode: u32,
    },
    SetTrackMuteGroup {
        track: usize,
        group: u8,
    },
    SetTrackGlobalTranspose {
        track: usize,
        enabled: bool,
    },

    // ── Effect params ─────────────────────────────────────────────────────────
    /// Set an effect slot default param value; also pushes to audio graph.
    SetEffectParam {
        track: usize,
        slot_idx: usize,
        param_idx: usize,
        value: f32,
    },

    /// Set a p-lock on a single step for an effect param.
    SetEffectPlock {
        track: usize,
        step: usize,
        slot_idx: usize,
        param_idx: usize,
        value: f32,
    },

    /// Set the same p-lock on multiple steps.
    SetEffectPlockMulti {
        track: usize,
        steps: Vec<usize>,
        slot_idx: usize,
        param_idx: usize,
        value: f32,
    },
    ClearEffectPlockMulti {
        track: usize,
        steps: Vec<usize>,
        slot_idx: usize,
        param_idx: usize,
    },
    SetEffectTensorCell {
        track: usize,
        slot_idx: usize,
        tensor_idx: usize,
        cell_idx: usize,
        value: f32,
    },
    SetEffectTensorPlockCellMulti {
        track: usize,
        steps: Vec<usize>,
        slot_idx: usize,
        tensor_idx: usize,
        cell_idx: usize,
        value: f32,
    },
    ClearEffectTensorPlockMulti {
        track: usize,
        steps: Vec<usize>,
        slot_idx: usize,
        tensor_idx: usize,
    },

    SetMidiFxParam {
        track: usize,
        slot_idx: usize,
        param_idx: usize,
        value: f32,
    },

    SetMidiFxPlockMulti {
        track: usize,
        steps: Vec<usize>,
        slot_idx: usize,
        param_idx: usize,
        value: f32,
    },
    ClearMidiFxPlockMulti {
        track: usize,
        steps: Vec<usize>,
        slot_idx: usize,
        param_idx: usize,
    },
    SetMidiFxTensorCell {
        track: usize,
        slot_idx: usize,
        tensor_idx: usize,
        cell_idx: usize,
        value: f32,
    },
    SetMidiFxTensorPlockCellMulti {
        track: usize,
        steps: Vec<usize>,
        slot_idx: usize,
        tensor_idx: usize,
        cell_idx: usize,
        value: f32,
    },
    ClearMidiFxTensorPlockMulti {
        track: usize,
        steps: Vec<usize>,
        slot_idx: usize,
        tensor_idx: usize,
    },

    // ── Instrument params ─────────────────────────────────────────────────────
    /// Set an instrument slot default param; also pushes to audio graph.
    SetInstrumentParam {
        track: usize,
        param_idx: usize,
        value: f32,
    },

    /// Set a p-lock on a single step for an instrument param.
    SetInstrumentPlock {
        track: usize,
        step: usize,
        param_idx: usize,
        value: f32,
    },

    /// Set the same p-lock on multiple steps.
    SetInstrumentPlockMulti {
        track: usize,
        steps: Vec<usize>,
        param_idx: usize,
        value: f32,
    },
    ClearInstrumentPlockMulti {
        track: usize,
        steps: Vec<usize>,
        param_idx: usize,
    },

    /// Set a key lock on a single MIDI note for an instrument param.
    SetInstrumentKeyLock {
        track: usize,
        note: u8,
        param_idx: usize,
        value: f32,
    },

    /// Set the same key lock on multiple MIDI notes.
    SetInstrumentKeyLockMulti {
        track: usize,
        notes: Vec<u8>,
        param_idx: usize,
        value: f32,
    },

    /// Clear a key lock on a single MIDI note for an instrument param.
    ClearInstrumentKeyLock {
        track: usize,
        note: u8,
        param_idx: usize,
    },

    /// Clear all key locks from a single MIDI note.
    ClearInstrumentKeyLocksForNote {
        track: usize,
        note: u8,
    },

    /// Stamp an existing key-lock variant onto multiple MIDI notes.
    StampInstrumentKeyLockVariant {
        track: usize,
        notes: Vec<u8>,
        key: PlockVariantKey,
    },

    /// Clear all key locks from multiple MIDI notes.
    ClearInstrumentKeyLockVariantsForNotes {
        track: usize,
        notes: Vec<u8>,
    },

    /// Edit one cell in an instrument tensor default.
    SetInstrumentTensorCell {
        track: usize,
        tensor_idx: usize,
        cell_idx: usize,
        value: f32,
    },

    /// Edit one cell in a whole-matrix instrument tensor p-lock on multiple steps.
    SetInstrumentTensorPlockCellMulti {
        track: usize,
        steps: Vec<usize>,
        tensor_idx: usize,
        cell_idx: usize,
        value: f32,
    },
    ClearInstrumentTensorPlockMulti {
        track: usize,
        steps: Vec<usize>,
        tensor_idx: usize,
    },

    /// Set the instrument base-note offset.
    SetInstrumentBaseNoteOffset {
        track: usize,
        value: f32,
    },

    /// Set a rack layer's linear gain; also pushes to the layer panner.
    SetRackSlotGain {
        track: usize,
        slot_idx: usize,
        value: f32,
    },

    /// Set a rack layer's pan; also pushes to the layer panner.
    SetRackSlotPan {
        track: usize,
        slot_idx: usize,
        value: f32,
    },

    /// Set a rack layer's mute flag; also pushes to the layer panner.
    SetRackSlotMute {
        track: usize,
        slot_idx: usize,
        value: bool,
    },

    /// Set a rack layer's solo flag; also refreshes all layer solo mutes.
    SetRackSlotSolo {
        track: usize,
        slot_idx: usize,
        value: bool,
    },

    /// Set a rack layer's max voice count.
    SetRackSlotMaxPolyphony {
        track: usize,
        slot_idx: usize,
        value: usize,
    },

    /// Set a rack layer's choke group. Group 0 means no choke group.
    SetRackSlotChokeGroup {
        track: usize,
        slot_idx: usize,
        value: u8,
    },

    /// Set a rack layer's instrument base-note offset.
    SetRackSlotBaseNoteOffset {
        track: usize,
        slot_idx: usize,
        value: f32,
    },

    /// Set a rack layer's strip parameter p-lock on one step.
    SetRackSlotParamPlock {
        track: usize,
        slot_idx: usize,
        step: usize,
        param: RackSlotParam,
        value: f32,
    },

    /// Set a rack layer's strip parameter p-lock across several steps.
    SetRackSlotParamPlockMulti {
        track: usize,
        slot_idx: usize,
        steps: Vec<usize>,
        param: RackSlotParam,
        value: f32,
    },

    /// Set a rack layer's underlying instrument default param.
    SetRackSlotInstrumentParam {
        track: usize,
        slot_idx: usize,
        param_idx: usize,
        value: f32,
    },

    SetRackSlotEffectParam {
        track: usize,
        rack_slot_idx: usize,
        effect_slot_idx: usize,
        param_idx: usize,
        value: f32,
    },

    /// Set a rack layer's underlying instrument step p-lock.
    SetRackSlotInstrumentPlock {
        track: usize,
        slot_idx: usize,
        step: usize,
        param_idx: usize,
        value: f32,
    },

    /// Set a rack layer's underlying instrument step p-lock across several steps.
    SetRackSlotInstrumentPlockMulti {
        track: usize,
        slot_idx: usize,
        steps: Vec<usize>,
        param_idx: usize,
        value: f32,
    },
    SetRackMacroPlockMulti {
        track: usize,
        steps: Vec<usize>,
        macro_idx: usize,
        value: f32,
    },
    ClearRackMacroPlockMulti {
        track: usize,
        steps: Vec<usize>,
        macro_idx: usize,
    },
    SetRackSlotEffectPlockMulti {
        track: usize,
        steps: Vec<usize>,
        rack_slot_idx: usize,
        effect_slot_idx: usize,
        param_idx: usize,
        value: f32,
    },
    ClearRackSlotEffectPlockMulti {
        track: usize,
        steps: Vec<usize>,
        rack_slot_idx: usize,
        effect_slot_idx: usize,
        param_idx: usize,
    },

    // ── Project-global macros ─────────────────────────────────────────────
    MacroCreate {
        name: String,
    },
    MacroCreateScene {
        name: String,
        target_scene: usize,
    },
    MacroSceneConfig {
        id: MacroId,
        config: crate::macro_engine::SceneMacroConfig,
    },
    MacroEnsure {
        key: String,
        name: String,
    },
    MacroDelete {
        id: MacroId,
    },
    MacroRename {
        id: MacroId,
        name: String,
    },
    MacroSetValue {
        id: MacroId,
        value: f32,
    },
    MacroRelease {
        id: MacroId,
    },
    ScenePushBegin {
        target_scene: usize,
        value: f32,
    },
    ScenePushSetValue {
        value: f32,
    },
    ScenePushEnd,
    MacroMapParam {
        id: MacroId,
        track: usize,
        target: ParamTarget,
    },
    MacroSetRange {
        id: MacroId,
        mapping_idx: usize,
        min: f32,
        max: f32,
    },
    MacroSetCurve {
        id: MacroId,
        mapping_idx: usize,
        curve: MacroCurve,
    },
    MacroUnmap {
        id: MacroId,
        mapping_idx: usize,
    },

    // ── Transport ─────────────────────────────────────────────────────────────
    TogglePlay,

    SetBpm {
        bpm: u32,
    },

    /// Adjust the record-quantize threshold (clamped to [0.1, 0.9]).
    AdjustRecordQuantizeThresh {
        delta: f32,
    },
}

/// Exhaustive staged history classification for every command intent.
///
/// Unsupported authoring commands remain barriers until their patch family is
/// implemented. Live macro gestures and transport playback are performance
/// actions and never affect authoring history.
pub fn history_policy(cmd: &AppCommand) -> super::history::HistoryPolicy {
    use super::history::HistoryPolicy;

    match cmd {
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
        | AppCommand::SetTimebasePlock { .. }
        | AppCommand::SetTimebasePlockMulti { .. }
        | AppCommand::ClearTimebasePlockMulti { .. }
        | AppCommand::SetTrackSwingPlock { .. }
        | AppCommand::SetTrackSwingPlockMulti { .. }
        | AppCommand::ClearTrackSwingPlockMulti { .. }
        | AppCommand::SetTrackSwingResolutionPlock { .. }
        | AppCommand::SetTrackSwingResolutionPlockMulti { .. }
        | AppCommand::ClearTrackSwingResolutionPlockMulti { .. }
        | AppCommand::ClearEffectPlockMulti { .. }
        | AppCommand::ClearEffectTensorPlockMulti { .. }
        | AppCommand::ClearMidiFxPlockMulti { .. }
        | AppCommand::ClearMidiFxTensorPlockMulti { .. }
        | AppCommand::ClearInstrumentPlockMulti { .. }
        | AppCommand::ClearInstrumentTensorPlockMulti { .. }
        | AppCommand::ClearRackMacroPlockMulti { .. }
        | AppCommand::ClearRackSlotEffectPlockMulti { .. } => HistoryPolicy::Record,
        AppCommand::DuplicateTrackPattern { .. }
        | AppCommand::HalveTrackPattern { .. }
        | AppCommand::SetTrackNumSteps { .. }
        | AppCommand::AdjustTrackNumSteps { .. } => HistoryPolicy::Record,

        AppCommand::ToggleTrackGate { .. }
        | AppCommand::ToggleTrackPolyphonic { .. }
        | AppCommand::ToggleTrackMute { .. }
        | AppCommand::ToggleTrackSolo { .. }
        | AppCommand::SetTrackSwingResolution { .. }
        | AppCommand::NextTrackSwingResolution { .. }
        | AppCommand::PrevTrackSwingResolution { .. }
        | AppCommand::SetTrackOutput { .. }
        | AppCommand::SetTrackTimebase { .. }
        | AppCommand::NextTrackTimebase { .. }
        | AppCommand::PrevTrackTimebase { .. }
        | AppCommand::SetTrackFtsScale { .. }
        | AppCommand::SetTrackAccumIdx { .. }
        | AppCommand::SetTrackAccumMode { .. }
        | AppCommand::SetTrackMuteGroup { .. }
        | AppCommand::SetTrackGlobalTranspose { .. } => HistoryPolicy::Record,

        AppCommand::ToggleBusMute { .. } | AppCommand::ToggleBusSolo { .. } => {
            HistoryPolicy::Record
        }
        AppCommand::SetInstrumentKeyLock { .. }
        | AppCommand::SetInstrumentKeyLockMulti { .. }
        | AppCommand::ClearInstrumentKeyLock { .. }
        | AppCommand::ClearInstrumentKeyLocksForNote { .. }
        | AppCommand::StampInstrumentKeyLockVariant { .. }
        | AppCommand::ClearInstrumentKeyLockVariantsForNotes { .. }
        | AppCommand::SetRackSlotMute { .. }
        | AppCommand::SetRackSlotSolo { .. }
        | AppCommand::SetRackSlotChokeGroup { .. } => HistoryPolicy::Record,

        AppCommand::AdjustTrackMaxPolyphony { track, .. }
        | AppCommand::SetTrackMaxPolyphony { track, .. } => HistoryPolicy::Coalesce(
            super::history::MergeKey::new(format!("track:{track}:max-polyphony")),
        ),
        AppCommand::SetTrackAttack { track, .. } | AppCommand::AdjustTrackAttack { track, .. } => {
            HistoryPolicy::Coalesce(super::history::MergeKey::new(format!(
                "track:{track}:attack"
            )))
        }
        AppCommand::SetTrackRelease { track, .. }
        | AppCommand::AdjustTrackRelease { track, .. } => HistoryPolicy::Coalesce(
            super::history::MergeKey::new(format!("track:{track}:release")),
        ),
        AppCommand::SetTrackSwing { track, .. } | AppCommand::AdjustTrackSwing { track, .. } => {
            HistoryPolicy::Coalesce(super::history::MergeKey::new(format!(
                "track:{track}:swing"
            )))
        }
        AppCommand::SetTrackVolume { track, .. } | AppCommand::AdjustTrackVolume { track, .. } => {
            HistoryPolicy::Coalesce(super::history::MergeKey::new(format!(
                "track:{track}:volume"
            )))
        }
        AppCommand::SetTrackPan { track, .. } | AppCommand::AdjustTrackPan { track, .. } => {
            HistoryPolicy::Coalesce(super::history::MergeKey::new(format!(
                "track:{track}:pan"
            )))
        }
        AppCommand::SetTrackSend { track, .. } | AppCommand::AdjustTrackSend { track, .. } => {
            HistoryPolicy::Coalesce(super::history::MergeKey::new(format!(
                "track:{track}:send"
            )))
        }
        AppCommand::SetTrackSends { track, .. } => HistoryPolicy::Coalesce(
            super::history::MergeKey::new(format!("track:{track}:sends")),
        ),
        AppCommand::SetBusVolume { bus, .. } => HistoryPolicy::Coalesce(
            super::history::MergeKey::new(format!("bus:{}:volume", bus.0)),
        ),
        AppCommand::SetEffectPlock { track, step, slot_idx, param_idx, .. } =>
            HistoryPolicy::Coalesce(super::history::MergeKey::new(format!(
                "track:{track}:effect:{slot_idx}:param:{param_idx}:step:{step}"
            ))),
        AppCommand::SetEffectPlockMulti { track, slot_idx, param_idx, .. } =>
            HistoryPolicy::Coalesce(super::history::MergeKey::new(format!(
                "track:{track}:effect:{slot_idx}:param:{param_idx}:plock"
            ))),
        AppCommand::SetEffectTensorPlockCellMulti {
            track, slot_idx, tensor_idx, cell_idx, ..
        } => HistoryPolicy::Coalesce(super::history::MergeKey::new(format!(
            "track:{track}:effect:{slot_idx}:tensor:{tensor_idx}:cell:{cell_idx}:plock"
        ))),
        AppCommand::SetMidiFxPlockMulti { track, slot_idx, param_idx, .. } =>
            HistoryPolicy::Coalesce(super::history::MergeKey::new(format!(
                "track:{track}:midi-fx:{slot_idx}:param:{param_idx}:plock"
            ))),
        AppCommand::SetMidiFxTensorPlockCellMulti {
            track, slot_idx, tensor_idx, cell_idx, ..
        } => HistoryPolicy::Coalesce(super::history::MergeKey::new(format!(
            "track:{track}:midi-fx:{slot_idx}:tensor:{tensor_idx}:cell:{cell_idx}:plock"
        ))),
        AppCommand::SetInstrumentPlock { track, step, param_idx, .. } =>
            HistoryPolicy::Coalesce(super::history::MergeKey::new(format!(
                "track:{track}:instrument:param:{param_idx}:step:{step}"
            ))),
        AppCommand::SetInstrumentPlockMulti { track, param_idx, .. } =>
            HistoryPolicy::Coalesce(super::history::MergeKey::new(format!(
                "track:{track}:instrument:param:{param_idx}:plock"
            ))),
        AppCommand::SetInstrumentTensorPlockCellMulti {
            track, tensor_idx, cell_idx, ..
        } => HistoryPolicy::Coalesce(super::history::MergeKey::new(format!(
            "track:{track}:instrument:tensor:{tensor_idx}:cell:{cell_idx}:plock"
        ))),
        AppCommand::SetRackSlotParamPlock { track, slot_idx, step, param, .. } =>
            HistoryPolicy::Coalesce(super::history::MergeKey::new(format!(
                "track:{track}:rack:{slot_idx}:{param:?}:step:{step}"
            ))),
        AppCommand::SetRackSlotParamPlockMulti { track, slot_idx, param, .. } =>
            HistoryPolicy::Coalesce(super::history::MergeKey::new(format!(
                "track:{track}:rack:{slot_idx}:{param:?}:plock"
            ))),
        AppCommand::SetRackSlotInstrumentPlock {
            track, slot_idx, step, param_idx, ..
        } => HistoryPolicy::Coalesce(super::history::MergeKey::new(format!(
            "track:{track}:rack:{slot_idx}:instrument:{param_idx}:step:{step}"
        ))),
        AppCommand::SetRackSlotInstrumentPlockMulti { track, slot_idx, param_idx, .. } =>
            HistoryPolicy::Coalesce(super::history::MergeKey::new(format!(
                "track:{track}:rack:{slot_idx}:instrument:{param_idx}:plock"
            ))),
        AppCommand::SetRackMacroPlockMulti { track, macro_idx, .. } =>
            HistoryPolicy::Coalesce(super::history::MergeKey::new(format!(
                "track:{track}:rack-macro:{macro_idx}:plock"
            ))),
        AppCommand::SetRackSlotEffectPlockMulti {
            track, rack_slot_idx, effect_slot_idx, param_idx, ..
        } => HistoryPolicy::Coalesce(super::history::MergeKey::new(format!(
            "track:{track}:rack:{rack_slot_idx}:effect:{effect_slot_idx}:param:{param_idx}:plock"
        ))),
        AppCommand::SetEffectParam {
            track, slot_idx, param_idx, ..
        } => HistoryPolicy::Coalesce(super::history::MergeKey::new(format!(
            "track:{track}:effect:{slot_idx}:param:{param_idx}"
        ))),
        AppCommand::SetMidiFxParam {
            track, slot_idx, param_idx, ..
        } => HistoryPolicy::Coalesce(super::history::MergeKey::new(format!(
            "track:{track}:midi-fx:{slot_idx}:param:{param_idx}"
        ))),
        AppCommand::SetEffectTensorCell {
            track, slot_idx, tensor_idx, cell_idx, ..
        } => HistoryPolicy::Coalesce(super::history::MergeKey::new(format!(
            "track:{track}:effect:{slot_idx}:tensor:{tensor_idx}:cell:{cell_idx}"
        ))),
        AppCommand::SetMidiFxTensorCell {
            track, slot_idx, tensor_idx, cell_idx, ..
        } => HistoryPolicy::Coalesce(super::history::MergeKey::new(format!(
            "track:{track}:midi-fx:{slot_idx}:tensor:{tensor_idx}:cell:{cell_idx}"
        ))),
        AppCommand::SetInstrumentParam { track, param_idx, .. } => HistoryPolicy::Coalesce(
            super::history::MergeKey::new(format!("track:{track}:instrument:param:{param_idx}")),
        ),
        AppCommand::SetInstrumentTensorCell {
            track, tensor_idx, cell_idx, ..
        } => HistoryPolicy::Coalesce(super::history::MergeKey::new(format!(
            "track:{track}:instrument:tensor:{tensor_idx}:cell:{cell_idx}"
        ))),
        AppCommand::SetRackSlotGain { track, slot_idx, .. } => HistoryPolicy::Coalesce(
            super::history::MergeKey::new(format!("track:{track}:rack-slot:{slot_idx}:gain")),
        ),
        AppCommand::SetRackSlotPan { track, slot_idx, .. } => HistoryPolicy::Coalesce(
            super::history::MergeKey::new(format!("track:{track}:rack-slot:{slot_idx}:pan")),
        ),
        AppCommand::SetRackSlotMaxPolyphony { track, slot_idx, .. } => HistoryPolicy::Coalesce(
            super::history::MergeKey::new(format!(
                "track:{track}:rack-slot:{slot_idx}:max-polyphony"
            )),
        ),
        AppCommand::SetRackSlotBaseNoteOffset { track, slot_idx, .. } => {
            HistoryPolicy::Coalesce(super::history::MergeKey::new(format!(
                "track:{track}:rack-slot:{slot_idx}:base-note"
            )))
        }
        AppCommand::SetRackSlotInstrumentParam {
            track, slot_idx, param_idx, ..
        } => HistoryPolicy::Coalesce(super::history::MergeKey::new(format!(
            "track:{track}:rack-slot:{slot_idx}:instrument:param:{param_idx}"
        ))),
        AppCommand::SetRackSlotEffectParam {
            track,
            rack_slot_idx,
            effect_slot_idx,
            param_idx,
            ..
        } => HistoryPolicy::Coalesce(super::history::MergeKey::new(format!(
            "track:{track}:rack-slot:{rack_slot_idx}:effect:{effect_slot_idx}:param:{param_idx}"
        ))),
        AppCommand::SetTrackAccumLimit { track, .. }
        | AppCommand::AdjustTrackAccumLimit { track, .. } => HistoryPolicy::Coalesce(
            super::history::MergeKey::new(format!("track:{track}:accum-limit")),
        ),
        AppCommand::SetInstrumentBaseNoteOffset { track, .. } => HistoryPolicy::Coalesce(
            super::history::MergeKey::new(format!("track:{track}:base-note")),
        ),
        AppCommand::SetMasterVolume { .. } | AppCommand::AdjustMasterVolume { .. } => {
            HistoryPolicy::Coalesce(super::history::MergeKey::new("transport:master-volume"))
        }
        AppCommand::SetReverbParam { param_idx, .. } => HistoryPolicy::Coalesce(
            super::history::MergeKey::new(format!("global:reverb:{param_idx}")),
        ),
        AppCommand::SetBpm { .. } => {
            HistoryPolicy::Coalesce(super::history::MergeKey::new("transport:bpm"))
        }

        AppCommand::MacroSetValue { .. }
        | AppCommand::MacroRelease { .. }
        | AppCommand::ScenePushBegin { .. }
        | AppCommand::ScenePushSetValue { .. }
        | AppCommand::ScenePushEnd
        | AppCommand::TogglePlay
        | AppCommand::AdjustRecordQuantizeThresh { .. } => HistoryPolicy::Ignore,

        AppCommand::MacroCreate { .. }
        | AppCommand::MacroCreateScene { .. }
        | AppCommand::MacroSceneConfig { .. }
        | AppCommand::MacroEnsure { .. }
        | AppCommand::MacroDelete { .. }
        | AppCommand::MacroRename { .. }
        | AppCommand::MacroMapParam { .. }
        | AppCommand::MacroSetRange { .. }
        | AppCommand::MacroSetCurve { .. }
        | AppCommand::MacroUnmap { .. } => HistoryPolicy::Record,
    }
}

/// Execute `cmd` against `app`, calling
/// `app.state.publish_scheduler_snapshot()` afterwards when the command
/// mutated sequencer/transport state.
///
/// Audio-graph side-effects (volume, pan, send, reverb, effect params) are
/// performed inside this function alongside the state mutation.
#[allow(dead_code)]
pub fn apply_command(app: &mut App, cmd: AppCommand) {
    if let Err(error) = super::edit::try_apply_command(app, cmd) {
        app.editor.status_message = Some((format!("Command failed: {error:?}"), Instant::now()));
        eprintln!("command failed: {error:?}");
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use super::{
        apply_command, command_mutates_sequencer_state, history_policy,
        sanitize_pasted_step_snapshot, AppCommand,
    };
    use crate::audiograph::LiveGraphPtr;
    use crate::effects::{
        EffectDescriptor, EffectSlotSnapshot, InstrumentModulationTarget, ParamDescriptor,
        ParamKind, ParamScaling, TensorParamDescriptor,
    };
    use crate::macro_engine::{MacroCurve, MacroKind, MacroMapping};
    use crate::neural::ParamNodeId;
    use crate::plock_variants::PlockVariantKey;
    use crate::process::ParamTarget;
    use crate::quantized_launch::PatternLaunchTarget;
    use crate::recorder::MasterRecorder;
    use crate::sequencer::{
        default_empty_effect_chain, CustomInstrumentRunMode, InstrumentType, RackRouting,
        RackSlotParam, RackSlotParamPlocks, RackSlotSnapshot, RackTrackSnapshot, SequencerState,
        StepSlotPlocks, StepSnapshot, SwingResolution, Timebase, TrackSendSnapshot,
        TrackSoundState, NUM_PARAMS,
    };
    use crate::tui::{App, AudioBuses};

    fn effect_mod_test_descriptor() -> EffectDescriptor {
        EffectDescriptor {
            name: "modded effect".to_string(),
            input_channels: 6,
            output_channels: 2,
            instrument_modulators: Vec::new(),
            instrument_modulation_targets: vec![
                InstrumentModulationTarget {
                    base_param_idx: 0,
                    source_param_idx: None,
                    modulator_slot: 1,
                    depth_param_idx: 2,
                    active_param_idx: Some(1),
                    depth_min: -1.0,
                    depth_max: 1.0,
                    depth_unit: None,
                },
                InstrumentModulationTarget {
                    base_param_idx: 0,
                    source_param_idx: None,
                    modulator_slot: 2,
                    depth_param_idx: 3,
                    active_param_idx: Some(1),
                    depth_min: -1.0,
                    depth_max: 1.0,
                    depth_unit: None,
                },
            ],
            tensor_params: Vec::new(),
            params: vec![
                ParamDescriptor {
                    name: "xyz".to_string(),
                    min: 0.0,
                    max: 1.0,
                    default: 0.5,
                    kind: ParamKind::Continuous { unit: None },
                    scaling: ParamScaling::Linear,
                    node_param_idx: 10,
                    node_param_span: 1,
                    host_control: None,
                    ui_metadata: None,
                },
                ParamDescriptor {
                    name: "__dgen_mod_active__xyz".to_string(),
                    min: 0.0,
                    max: 1.0,
                    default: 0.0,
                    kind: ParamKind::Boolean,
                    scaling: ParamScaling::Linear,
                    node_param_idx: 11,
                    node_param_span: 1,
                    host_control: None,
                    ui_metadata: None,
                },
                ParamDescriptor {
                    name: "mod xyz slot 1 amt".to_string(),
                    min: -1.0,
                    max: 1.0,
                    default: 0.0,
                    kind: ParamKind::Continuous { unit: None },
                    scaling: ParamScaling::Linear,
                    node_param_idx: 12,
                    node_param_span: 1,
                    host_control: None,
                    ui_metadata: None,
                },
                ParamDescriptor {
                    name: "mod xyz slot 2 amt".to_string(),
                    min: -1.0,
                    max: 1.0,
                    default: 0.0,
                    kind: ParamKind::Continuous { unit: None },
                    scaling: ParamScaling::Linear,
                    node_param_idx: 13,
                    node_param_span: 1,
                    host_control: None,
                    ui_metadata: None,
                },
            ],
        }
    }

    #[test]
    fn history_policy_records_completed_step_and_pattern_geometry_slices() {
        use crate::tui::history::HistoryPolicy;

        assert_eq!(
            history_policy(&AppCommand::ToggleStep { track: 0, step: 3 }),
            HistoryPolicy::Record
        );
        assert_eq!(
            history_policy(&AppCommand::DuplicateTrackPattern { track: 0 }),
            HistoryPolicy::Record
        );
        assert_eq!(
            history_policy(&AppCommand::MacroSetValue {
                id: 1,
                value: 0.5,
            }),
            HistoryPolicy::Ignore
        );
        assert_eq!(
            history_policy(&AppCommand::TogglePlay),
            HistoryPolicy::Ignore
        );
        assert!(matches!(
            history_policy(&AppCommand::SetTrackVolume {
                track: 0,
                value: 0.75,
            }),
            HistoryPolicy::Coalesce(_)
        ));
        assert_eq!(
            history_policy(&AppCommand::SetTrackTimebase {
                track: 0,
                timebase: Timebase::Eighth,
            }),
            HistoryPolicy::Record
        );
        assert_eq!(
            history_policy(&AppCommand::AdjustRecordQuantizeThresh { delta: 0.1 }),
            HistoryPolicy::Ignore
        );
    }

    fn test_app_with_effect_descriptor(desc: EffectDescriptor) -> App {
        let state = Arc::new(SequencerState::new(1, vec![default_empty_effect_chain()]));
        state.pattern.effect_chains[0][0].apply_descriptor(&desc, 0);
        let (keyboard_tx, _keyboard_rx) = std::sync::mpsc::channel();
        let mut app = App::new(
            state,
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
        app.graph.effect_descriptors = vec![vec![desc]];
        app
    }

    fn test_app_with_instrument_descriptor(desc: EffectDescriptor) -> App {
        let state = Arc::new(SequencerState::new(1, vec![default_empty_effect_chain()]));
        state.pattern.instrument_slots[0].apply_descriptor(&desc, 42);
        let (keyboard_tx, _keyboard_rx) = std::sync::mpsc::channel();
        let mut app = App::new(
            state,
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
        app.graph.track_instrument_types = vec![InstrumentType::Custom];
        app.graph.instrument_descriptors = vec![desc];
        app
    }

    fn tensor_instrument_descriptor() -> EffectDescriptor {
        EffectDescriptor {
            name: "tensor instrument".to_string(),
            input_channels: 0,
            output_channels: 2,
            instrument_modulators: Vec::new(),
            instrument_modulation_targets: Vec::new(),
            tensor_params: vec![TensorParamDescriptor {
                name: "strike_mask".to_string(),
                shape: vec![2, 2],
                cell_offset: 64,
                default: vec![0.1, 0.2, 0.3, 0.4],
                min: 0.0,
                max: 1.0,
            }],
            params: Vec::new(),
        }
    }

    fn test_app_with_bus_effect_descriptor(desc: EffectDescriptor) -> App {
        let mut app = test_app_with_effect_descriptor(desc.clone());
        let bus = app
            .buses
            .first_mut()
            .expect("test app should start with a mix bus");
        bus.effect_descriptors = vec![desc.clone()];
        bus.effect_slots = vec![EffectSlotSnapshot::new_default(&desc, 0)];
        app
    }

    fn test_app_with_rack_sampler_slot() -> App {
        let sampler_desc = EffectDescriptor::builtin_sampler();
        let state = Arc::new(SequencerState::new(1, vec![default_empty_effect_chain()]));
        state.set_rack_track_for_all_pattern_snapshots(
            0,
            RackTrackSnapshot::new(
                RackRouting::Broadcast,
                vec![RackSlotSnapshot {
                    instrument_type: InstrumentType::Sampler,
                    instrument_run_mode: CustomInstrumentRunMode::Instrument,
                    instrument_base_note_offset: 0.0,
                    pad_note: None,
                    choke_group: None,
                    gain: 1.0,
                    pan: 0.0,
                    mute: false,
                    solo: false,
                    max_polyphony: 8,
                    param_plocks: RackSlotParamPlocks::new(),
                    instrument_slot: EffectSlotSnapshot::new_default_with_modulator(
                        &sampler_desc,
                        77,
                        0,
                    ),
                    effect_slots: RackSlotSnapshot::empty_effect_slots(),
                    effect_descriptors: EffectDescriptor::default_full_chain(),
                    custom_effect_names: RackSlotSnapshot::empty_effect_names(),
                    track_sound_state: TrackSoundState::default(),
                    sample_id: Some((1, "test.wav".to_string(), 44_100)),
                }],
                crate::sequencer::default_rack_macros(),
            ),
        );
        let (keyboard_tx, _keyboard_rx) = std::sync::mpsc::channel();
        let mut app = App::new(
            state,
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
        app.tracks = vec!["Rack".to_string()];
        app.track_registry = crate::sequencer::TrackRegistry::for_legacy_track_count(1).unwrap();
        app.graph.track_instrument_types = vec![InstrumentType::Rack];
        app.graph.instrument_descriptors = vec![EffectDescriptor::empty_custom_slot()];
        app
    }

    #[test]
    fn live_mixer_commands_do_not_publish_scheduler_snapshots() {
        assert!(!command_mutates_sequencer_state(
            &AppCommand::SetTrackVolume {
                track: 0,
                value: 0.8,
            }
        ));
        assert!(!command_mutates_sequencer_state(&AppCommand::SetTrackPan {
            track: 0,
            value: -0.25,
        }));
        assert!(!command_mutates_sequencer_state(
            &AppCommand::SetTrackSends {
                track: 0,
                sends: vec![TrackSendSnapshot {
                    destination: crate::sequencer::BusId(1),
                    amount: 0.5,
                }],
            }
        ));
        assert!(!command_mutates_sequencer_state(
            &AppCommand::SetMasterVolume { value: 1.1 }
        ));
    }

    #[test]
    fn sequenced_pattern_commands_publish_scheduler_snapshots() {
        assert!(command_mutates_sequencer_state(&AppCommand::ToggleStep {
            track: 0,
            step: 0,
        }));
        assert!(command_mutates_sequencer_state(
            &AppCommand::SetInstrumentParam {
                track: 0,
                param_idx: 0,
                value: 0.5,
            }
        ));
        assert!(command_mutates_sequencer_state(
            &AppCommand::SetInstrumentKeyLock {
                track: 0,
                note: 69,
                param_idx: 0,
                value: 0.5,
            }
        ));
        assert!(command_mutates_sequencer_state(
            &AppCommand::SetInstrumentKeyLockMulti {
                track: 0,
                notes: vec![69, 72],
                param_idx: 0,
                value: 0.6,
            }
        ));
        assert!(command_mutates_sequencer_state(
            &AppCommand::StampInstrumentKeyLockVariant {
                track: 0,
                notes: vec![69, 72],
                key: PlockVariantKey::default(),
            }
        ));
        assert!(command_mutates_sequencer_state(
            &AppCommand::ClearInstrumentKeyLockVariantsForNotes {
                track: 0,
                notes: vec![69, 72],
            }
        ));
        assert!(command_mutates_sequencer_state(
            &AppCommand::SetInstrumentTensorCell {
                track: 0,
                tensor_idx: 0,
                cell_idx: 0,
                value: 0.5,
            }
        ));
        assert!(command_mutates_sequencer_state(
            &AppCommand::SetInstrumentTensorPlockCellMulti {
                track: 0,
                steps: vec![0],
                tensor_idx: 0,
                cell_idx: 0,
                value: 0.5,
            }
        ));
        assert!(command_mutates_sequencer_state(
            &AppCommand::SetRackSlotGain {
                track: 0,
                slot_idx: 0,
                value: 0.5,
            }
        ));
        assert!(command_mutates_sequencer_state(
            &AppCommand::SetRackSlotParamPlock {
                track: 0,
                slot_idx: 0,
                step: 0,
                param: RackSlotParam::Gain,
                value: 0.5,
            }
        ));
        assert!(command_mutates_sequencer_state(
            &AppCommand::SetRackSlotChokeGroup {
                track: 0,
                slot_idx: 0,
                value: 1,
            }
        ));
        assert!(command_mutates_sequencer_state(
            &AppCommand::SetRackSlotInstrumentPlock {
                track: 0,
                slot_idx: 0,
                step: 0,
                param_idx: 8,
                value: 22_050.0,
            }
        ));
        assert!(command_mutates_sequencer_state(&AppCommand::SetBpm {
            bpm: 128
        }));
    }

    #[test]
    fn paste_sanitizer_clears_audio_plocks_but_keeps_sequencer_plocks() {
        let mut params = [0.0; NUM_PARAMS];
        params[0] = 0.75;
        let snapshot = StepSnapshot {
            active: true,
            neural_reset: true,
            params,
            chord: vec![0.0, 7.0],
            chord_durations: vec![1.0, 1.0],
            chord_delays: vec![0.0, 0.25],
            timebase: Some(Timebase::Eighth),
            swing: Some(62.0),
            swing_resolution: Some(SwingResolution::Eighth),
            midi_fx_plocks: vec![StepSlotPlocks {
                params: vec![Some(0.5)],
                tensor_params: vec![Some(vec![0.1, 0.2])],
            }],
            effect_plocks: vec![StepSlotPlocks {
                params: vec![Some(0.1), None, Some(0.9)],
                tensor_params: vec![Some(vec![0.3, 0.4])],
            }],
            instrument_plocks: StepSlotPlocks {
                params: vec![Some(0.2), Some(0.8)],
                tensor_params: vec![Some(vec![0.5, 0.6])],
            },
            rack_macro_plocks: vec![Some(0.6), None],
            rack_slot_param_plocks: vec![StepSlotPlocks {
                params: vec![Some(12.0), Some(0.4), None],
                tensor_params: Vec::new(),
            }],
            rack_slot_instrument_plocks: vec![StepSlotPlocks {
                params: vec![Some(0.3), None, Some(0.7)],
                tensor_params: vec![Some(vec![0.7, 0.8])],
            }],
            rack_slot_effect_plocks: vec![vec![StepSlotPlocks {
                params: vec![Some(0.9)],
                tensor_params: vec![Some(vec![0.9, 1.0])],
            }]],
        };

        let sanitized = sanitize_pasted_step_snapshot(&snapshot, false);

        assert!(sanitized.active);
        assert!(sanitized.neural_reset);
        assert_eq!(sanitized.params, params);
        assert_eq!(sanitized.chord, vec![0.0, 7.0]);
        assert_eq!(sanitized.chord_durations, vec![1.0, 1.0]);
        assert_eq!(sanitized.chord_delays, vec![0.0, 0.25]);
        assert_eq!(sanitized.timebase, Some(Timebase::Eighth));
        assert_eq!(sanitized.swing, Some(62.0));
        assert_eq!(sanitized.swing_resolution, Some(SwingResolution::Eighth));
        assert!(sanitized
            .midi_fx_plocks
            .iter()
            .all(|plocks| plocks.params.iter().all(Option::is_none)
                && plocks.tensor_params.iter().all(Option::is_none)));
        assert!(sanitized
            .effect_plocks
            .iter()
            .flat_map(|plocks| plocks.params.iter())
            .all(Option::is_none));
        assert!(sanitized
            .instrument_plocks
            .params
            .iter()
            .all(Option::is_none));
        assert!(sanitized.rack_macro_plocks.iter().all(Option::is_none));
        assert!(sanitized
            .rack_slot_param_plocks
            .iter()
            .flat_map(|plocks| plocks.params.iter())
            .all(Option::is_none));
        assert!(sanitized
            .rack_slot_instrument_plocks
            .iter()
            .all(|plocks| plocks.params.iter().all(Option::is_none)
                && plocks.tensor_params.iter().all(Option::is_none)));
        assert!(sanitized
            .rack_slot_effect_plocks
            .iter()
            .flatten()
            .all(|plocks| plocks.params.iter().all(Option::is_none)
                && plocks.tensor_params.iter().all(Option::is_none)));
    }

    #[test]
    fn paste_sanitizer_preserves_audio_plocks_for_same_track_paste() {
        let snapshot = StepSnapshot {
            active: true,
            neural_reset: true,
            params: [0.0; NUM_PARAMS],
            chord: vec![],
            chord_durations: vec![],
            chord_delays: vec![],
            timebase: None,
            swing: None,
            swing_resolution: None,
            midi_fx_plocks: vec![StepSlotPlocks {
                params: vec![Some(0.5)],
                tensor_params: vec![Some(vec![0.1, 0.2])],
            }],
            effect_plocks: vec![StepSlotPlocks {
                params: vec![Some(0.1), None, Some(0.9)],
                tensor_params: vec![Some(vec![0.3, 0.4])],
            }],
            instrument_plocks: StepSlotPlocks {
                params: vec![Some(0.2), Some(0.8)],
                tensor_params: vec![Some(vec![0.5, 0.6])],
            },
            rack_macro_plocks: vec![Some(0.6), None],
            rack_slot_param_plocks: vec![StepSlotPlocks {
                params: vec![Some(12.0), Some(0.4), None],
                tensor_params: Vec::new(),
            }],
            rack_slot_instrument_plocks: vec![StepSlotPlocks {
                params: vec![Some(0.3), None, Some(0.7)],
                tensor_params: vec![Some(vec![0.7, 0.8])],
            }],
            rack_slot_effect_plocks: vec![vec![StepSlotPlocks {
                params: vec![Some(0.9)],
                tensor_params: vec![Some(vec![0.9, 1.0])],
            }]],
        };

        let sanitized = sanitize_pasted_step_snapshot(&snapshot, true);

        assert_eq!(
            sanitized.effect_plocks[0].params,
            vec![Some(0.1), None, Some(0.9)]
        );
        assert_eq!(
            sanitized.instrument_plocks.params,
            vec![Some(0.2), Some(0.8)]
        );
        assert_eq!(sanitized.rack_macro_plocks, vec![Some(0.6), None]);
        assert_eq!(
            sanitized.rack_slot_param_plocks[0].params,
            vec![Some(12.0), Some(0.4), None]
        );
        assert_eq!(
            sanitized.rack_slot_instrument_plocks[0].params,
            vec![Some(0.3), None, Some(0.7)]
        );
    }

    #[test]
    fn rack_slot_param_plock_command_writes_slot_plocks_without_changing_default() {
        let mut app = test_app_with_rack_sampler_slot();

        apply_command(
            &mut app,
            AppCommand::SetRackSlotParamPlockMulti {
                track: 0,
                slot_idx: 0,
                steps: vec![2, 3],
                param: RackSlotParam::Gain,
                value: 0.42,
            },
        );

        let rack_tracks = app.state.pattern.rack_tracks.lock().unwrap();
        let rack = rack_tracks[0]
            .as_ref()
            .expect("test rack track should be present");
        let slot = &rack.slots[0];
        assert_eq!(
            slot.gain, 1.0,
            "editing selected rack slot steps must not overwrite the default gain"
        );
        assert_eq!(slot.param_plocks.get(2, RackSlotParam::Gain), Some(0.42));
        assert_eq!(slot.param_plocks.get(3, RackSlotParam::Gain), Some(0.42));
    }

    #[test]
    fn rack_slot_instrument_plock_command_writes_slot_plocks_without_changing_default() {
        let mut app = test_app_with_rack_sampler_slot();

        apply_command(
            &mut app,
            AppCommand::SetRackSlotInstrumentPlockMulti {
                track: 0,
                slot_idx: 0,
                steps: vec![2, 3],
                param_idx: 8,
                value: 22_050.0,
            },
        );

        let rack_tracks = app.state.pattern.rack_tracks.lock().unwrap();
        let rack = rack_tracks[0]
            .as_ref()
            .expect("test rack track should be present");
        let slot = &rack.slots[0].instrument_slot;
        assert_eq!(
            slot.defaults[8], 44_100.0,
            "editing selected rack sampler steps must not overwrite the default sample rate"
        );
        assert_eq!(slot.plocks[2][8], Some(22_050.0));
        assert_eq!(slot.plocks[3][8], Some(22_050.0));
        assert!(
            slot.plock_param_ids[2][8].is_some(),
            "rack sampler p-locks need node identity for scheduler/audio resolution"
        );
        assert!(rack.slots[0].track_sound_state.dirty);
    }

    #[test]
    fn instrument_tensor_default_cell_command_updates_default_without_plock() {
        let mut app = test_app_with_instrument_descriptor(tensor_instrument_descriptor());

        apply_command(
            &mut app,
            AppCommand::SetInstrumentTensorCell {
                track: 0,
                tensor_idx: 0,
                cell_idx: 2,
                value: 0.95,
            },
        );

        let slot = &app.state.pattern.instrument_slots[0];
        assert_eq!(
            slot.tensor_params.default_values(0).unwrap(),
            vec![0.1, 0.2, 0.95, 0.4]
        );
        assert_eq!(slot.tensor_params.plock_values(3, 0), None);
        assert!(
            app.state.pattern.track_sound_state.lock().unwrap()[0].dirty,
            "default tensor edits should refresh live instrument voices"
        );
    }

    #[test]
    fn instrument_tensor_plock_cell_command_writes_whole_matrix_without_changing_default() {
        let mut app = test_app_with_instrument_descriptor(tensor_instrument_descriptor());

        apply_command(
            &mut app,
            AppCommand::SetInstrumentTensorPlockCellMulti {
                track: 0,
                steps: vec![2, 3],
                tensor_idx: 0,
                cell_idx: 1,
                value: 0.85,
            },
        );

        let slot = &app.state.pattern.instrument_slots[0];
        assert_eq!(
            slot.tensor_params.default_values(0).unwrap(),
            vec![0.1, 0.2, 0.3, 0.4]
        );
        assert_eq!(
            slot.tensor_params.plock_values(2, 0).unwrap(),
            vec![0.1, 0.85, 0.3, 0.4]
        );
        assert_eq!(
            slot.tensor_params.plock_values(3, 0).unwrap(),
            vec![0.1, 0.85, 0.3, 0.4]
        );
    }

    #[test]
    fn effect_depth_default_command_updates_dgen_mod_active_param() {
        let desc = effect_mod_test_descriptor();
        let mut app = test_app_with_effect_descriptor(desc);

        apply_command(
            &mut app,
            AppCommand::SetEffectParam {
                track: 0,
                slot_idx: 0,
                param_idx: 2,
                value: 0.25,
            },
        );

        assert_eq!(app.state.pattern.effect_chains[0][0].defaults.get(1), 1.0);

        apply_command(
            &mut app,
            AppCommand::SetEffectParam {
                track: 0,
                slot_idx: 0,
                param_idx: 2,
                value: 0.0,
            },
        );

        let slot = &app.state.pattern.effect_chains[0][0];
        assert_eq!(slot.defaults.get(1), 0.0);
    }

    #[test]
    fn effect_base_edit_is_masked_while_macro_is_engaged_and_restored_on_release() {
        let desc = effect_mod_test_descriptor();
        let mut app = test_app_with_effect_descriptor(desc.clone());
        let id = app
            .macro_engine
            .create_macro("push", MacroKind::Mapped)
            .expect("macro id");
        app.macro_engine
            .add_mapping(
                id,
                MacroMapping::new_resolved(
                    0,
                    ParamTarget::EffectParam {
                        slot: 0,
                        effect: desc.name.clone(),
                        param: desc.params[0].name.clone(),
                        param_id: None,
                    },
                    Some(0),
                    0.2,
                    0.8,
                    MacroCurve::Linear,
                )
                .expect("effect mapping"),
            )
            .expect("known macro");
        app.set_macro_value(id, 1.0);

        apply_command(
            &mut app,
            AppCommand::SetEffectParam {
                track: 0,
                slot_idx: 0,
                param_idx: 0,
                value: 0.35,
            },
        );

        assert_eq!(app.state.pattern.effect_chains[0][0].defaults.get(0), 0.35);
        assert_eq!(app.effective_slot_param_value(0, 0, 0), Some(0.8));

        app.release_macro(id);
        assert_eq!(app.effective_slot_param_value(0, 0, 0), Some(0.35));
    }

    #[test]
    fn macro_commands_capture_descriptor_range_current_value_and_node_identity() {
        let desc = effect_mod_test_descriptor();
        let mut app = test_app_with_effect_descriptor(desc.clone());
        app.state.pattern.effect_chains[0][0].apply_descriptor(&desc, 41);
        app.state.pattern.effect_chains[0][0].defaults.set(0, 0.37);
        apply_command(
            &mut app,
            AppCommand::MacroCreate {
                name: "Push".to_string(),
            },
        );
        let id = app.macro_engine.macros()[0].id;

        apply_command(
            &mut app,
            AppCommand::MacroMapParam {
                id,
                track: 0,
                target: ParamTarget::EffectParam {
                    slot: 0,
                    effect: desc.name.clone(),
                    param: desc.params[0].name.clone(),
                    param_id: None,
                },
            },
        );

        let mapping = &app.macro_engine.macro_definition(id).unwrap().mappings[0];
        assert_eq!((mapping.range_min, mapping.range_max), (0.0, 1.0));
        assert_eq!(
            app.effective_slot_param_value(0, 0, 0),
            Some(0.0),
            "mapping a zero-position continuous macro must take ownership at its minimum"
        );
        assert_eq!(
            mapping.target,
            ParamTarget::EffectParam {
                slot: 0,
                effect: desc.name,
                param: desc.params[0].name.clone(),
                param_id: Some(ParamNodeId {
                    logical_id: 41,
                    node_param_idx: desc.params[0].node_param_idx,
                }),
            }
        );
    }

    #[test]
    fn macro_ensure_command_reuses_key_without_resetting_display_name() {
        let desc = effect_mod_test_descriptor();
        let mut app = test_app_with_effect_descriptor(desc);
        apply_command(
            &mut app,
            AppCommand::MacroEnsure {
                key: ":Player/Push".to_string(),
                name: "Push".to_string(),
            },
        );
        let id = app.macro_engine.macros()[0].id;
        apply_command(
            &mut app,
            AppCommand::MacroRename {
                id,
                name: "Performance Push".to_string(),
            },
        );
        apply_command(
            &mut app,
            AppCommand::MacroEnsure {
                key: "player/push".to_string(),
                name: "Reset Name".to_string(),
            },
        );

        assert_eq!(app.macro_engine.macros().len(), 1);
        assert_eq!(app.macro_engine.macros()[0].id, id);
        assert_eq!(app.macro_engine.macros()[0].name, "Performance Push");
        assert_eq!(
            app.macro_engine.macros()[0].key.as_deref(),
            Some("player/push")
        );
    }

    #[test]
    fn macro_range_unmap_and_delete_commands_release_without_reusing_ids() {
        let desc = effect_mod_test_descriptor();
        let mut app = test_app_with_effect_descriptor(desc.clone());
        let first = app
            .macro_engine
            .create_macro("first", MacroKind::Mapped)
            .expect("id");
        app.map_macro_param(
            first,
            0,
            ParamTarget::EffectParam {
                slot: 0,
                effect: desc.name.clone(),
                param: desc.params[0].name.clone(),
                param_id: None,
            },
        )
        .expect("map");
        apply_command(
            &mut app,
            AppCommand::MacroSetValue {
                id: first,
                value: 0.5,
            },
        );
        apply_command(
            &mut app,
            AppCommand::MacroSetRange {
                id: first,
                mapping_idx: 0,
                min: 0.2,
                max: 0.6,
            },
        );
        assert!((app.effective_slot_param_value(0, 0, 0).unwrap() - 0.4).abs() < 1.0e-6);

        apply_command(
            &mut app,
            AppCommand::MacroUnmap {
                id: first,
                mapping_idx: 0,
            },
        );
        assert_eq!(app.effective_slot_param_value(0, 0, 0), Some(0.5));

        apply_command(&mut app, AppCommand::MacroDelete { id: first });
        apply_command(
            &mut app,
            AppCommand::MacroCreate {
                name: "second".to_string(),
            },
        );
        assert_eq!(app.macro_engine.macros()[0].id, first + 1);
    }

    #[test]
    fn instrument_base_edit_is_masked_while_macro_is_engaged_and_restored_on_release() {
        let desc = effect_mod_test_descriptor();
        let mut app = test_app_with_instrument_descriptor(desc.clone());
        let id = app
            .macro_engine
            .create_macro("instrument push", MacroKind::Mapped)
            .expect("macro id");
        app.macro_engine
            .add_mapping(
                id,
                MacroMapping::new(
                    0,
                    ParamTarget::InstrumentParam {
                        param: desc.params[0].name.clone(),
                        param_id: Some(ParamNodeId {
                            logical_id: 42,
                            node_param_idx: desc.params[0].node_param_idx,
                        }),
                    },
                    0.1,
                    0.9,
                    MacroCurve::Linear,
                )
                .expect("instrument mapping"),
            )
            .expect("known macro");
        app.set_macro_value(id, 1.0);

        apply_command(
            &mut app,
            AppCommand::SetInstrumentParam {
                track: 0,
                param_idx: 0,
                value: 0.4,
            },
        );

        assert_eq!(app.state.pattern.instrument_slots[0].defaults.get(0), 0.4);
        assert_eq!(app.effective_instrument_param_value(0, 0), Some(0.9));

        app.release_macro(id);
        assert_eq!(app.effective_instrument_param_value(0, 0), Some(0.4));
    }

    #[test]
    fn restored_defaults_keep_engaged_override_and_suspend_incompatible_scene_target() {
        let desc = effect_mod_test_descriptor();
        let mut app = test_app_with_effect_descriptor(desc.clone());
        let id = app
            .macro_engine
            .create_macro("scene-safe push", MacroKind::Mapped)
            .expect("macro id");
        app.macro_engine
            .add_mapping(
                id,
                MacroMapping::new_resolved(
                    0,
                    ParamTarget::EffectParam {
                        slot: 0,
                        effect: desc.name.clone(),
                        param: desc.params[0].name.clone(),
                        param_id: None,
                    },
                    Some(0),
                    0.2,
                    0.8,
                    MacroCurve::Linear,
                )
                .expect("resolved fallback mapping"),
            )
            .expect("known macro");
        app.set_macro_value(id, 1.0);
        app.state.pattern.effect_chains[0][0].defaults.set(0, 0.3);

        app.push_all_restored_defaults();
        assert_eq!(app.effective_slot_param_value(0, 0, 0), Some(0.8));
        assert!(!app.macro_engine.macro_definition(id).unwrap().mappings[0].suspended);

        app.graph.effect_descriptors[0][0].name = "different effect".to_string();
        app.push_all_restored_defaults();
        assert_eq!(app.effective_slot_param_value(0, 0, 0), Some(0.3));
        assert!(app.macro_engine.macro_definition(id).unwrap().mappings[0].suspended);
    }

    #[test]
    fn full_scene_application_seam_reasserts_macro_effective_defaults() {
        let desc = effect_mod_test_descriptor();
        let mut app = test_app_with_effect_descriptor(desc.clone());
        let id = app
            .macro_engine
            .create_macro("scene launch push", MacroKind::Mapped)
            .expect("macro id");
        app.macro_engine
            .add_mapping(
                id,
                MacroMapping::new_resolved(
                    0,
                    ParamTarget::EffectParam {
                        slot: 0,
                        effect: desc.name.clone(),
                        param: desc.params[0].name.clone(),
                        param_id: None,
                    },
                    Some(0),
                    0.2,
                    0.8,
                    MacroCurve::Linear,
                )
                .unwrap(),
            )
            .unwrap();
        app.set_macro_value(id, 1.0);

        app.state.clone_pattern(
            1,
            &[-1],
            &[44_100],
            &["Track 1".to_string()],
            &[InstrumentType::Sampler],
        );
        let outcome = app
            .apply_pattern_launch(&PatternLaunchTarget::Scene { scene: 0 })
            .expect("saved scene should launch through the shared seam");

        assert!(outcome.warnings.is_empty());
        assert_eq!(app.effective_slot_param_value(0, 0, 0), Some(0.8));
        assert!(!app.macro_engine.macro_definition(id).unwrap().mappings[0].suspended);
    }

    #[test]
    fn scene_macro_diffs_live_scene_values_and_restores_base_on_release() {
        let desc = effect_mod_test_descriptor();
        let mut app = test_app_with_effect_descriptor(desc);
        let capture = |app: &App| {
            app.state.capture_current_pattern_snapshot(
                1,
                &[-1],
                &[44_100],
                &["Track 1".to_string()],
                &[InstrumentType::Sampler],
            )
        };
        app.state.pattern.effect_chains[0][0].defaults.set(0, 0.2);
        let origin = capture(&app);
        app.state.pattern.effect_chains[0][0].defaults.set(0, 0.8);
        let target = capture(&app);
        app.state
            .replace_pattern_repository(vec![origin, target], 0);
        app.state.restore_current_pattern_from_repository().unwrap();

        let id = app
            .macro_engine
            .create_macro(
                "scene morph",
                MacroKind::Scene(crate::macro_engine::SceneMacroConfig {
                    target_scene: 1,
                    morph_params: true,
                    steal_patterns: false,
                    quantize: crate::macro_engine::StealQuantize::Off,
                    track_mask: None,
                }),
            )
            .unwrap();
        app.set_macro_value(id, 0.5);
        assert_eq!(
            app.macro_engine
                .macro_definition(id)
                .unwrap()
                .mappings
                .len(),
            1
        );
        assert!((app.effective_slot_param_value(0, 0, 0).unwrap() - 0.5).abs() < 1.0e-6);

        app.release_macro(id);
        assert!(app
            .macro_engine
            .macro_definition(id)
            .unwrap()
            .mappings
            .is_empty());
        assert!((app.effective_slot_param_value(0, 0, 0).unwrap() - 0.2).abs() < 1.0e-6);
    }

    #[test]
    fn scene_push_gesture_morphs_without_creating_a_project_macro() {
        let desc = effect_mod_test_descriptor();
        let mut app = test_app_with_effect_descriptor(desc);
        let capture = |app: &App| {
            app.state.capture_current_pattern_snapshot(
                1,
                &[-1],
                &[44_100],
                &["Track 1".to_string()],
                &[InstrumentType::Sampler],
            )
        };
        app.state.pattern.effect_chains[0][0].defaults.set(0, 0.2);
        let origin = capture(&app);
        app.state.pattern.effect_chains[0][0].defaults.set(0, 0.8);
        let target = capture(&app);
        app.state
            .replace_pattern_repository(vec![origin, target], 0);
        app.state.restore_current_pattern_from_repository().unwrap();

        app.begin_scene_push(1, 1.0);
        assert!((app.effective_slot_param_value(0, 0, 0).unwrap() - 0.8).abs() < 1.0e-6);
        app.set_scene_push_value(0.5);
        assert!((app.effective_slot_param_value(0, 0, 0).unwrap() - 0.5).abs() < 1.0e-6);
        app.end_scene_push();

        assert!((app.effective_slot_param_value(0, 0, 0).unwrap() - 0.2).abs() < 1.0e-6);
        assert!(app.macro_engine.macros().is_empty());
    }

    #[test]
    fn scene_push_interpolates_rack_macro_values_without_mutating_the_scene() {
        let mut app = test_app_with_rack_sampler_slot();
        let macro_id = crate::sequencer::RackMacroId::from_index(0).unwrap();
        let capture = |app: &App| {
            app.state.capture_current_pattern_snapshot(
                1,
                &[-1],
                &[44_100],
                &["Rack".to_string()],
                &[InstrumentType::Rack],
            )
        };
        {
            let mut racks = app.state.pattern.rack_tracks.lock().unwrap();
            racks[0].as_mut().unwrap().macros[0].value = 0.2;
        }
        let origin = capture(&app);
        {
            let mut racks = app.state.pattern.rack_tracks.lock().unwrap();
            racks[0].as_mut().unwrap().macros[0].value = 0.8;
        }
        let target = capture(&app);
        app.state
            .replace_pattern_repository(vec![origin, target], 0);
        app.state.restore_current_pattern_from_repository().unwrap();
        {
            let mut racks = app.state.pattern.rack_tracks.lock().unwrap();
            racks[0].as_mut().unwrap().macros[0].plocks[3] = Some(0.35);
        }

        app.begin_scene_push(1, 1.0);
        assert!((app.effective_rack_macro_value(0, macro_id, None).unwrap() - 0.8).abs() < 1.0e-6);
        assert_eq!(
            app.state.latest_scheduler_snapshot().tracks[0]
                .rack_track
                .as_ref()
                .unwrap()
                .macros[0]
                .value,
            0.8
        );

        app.set_scene_push_value(0.5);
        assert!((app.effective_rack_macro_value(0, macro_id, None).unwrap() - 0.5).abs() < 1.0e-6);
        assert_eq!(
            app.effective_rack_macro_value(0, macro_id, Some(3)),
            Some(0.35)
        );
        let scheduler_snapshot = app.state.latest_scheduler_snapshot();
        let scheduler_macro = &scheduler_snapshot.tracks[0]
            .rack_track
            .as_ref()
            .unwrap()
            .macros[0];
        assert_eq!(scheduler_macro.value, 0.5);
        assert_eq!(scheduler_macro.value_at(3), 0.35);
        assert_eq!(
            app.state.pattern.rack_tracks.lock().unwrap()[0]
                .as_ref()
                .unwrap()
                .macros[0]
                .value,
            0.2
        );

        app.end_scene_push();
        assert!((app.effective_rack_macro_value(0, macro_id, None).unwrap() - 0.2).abs() < 1.0e-6);
        assert!(app.macro_engine.macros().is_empty());
    }

    #[test]
    fn scene_push_morphs_bus_effect_defaults_and_restores_them() {
        let mut app = test_app_with_bus_effect_descriptor(effect_mod_test_descriptor());
        let first = app.state.capture_current_pattern_snapshot(
            1,
            &[-1],
            &[44_100],
            &["Track 1".to_string()],
            &[InstrumentType::Sampler],
        );
        app.state
            .replace_pattern_repository(vec![first.clone(), first], 0);

        app.buses[0].effect_slots[0].defaults[0] = 0.2;
        let origin = app.capture_bus_pattern_snapshot();
        app.buses[0].effect_slots[0].defaults[0] = 0.8;
        let target = app.capture_bus_pattern_snapshot();
        app.buses[0].effect_slots[0].defaults[0] = 0.2;
        app.state
            .replace_bus_pattern_repository(vec![origin.clone(), target], &origin);
        app.buses[0].effect_slots[0].plocks[0][0] = Some(0.33);
        app.graph.bus_node_ids.push(crate::tui::BusNodeIds {
            id: app.buses[0].id,
            left_id: 0,
            right_id: 0,
            merge_id: 0,
            gate_id: 1,
            volume_id: 0,
            mod_in_clip_ids: [0; crate::sequencer::EXT_MOD_INPUT_COUNT],
        });

        app.begin_scene_push(1, 1.0);
        assert!(app.macro_engine.macros().is_empty());
        assert!(app
            .macro_engine
            .override_snapshot()
            .keys()
            .any(|key| matches!(
                key,
                crate::macro_engine::MacroParamKey::BusEffect { bus, .. }
                    if *bus == app.buses[0].id
            )));
        assert!((app.effective_bus_slot_param_value(0, 0, 0).unwrap() - 0.8).abs() < 1.0e-6);
        {
            let runtime = app.graph.bus_gate_runtime.lock().unwrap();
            assert!((runtime[0].effect_slots[0].defaults[0] - 0.8).abs() < 1.0e-6);
            assert_eq!(runtime[0].effect_slots[0].plocks[0][0], Some(0.33));
        }

        app.set_scene_push_value(0.5);
        assert!((app.effective_bus_slot_param_value(0, 0, 0).unwrap() - 0.5).abs() < 1.0e-6);

        app.end_scene_push();
        assert!((app.effective_bus_slot_param_value(0, 0, 0).unwrap() - 0.2).abs() < 1.0e-6);
    }

    #[test]
    fn scene_macro_steal_returns_only_after_target_launch_applies() {
        let mut app = test_app_with_effect_descriptor(effect_mod_test_descriptor());
        let first = app.state.capture_current_pattern_snapshot(
            1,
            &[-1],
            &[44_100],
            &["Track 1".to_string()],
            &[InstrumentType::Sampler],
        );
        let mut second = first.clone();
        second.track_bits[0][0] |= 1 << 3;
        app.state.replace_pattern_repository(vec![first, second], 0);
        app.state.restore_current_pattern_from_repository().unwrap();
        let id = app
            .macro_engine
            .create_macro(
                "pattern steal",
                MacroKind::Scene(crate::macro_engine::SceneMacroConfig {
                    target_scene: 1,
                    morph_params: false,
                    steal_patterns: true,
                    quantize: crate::macro_engine::StealQuantize::Off,
                    track_mask: None,
                }),
            )
            .unwrap();
        let mut pending = crate::quantized_launch::PendingQuantizedLaunches::default();

        app.set_macro_value(id, 1.0);
        assert_eq!(app.state.current_scene_index(), 0);
        app.state
            .quantized_launches()
            .process_scheduler(&mut pending, 0.0, true);
        let results = app.drain_due_pattern_launches();
        assert!(results[0].is_ok());
        assert_eq!(app.state.current_scene_index(), 1);

        app.release_macro(id);
        assert_eq!(
            app.state.current_scene_index(),
            1,
            "return is scheduler-owned"
        );
        app.state
            .quantized_launches()
            .process_scheduler(&mut pending, 0.0, true);
        let results = app.drain_due_pattern_launches();
        assert!(results[0].is_ok());
        assert_eq!(app.state.current_scene_index(), 0);
        assert!(!app.macro_engine.is_engaged(id));
    }

    #[test]
    fn effect_depth_plock_command_updates_dgen_mod_active_plock() {
        let desc = effect_mod_test_descriptor();
        let mut app = test_app_with_effect_descriptor(desc);
        let step = 7;

        apply_command(
            &mut app,
            AppCommand::SetEffectParam {
                track: 0,
                slot_idx: 0,
                param_idx: 2,
                value: 0.25,
            },
        );
        apply_command(
            &mut app,
            AppCommand::SetEffectPlock {
                track: 0,
                step,
                slot_idx: 0,
                param_idx: 2,
                value: 0.0,
            },
        );

        let slot = &app.state.pattern.effect_chains[0][0];
        assert_eq!(slot.defaults.get(1), 1.0);
        assert_eq!(slot.plocks.get(step, 1), Some(0.0));

        apply_command(
            &mut app,
            AppCommand::SetEffectPlock {
                track: 0,
                step,
                slot_idx: 0,
                param_idx: 3,
                value: -0.5,
            },
        );

        assert_eq!(
            app.state.pattern.effect_chains[0][0].plocks.get(step, 1),
            Some(1.0)
        );
    }

    #[test]
    fn bus_effect_depth_default_updates_dgen_mod_active_param() {
        let desc = effect_mod_test_descriptor();
        let mut app = test_app_with_bus_effect_descriptor(desc);

        app.set_bus_effect_param(0, 0, 2, 0.25)
            .expect("bus depth default should update");

        assert_eq!(app.buses[0].effect_slots[0].defaults[1], 1.0);

        app.set_bus_effect_param(0, 0, 2, 0.0)
            .expect("bus depth default should clear");

        assert_eq!(app.buses[0].effect_slots[0].defaults[1], 0.0);

        app.buses[0].effect_slots[0].defaults[2] = 0.25;
        app.set_bus_effect_param(0, 0, 0, 0.75)
            .expect("unrelated bus param should repair active flag");

        assert_eq!(app.buses[0].effect_slots[0].defaults[1], 1.0);
    }

    #[test]
    fn bus_effect_depth_plock_updates_dgen_mod_active_plock() {
        let desc = effect_mod_test_descriptor();
        let mut app = test_app_with_bus_effect_descriptor(desc);
        let step = 7;

        app.set_bus_effect_param(0, 0, 2, 0.25)
            .expect("bus depth default should update");
        app.set_bus_effect_plock(0, 0, step, 2, 0.0)
            .expect("bus depth p-lock should update");

        let slot = &app.buses[0].effect_slots[0];
        assert_eq!(slot.defaults[1], 1.0);
        assert_eq!(slot.plocks[step][1], Some(0.0));

        app.set_bus_effect_plock(0, 0, step, 3, 0.5)
            .expect("second bus depth p-lock should update");

        let slot = &app.buses[0].effect_slots[0];
        assert_eq!(slot.plocks[step][1], Some(1.0));
    }

    #[test]
    fn instrument_key_lock_variant_command_stamps_and_clears_notes() {
        let desc = EffectDescriptor::builtin_filter();
        let cutoff_idx = desc
            .params
            .iter()
            .position(|param| param.name == "cutoff")
            .expect("filter descriptor should include cutoff");
        let mode_idx = desc
            .params
            .iter()
            .position(|param| param.name == "mode")
            .expect("filter descriptor should include mode");
        let mut app = test_app_with_instrument_descriptor(desc);
        app.state.pattern.instrument_slots[0].set_key_lock(60, cutoff_idx, 900.0);
        app.state.pattern.instrument_slots[0].set_key_lock(60, mode_idx, 2.0);
        let assignment = app.state.reconcile_key_lock_variant_registry_for_track(0)[60]
            .clone()
            .expect("source note should have a key-lock variant");

        apply_command(
            &mut app,
            AppCommand::StampInstrumentKeyLockVariant {
                track: 0,
                notes: vec![62],
                key: assignment.key,
            },
        );

        let slot = &app.state.pattern.instrument_slots[0];
        assert_eq!(slot.key_locks.get(62, cutoff_idx), Some(900.0));
        assert_eq!(slot.key_locks.get(62, mode_idx), Some(2.0));
        assert!(
            app.state.pattern.track_sound_state.lock().unwrap()[0].dirty,
            "stamping a key-lock variant should mark the track sound dirty"
        );

        apply_command(
            &mut app,
            AppCommand::ClearInstrumentKeyLockVariantsForNotes {
                track: 0,
                notes: vec![62],
            },
        );

        let slot = &app.state.pattern.instrument_slots[0];
        assert_eq!(slot.key_locks.get(62, cutoff_idx), None);
        assert_eq!(slot.key_locks.get(62, mode_idx), None);
    }
}

/// Returns `true` for commands whose state changes must be visible to the
/// scheduler's immutable pattern snapshot.
///
/// Continuous live-mixer controls are pushed directly to the audio graph by
/// `execute_command`. Publishing a full scheduler snapshot for each drag tick
/// makes the app loop perform broad pattern/UI sync work even though event
/// scheduling cannot observe those fields.
pub(crate) fn command_mutates_sequencer_state(cmd: &AppCommand) -> bool {
    !matches!(
        cmd,
        AppCommand::SetTrackVolume { .. }
            | AppCommand::AdjustTrackVolume { .. }
            | AppCommand::SetTrackPan { .. }
            | AppCommand::AdjustTrackPan { .. }
            | AppCommand::SetTrackSend { .. }
            | AppCommand::AdjustTrackSend { .. }
            | AppCommand::SetTrackSends { .. }
            | AppCommand::SetMidiFxParam { .. }
            | AppCommand::SetBusVolume { .. }
            | AppCommand::ToggleBusMute { .. }
            | AppCommand::ToggleBusSolo { .. }
            | AppCommand::SetMasterVolume { .. }
            | AppCommand::AdjustMasterVolume { .. }
            | AppCommand::SetReverbParam { .. }
            | AppCommand::MacroCreate { .. }
            | AppCommand::MacroEnsure { .. }
            | AppCommand::MacroDelete { .. }
            | AppCommand::MacroRename { .. }
            | AppCommand::MacroSetValue { .. }
            | AppCommand::MacroRelease { .. }
            | AppCommand::MacroMapParam { .. }
            | AppCommand::MacroSetRange { .. }
            | AppCommand::MacroSetCurve { .. }
            | AppCommand::MacroUnmap { .. }
    )
}

pub(crate) fn execute_command(app: &mut App, cmd: AppCommand) {
    match cmd {
        // ── Pattern / step mutations ──────────────────────────────────────
        AppCommand::ToggleStep { track, step } => {
            app.clear_step_selection();
            app.state.toggle_step_and_clear_plocks(track, step);
        }

        AppCommand::SetStepActive {
            track,
            step,
            active,
        } => {
            app.state.pattern.patterns[track].set_step_active(step, active);
        }

        AppCommand::SetStepParam {
            track,
            step,
            param,
            value,
        } => {
            app.state.set_step_param(track, step, param, value);
        }

        AppCommand::AdjustStepParam {
            track,
            step,
            param,
            delta,
        } => {
            app.state.adjust_step_param(track, step, param, delta);
        }

        AppCommand::ClearStepPayload { track, step } => {
            app.state.clear_step_payload(track, step);
        }

        AppCommand::ClearSteps { track, steps } => {
            for step in steps {
                app.state.clear_step_payload(track, step);
            }
        }

        AppCommand::RotateSteps {
            track,
            steps,
            direction,
        } => {
            app.state.rotate_steps(track, &steps, direction);
        }

        AppCommand::PasteSteps {
            track,
            source_track,
            clipboard,
            dest_start,
            num_steps,
        } => {
            let preserve_audio_plocks = source_track == track;
            for (offset, snap) in &clipboard {
                let dest = dest_start + offset;
                if dest >= num_steps {
                    continue;
                }
                // Skip pasting an empty step over an existing active step
                if !snap.active && app.state.pattern.patterns[track].is_active(dest) {
                    continue;
                }
                let sanitized = sanitize_pasted_step_snapshot(snap, preserve_audio_plocks);
                app.state.restore_step_snapshot(track, dest, &sanitized);
            }
        }

        AppCommand::ShiftStepRange {
            track,
            lo,
            hi,
            new_lo,
        } => {
            app.state.move_step_range(track, lo, hi, new_lo);
        }

        AppCommand::TogglePianoNote { .. } => {
            unreachable!("piano-note edits execute through the recorded edit boundary")
        }

        AppCommand::DuplicateTrackPattern { track } => {
            app.state.duplicate_track_pattern_no_publish(track);
        }

        AppCommand::HalveTrackPattern { track } => {
            app.state.halve_track_pattern_no_publish(track);
        }

        AppCommand::SetTimebasePlock {
            track,
            step,
            timebase,
        } => match timebase {
            Some(tb) => app.state.pattern.timebase_plocks[track].set(step, tb),
            None => app.state.pattern.timebase_plocks[track].clear(step),
        },

        AppCommand::SetTimebasePlockMulti {
            track,
            steps,
            timebase,
        } => {
            for step in steps {
                app.state.pattern.timebase_plocks[track].set(step, timebase);
            }
        }

        AppCommand::ClearTimebasePlockMulti { track, steps } => {
            for step in steps {
                app.state.pattern.timebase_plocks[track].clear(step);
            }
        }

        // ── Track params ──────────────────────────────────────────────────
        AppCommand::ToggleTrackGate { track } => {
            app.state.pattern.track_params[track].toggle_gate();
        }

        AppCommand::ToggleTrackPolyphonic { track } => {
            app.state.pattern.track_params[track].toggle_polyphonic();
        }

        AppCommand::ToggleTrackMute { track } => {
            app.state.pattern.track_params[track].toggle_mute();
            app.push_track_mute(track);
        }

        AppCommand::ToggleTrackSolo { track } => {
            app.state.pattern.track_params[track].toggle_solo();
            app.push_track_solo_mutes();
        }

        AppCommand::AdjustTrackMaxPolyphony { track, delta } => {
            let tp = &app.state.pattern.track_params[track];
            let cur = tp.get_max_polyphony() as isize;
            tp.set_max_polyphony((cur + delta).max(1) as usize);
        }

        AppCommand::SetTrackMaxPolyphony { track, value } => {
            app.state.pattern.track_params[track].set_max_polyphony(value);
        }

        AppCommand::SetTrackAttack { track, ms } => {
            app.state.pattern.track_params[track].set_attack_ms(ms);
        }

        AppCommand::AdjustTrackAttack { track, delta } => {
            let tp = &app.state.pattern.track_params[track];
            tp.set_attack_ms(tp.get_attack_ms() + delta);
        }

        AppCommand::SetTrackRelease { track, ms } => {
            app.state.pattern.track_params[track].set_release_ms(ms);
        }

        AppCommand::AdjustTrackRelease { track, delta } => {
            let tp = &app.state.pattern.track_params[track];
            tp.set_release_ms(tp.get_release_ms() + delta);
        }

        AppCommand::SetTrackSwing { track, value } => {
            app.state.pattern.track_params[track].set_swing(value);
        }

        AppCommand::SetTrackSwingPlock { track, step, value } => match value {
            Some(value) => app.state.pattern.swing_plocks[track].set(step, value),
            None => app.state.pattern.swing_plocks[track].clear(step),
        },

        AppCommand::SetTrackSwingPlockMulti {
            track,
            steps,
            value,
        } => {
            for step in steps {
                app.state.pattern.swing_plocks[track].set(step, value);
            }
        }

        AppCommand::ClearTrackSwingPlockMulti { track, steps } => {
            for step in steps {
                app.state.pattern.swing_plocks[track].clear(step);
            }
        }

        AppCommand::AdjustTrackSwing { track, delta } => {
            let tp = &app.state.pattern.track_params[track];
            tp.set_swing(tp.get_swing() + delta);
        }

        AppCommand::SetTrackSwingResolution { track, resolution } => {
            app.state.pattern.track_params[track].set_swing_resolution(resolution);
        }

        AppCommand::SetTrackSwingResolutionPlock {
            track,
            step,
            resolution,
        } => match resolution {
            Some(resolution) => {
                app.state.pattern.swing_resolution_plocks[track].set(step, resolution)
            }
            None => app.state.pattern.swing_resolution_plocks[track].clear(step),
        },

        AppCommand::SetTrackSwingResolutionPlockMulti {
            track,
            steps,
            resolution,
        } => {
            for step in steps {
                app.state.pattern.swing_resolution_plocks[track].set(step, resolution);
            }
        }

        AppCommand::ClearTrackSwingResolutionPlockMulti { track, steps } => {
            for step in steps {
                app.state.pattern.swing_resolution_plocks[track].clear(step);
            }
        }

        AppCommand::NextTrackSwingResolution { track } => {
            app.state.pattern.track_params[track].next_swing_resolution();
        }

        AppCommand::PrevTrackSwingResolution { track } => {
            app.state.pattern.track_params[track].prev_swing_resolution();
        }

        AppCommand::SetTrackNumSteps { track, n } => {
            app.state.pattern.track_params[track].set_num_steps(n);
        }

        AppCommand::AdjustTrackNumSteps { track, delta } => {
            let tp = &app.state.pattern.track_params[track];
            let cur = tp.get_num_steps() as isize;
            tp.set_num_steps((cur + delta).max(1) as usize);
        }

        AppCommand::SetTrackVolume { track, value } => {
            app.state.pattern.track_params[track].set_volume(value);
            app.push_track_volume(track);
        }

        AppCommand::AdjustTrackVolume { track, delta } => {
            let tp = &app.state.pattern.track_params[track];
            tp.set_volume(tp.get_volume() + delta);
            app.push_track_volume(track);
        }

        AppCommand::SetTrackPan { track, value } => {
            app.state.pattern.track_params[track].set_pan(value);
            app.push_track_pan(track);
        }

        AppCommand::AdjustTrackPan { track, delta } => {
            let tp = &app.state.pattern.track_params[track];
            tp.set_pan(tp.get_pan() + delta);
            app.push_track_pan(track);
        }

        AppCommand::SetTrackSend { track, value } => {
            app.state.pattern.track_params[track].set_send(value);
            app.push_send_gain(track);
        }

        AppCommand::AdjustTrackSend { track, delta } => {
            let tp = &app.state.pattern.track_params[track];
            tp.set_send(tp.get_send() + delta);
            app.push_send_gain(track);
        }

        AppCommand::SetTrackOutput { track, output } => {
            app.state.pattern.track_params[track].set_output(output);
            app.graph_controller().apply_track_output_routing(track);
        }

        AppCommand::SetTrackSends { track, sends } => {
            app.state.pattern.track_params[track].set_sends(sends);
            app.graph_controller().apply_track_bus_sends(track);
        }

        AppCommand::SetBusVolume { bus, value } => {
            if let Some(channel) = app.buses.iter_mut().find(|channel| channel.id == bus) {
                channel.volume = value.clamp(0.0, 1.0);
                app.push_bus_volume(bus);
            }
        }

        AppCommand::ToggleBusMute { bus } => {
            if let Some(channel) = app.buses.iter_mut().find(|channel| channel.id == bus) {
                channel.mute = !channel.mute;
                app.push_bus_mute(bus);
            }
        }

        AppCommand::ToggleBusSolo { bus } => {
            if let Some(channel) = app.buses.iter_mut().find(|channel| channel.id == bus) {
                channel.solo = !channel.solo;
                app.push_bus_solo_mutes();
            }
        }

        AppCommand::SetMasterVolume { value } => {
            app.state
                .transport
                .master_volume
                .store(value.clamp(0.0, 2.0).to_bits(), Ordering::Relaxed);
            app.push_master_volume();
        }

        AppCommand::AdjustMasterVolume { delta } => {
            let current = f32::from_bits(app.state.transport.master_volume.load(Ordering::Relaxed));
            app.state.transport.master_volume.store(
                (current + delta).clamp(0.0, 2.0).to_bits(),
                Ordering::Relaxed,
            );
            app.push_master_volume();
        }

        AppCommand::SetReverbParam { param_idx, value } => {
            app.set_reverb_param_unrecorded(param_idx, value);
        }

        AppCommand::SetTrackTimebase { track, timebase } => {
            app.state.pattern.track_params[track].set_timebase(timebase);
        }

        AppCommand::NextTrackTimebase { track } => {
            app.state.pattern.track_params[track].next_timebase();
        }

        AppCommand::PrevTrackTimebase { track } => {
            app.state.pattern.track_params[track].prev_timebase();
        }

        AppCommand::SetTrackFtsScale { track, scale_idx } => {
            app.state.pattern.track_params[track].set_fts_scale(scale_idx);
        }

        AppCommand::SetTrackAccumIdx {
            track,
            idx,
            default_limit,
            script_name,
        } => {
            app.state.pattern.track_params[track].set_accumulator_idx(idx);
            app.state.pattern.track_params[track].set_script_accumulator_name(script_name);
            if let Some(limit) = default_limit {
                app.state.pattern.track_params[track].set_accum_limit(limit);
            }
            app.state.request_accumulator_reset(track);
        }

        AppCommand::SetTrackAccumLimit { track, value } => {
            app.state.pattern.track_params[track].set_accum_limit(value);
        }

        AppCommand::AdjustTrackAccumLimit { track, delta } => {
            let tp = &app.state.pattern.track_params[track];
            tp.set_accum_limit(tp.get_accum_limit() + delta);
        }

        AppCommand::SetTrackAccumMode { track, mode } => {
            app.state.pattern.track_params[track].set_accum_mode(mode);
        }

        AppCommand::SetTrackMuteGroup { track, group } => {
            app.state.pattern.track_params[track].set_mute_group(group);
        }

        AppCommand::SetTrackGlobalTranspose { track, enabled } => {
            app.state.pattern.track_params[track].set_global_transpose(enabled);
        }

        // ── Effect params ─────────────────────────────────────────────────
        AppCommand::SetEffectParam {
            track,
            slot_idx,
            param_idx,
            value,
        } => {
            let value = app
                .graph
                .effect_descriptors
                .get(track)
                .and_then(|descriptors| descriptors.get(slot_idx))
                .and_then(|descriptor| descriptor.params.get(param_idx))
                .map(|param| param.clamp(value))
                .unwrap_or(value);
            let is_sidechain = app
                .graph
                .effect_descriptors
                .get(track)
                .and_then(|descriptors| descriptors.get(slot_idx))
                .and_then(|descriptor| descriptor.params.get(param_idx))
                .is_some_and(|param| {
                    matches!(param.host_control, Some(HostControl::FxSidechain { .. }))
                });
            if is_sidechain {
                app.apply_effect_sidechain_selection(
                    track,
                    slot_idx,
                    param_idx,
                    value.round().max(0.0) as usize,
                );
            }
            let chain = &app.state.pattern.effect_chains[track];
            if let Some(slot) = chain.get(slot_idx) {
                slot.defaults.set(param_idx, value);
                if !is_sidechain {
                    app.send_effective_slot_param(track, slot_idx, param_idx);
                }
                sync_effect_mod_active_default(app, track, slot_idx, param_idx);
            }
        }

        AppCommand::SetEffectPlock {
            track,
            step,
            slot_idx,
            param_idx,
            value,
        } => {
            let value = app.graph.effect_descriptors.get(track)
                .and_then(|descriptors| descriptors.get(slot_idx))
                .and_then(|descriptor| descriptor.params.get(param_idx))
                .map(|param| param.clamp(value))
                .unwrap_or(value);
            let chain = &app.state.pattern.effect_chains[track];
            if let Some(slot) = chain.get(slot_idx) {
                slot.set_plock(step, param_idx, value);
                sync_effect_mod_active_plock(app, track, step, slot_idx, param_idx);
            }
        }

        AppCommand::SetEffectPlockMulti {
            track,
            steps,
            slot_idx,
            param_idx,
            value,
        } => {
            let value = app.graph.effect_descriptors.get(track)
                .and_then(|descriptors| descriptors.get(slot_idx))
                .and_then(|descriptor| descriptor.params.get(param_idx))
                .map(|param| param.clamp(value))
                .unwrap_or(value);
            let updated = app
                .state
                .pattern
                .effect_chains
                .get(track)
                .and_then(|chain| chain.get(slot_idx))
                .is_some_and(|slot| {
                    for step in &steps {
                        slot.set_plock(*step, param_idx, value);
                    }
                    true
                });
            if updated {
                for step in steps {
                    sync_effect_mod_active_plock(app, track, step, slot_idx, param_idx);
                }
            }
        }

        AppCommand::ClearEffectPlockMulti {
            track,
            steps,
            slot_idx,
            param_idx,
        } => {
            if let Some(slot) = app
                .state
                .pattern
                .effect_chains
                .get(track)
                .and_then(|chain| chain.get(slot_idx))
            {
                for step in &steps {
                    slot.plocks.clear_param(*step, param_idx);
                }
            }
            for step in steps {
                sync_effect_mod_active_plock(app, track, step, slot_idx, param_idx);
            }
        }

        AppCommand::SetEffectTensorCell {
            track,
            slot_idx,
            tensor_idx,
            cell_idx,
            value,
        } => {
            if let Some(slot) = app
                .state
                .pattern
                .effect_chains
                .get(track)
                .and_then(|chain| chain.get(slot_idx))
            {
                if let Some(values) = slot.tensor_params.set_default_cell(tensor_idx, cell_idx, value)
                {
                    app.send_effect_tensor_param(track, slot_idx, tensor_idx, &values);
                }
            }
        }

        AppCommand::SetEffectTensorPlockCellMulti {
            track, steps, slot_idx, tensor_idx, cell_idx, value,
        } => {
            if let Some(slot) = app.state.pattern.effect_chains.get(track).and_then(|chain| chain.get(slot_idx)) {
                for step in steps {
                    slot.tensor_params.set_plock_cell(step, tensor_idx, cell_idx, value);
                }
            }
        }

        AppCommand::ClearEffectTensorPlockMulti {
            track, steps, slot_idx, tensor_idx,
        } => {
            if let Some(slot) = app.state.pattern.effect_chains.get(track).and_then(|chain| chain.get(slot_idx)) {
                for step in steps {
                    slot.tensor_params.clear_plock(step, tensor_idx);
                }
            }
        }

        AppCommand::SetMidiFxParam {
            track,
            slot_idx,
            param_idx,
            value,
        } => {
            let value = app.state.pattern.track_params.get(track)
                .and_then(|params| params.midi_fx_chain().get(slot_idx).cloned())
                .and_then(|name| crate::lisp_host::load_midi_fx_descriptor(&name))
                .and_then(|descriptor| descriptor.params.get(param_idx).cloned())
                .map(|param| param.clamp(value))
                .unwrap_or(value);
            if let Some(slot) = app
                .state
                .pattern
                .midi_fx_slots
                .get(track)
                .and_then(|slots| slots.get(slot_idx))
            {
                slot.defaults.set(param_idx, value);
            }
        }

        AppCommand::SetMidiFxPlockMulti {
            track,
            steps,
            slot_idx,
            param_idx,
            value,
        } => {
            let value = app.state.pattern.track_params.get(track)
                .and_then(|params| params.midi_fx_chain().get(slot_idx).cloned())
                .and_then(|name| crate::lisp_host::load_midi_fx_descriptor(&name))
                .and_then(|descriptor| descriptor.params.get(param_idx).cloned())
                .map(|param| param.clamp(value))
                .unwrap_or(value);
            if let Some(slot) = app
                .state
                .pattern
                .midi_fx_slots
                .get(track)
                .and_then(|slots| slots.get(slot_idx))
            {
                for step in steps {
                    slot.set_plock(step, param_idx, value);
                }
            }
        }

        AppCommand::ClearMidiFxPlockMulti {
            track,
            steps,
            slot_idx,
            param_idx,
        } => {
            if let Some(slot) = app
                .state
                .pattern
                .midi_fx_slots
                .get(track)
                .and_then(|slots| slots.get(slot_idx))
            {
                for step in steps {
                    slot.plocks.clear_param(step, param_idx);
                }
            }
        }

        AppCommand::SetMidiFxTensorCell {
            track, slot_idx, tensor_idx, cell_idx, value,
        } => {
            if let Some(slot) = app.state.pattern.midi_fx_slots.get(track).and_then(|slots| slots.get(slot_idx)) {
                slot.tensor_params.set_default_cell(tensor_idx, cell_idx, value);
            }
        }

        AppCommand::SetMidiFxTensorPlockCellMulti {
            track, steps, slot_idx, tensor_idx, cell_idx, value,
        } => {
            if let Some(slot) = app.state.pattern.midi_fx_slots.get(track).and_then(|slots| slots.get(slot_idx)) {
                for step in steps {
                    slot.tensor_params.set_plock_cell(step, tensor_idx, cell_idx, value);
                }
            }
        }

        AppCommand::ClearMidiFxTensorPlockMulti {
            track, steps, slot_idx, tensor_idx,
        } => {
            if let Some(slot) = app.state.pattern.midi_fx_slots.get(track).and_then(|slots| slots.get(slot_idx)) {
                for step in steps {
                    slot.tensor_params.clear_plock(step, tensor_idx);
                }
            }
        }

        // ── Instrument params ─────────────────────────────────────────────
        AppCommand::SetInstrumentParam {
            track,
            param_idx,
            value,
        } => {
            let value = app.graph.instrument_descriptors.get(track)
                .and_then(|descriptor| descriptor.params.get(param_idx))
                .map(|param| param.clamp(value))
                .unwrap_or(value);
            let slot = &app.state.pattern.instrument_slots[track];
            slot.defaults.set(param_idx, value);
            app.send_effective_instrument_param(track, param_idx);
            sync_instrument_mod_active_default(app, track, param_idx);
            app.mark_track_sound_dirty(track);
        }
        AppCommand::SetInstrumentPlock {
            track,
            step,
            param_idx,
            value,
        } => {
            let value = app.graph.instrument_descriptors.get(track)
                .and_then(|descriptor| descriptor.params.get(param_idx))
                .map(|param| param.clamp(value))
                .unwrap_or(value);
            app.state.pattern.instrument_slots[track].set_plock(step, param_idx, value);
            sync_instrument_mod_active_plock(app, track, step, param_idx);
        }

        AppCommand::SetInstrumentPlockMulti {
            track,
            steps,
            param_idx,
            value,
        } => {
            let value = app.graph.instrument_descriptors.get(track)
                .and_then(|descriptor| descriptor.params.get(param_idx))
                .map(|param| param.clamp(value))
                .unwrap_or(value);
            for step in steps {
                app.state.pattern.instrument_slots[track].set_plock(step, param_idx, value);
                sync_instrument_mod_active_plock(app, track, step, param_idx);
            }
        }

        AppCommand::ClearInstrumentPlockMulti {
            track,
            steps,
            param_idx,
        } => {
            for step in steps {
                app.state.pattern.instrument_slots[track]
                    .plocks
                    .clear_param(step, param_idx);
                sync_instrument_mod_active_plock(app, track, step, param_idx);
            }
        }

        AppCommand::SetInstrumentKeyLock {
            track,
            note,
            param_idx,
            value,
        } => {
            let value = app.graph.instrument_descriptors.get(track)
                .and_then(|descriptor| descriptor.params.get(param_idx))
                .map(|param| param.clamp(value))
                .unwrap_or(value);
            app.state.pattern.instrument_slots[track].set_key_lock(note, param_idx, value);
            sync_instrument_mod_active_key_lock(app, track, note, param_idx);
            app.mark_track_sound_dirty(track);
        }

        AppCommand::SetInstrumentKeyLockMulti {
            track,
            notes,
            param_idx,
            value,
        } => {
            let value = app.graph.instrument_descriptors.get(track)
                .and_then(|descriptor| descriptor.params.get(param_idx))
                .map(|param| param.clamp(value))
                .unwrap_or(value);
            for note in notes {
                app.state.pattern.instrument_slots[track].set_key_lock(note, param_idx, value);
                sync_instrument_mod_active_key_lock(app, track, note, param_idx);
            }
            app.mark_track_sound_dirty(track);
        }

        AppCommand::ClearInstrumentKeyLock {
            track,
            note,
            param_idx,
        } => {
            app.state.pattern.instrument_slots[track].clear_key_lock(note, param_idx);
            sync_instrument_mod_active_key_lock(app, track, note, param_idx);
            app.mark_track_sound_dirty(track);
        }

        AppCommand::ClearInstrumentKeyLocksForNote { track, note } => {
            app.state.pattern.instrument_slots[track].clear_note_key_locks(note);
            app.mark_track_sound_dirty(track);
        }

        AppCommand::StampInstrumentKeyLockVariant { track, notes, key } => {
            if track < app.state.pattern.instrument_slots.len() {
                for note in notes {
                    app.state.pattern.instrument_slots[track].clear_note_key_locks(note);
                    for entry in &key.entries {
                        if entry.domain != PlockVariantDomain::InstrumentKeyLock
                            || entry.slot != 0
                            || entry.cell.is_some()
                        {
                            continue;
                        }
                        let param_count = app.state.pattern.instrument_slots[track]
                            .num_params
                            .load(Ordering::Relaxed)
                            as usize;
                        if entry.param >= param_count {
                            continue;
                        }
                        app.state.pattern.instrument_slots[track].set_key_lock(
                            note,
                            entry.param,
                            f32::from_bits(entry.value_bits),
                        );
                        sync_instrument_mod_active_key_lock(app, track, note, entry.param);
                    }
                }
                let _ = app
                    .state
                    .reconcile_key_lock_variant_registry_for_track(track);
                app.mark_track_sound_dirty(track);
            }
        }

        AppCommand::ClearInstrumentKeyLockVariantsForNotes { track, notes } => {
            if track < app.state.pattern.instrument_slots.len() {
                for note in notes {
                    app.state.pattern.instrument_slots[track].clear_note_key_locks(note);
                }
                let _ = app
                    .state
                    .reconcile_key_lock_variant_registry_for_track(track);
                app.mark_track_sound_dirty(track);
            }
        }

        AppCommand::SetInstrumentTensorCell {
            track,
            tensor_idx,
            cell_idx,
            value,
        } => {
            let slot = &app.state.pattern.instrument_slots[track];
            if let Some(values) = slot
                .tensor_params
                .set_default_cell(tensor_idx, cell_idx, value)
            {
                app.send_instrument_tensor_param(track, tensor_idx, &values);
                app.mark_track_sound_dirty(track);
            }
        }

        AppCommand::SetInstrumentTensorPlockCellMulti {
            track,
            steps,
            tensor_idx,
            cell_idx,
            value,
        } => {
            let slot = &app.state.pattern.instrument_slots[track];
            for step in steps {
                slot.tensor_params
                    .set_plock_cell(step, tensor_idx, cell_idx, value);
            }
        }

        AppCommand::ClearInstrumentTensorPlockMulti {
            track,
            steps,
            tensor_idx,
        } => {
            let slot = &app.state.pattern.instrument_slots[track];
            for step in steps {
                slot.tensor_params.clear_plock(step, tensor_idx);
            }
        }

        AppCommand::SetInstrumentBaseNoteOffset { track, value } => {
            app.state.pattern.instrument_base_note_offsets[track]
                .store(value.to_bits(), Ordering::Relaxed);
        }

        AppCommand::SetRackSlotGain {
            track,
            slot_idx,
            value,
        } => {
            app.set_rack_slot_gain(track, slot_idx, value);
        }

        AppCommand::SetRackSlotPan {
            track,
            slot_idx,
            value,
        } => {
            app.set_rack_slot_pan(track, slot_idx, value);
        }

        AppCommand::SetRackSlotMute {
            track,
            slot_idx,
            value,
        } => {
            app.set_rack_slot_mute(track, slot_idx, value);
        }

        AppCommand::SetRackSlotSolo {
            track,
            slot_idx,
            value,
        } => {
            app.set_rack_slot_solo(track, slot_idx, value);
        }

        AppCommand::SetRackSlotMaxPolyphony {
            track,
            slot_idx,
            value,
        } => {
            app.set_rack_slot_max_polyphony(track, slot_idx, value);
        }

        AppCommand::SetRackSlotChokeGroup {
            track,
            slot_idx,
            value,
        } => {
            app.set_rack_slot_choke_group(track, slot_idx, value);
        }

        AppCommand::SetRackSlotBaseNoteOffset {
            track,
            slot_idx,
            value,
        } => {
            app.set_rack_slot_base_note_offset(track, slot_idx, value);
        }

        AppCommand::SetRackSlotParamPlock {
            track,
            slot_idx,
            step,
            param,
            value,
        } => {
            app.set_rack_slot_param_plock(track, slot_idx, step, param, value);
        }

        AppCommand::SetRackSlotParamPlockMulti {
            track,
            slot_idx,
            steps,
            param,
            value,
        } => {
            for step in steps {
                app.set_rack_slot_param_plock(track, slot_idx, step, param, value);
            }
        }

        AppCommand::SetRackSlotInstrumentParam {
            track,
            slot_idx,
            param_idx,
            value,
        } => {
            app.set_rack_slot_instrument_param(track, slot_idx, param_idx, value);
        }

        AppCommand::SetRackSlotEffectParam {
            track,
            rack_slot_idx,
            effect_slot_idx,
            param_idx,
            value,
        } => {
            let _ = app.set_rack_slot_effect_param(
                track,
                rack_slot_idx,
                effect_slot_idx,
                param_idx,
                value,
            );
        }

        AppCommand::SetRackSlotInstrumentPlock {
            track,
            slot_idx,
            step,
            param_idx,
            value,
        } => {
            app.set_rack_slot_instrument_plock(track, slot_idx, step, param_idx, value);
        }

        AppCommand::SetRackSlotInstrumentPlockMulti {
            track,
            slot_idx,
            steps,
            param_idx,
            value,
        } => {
            for step in steps {
                app.set_rack_slot_instrument_plock(track, slot_idx, step, param_idx, value);
            }
        }

        AppCommand::SetRackMacroPlockMulti {
            track,
            steps,
            macro_idx,
            value,
        } => {
            if let Some(id) = crate::sequencer::RackMacroId::from_index(macro_idx) {
                app.set_rack_macro_plocks(track, id, &steps, value);
            }
        }

        AppCommand::ClearRackMacroPlockMulti {
            track,
            steps,
            macro_idx,
        } => {
            if let Some(id) = crate::sequencer::RackMacroId::from_index(macro_idx) {
                for step in steps {
                    app.clear_rack_macro_plock(track, id, step);
                }
            }
        }

        AppCommand::SetRackSlotEffectPlockMulti {
            track,
            steps,
            rack_slot_idx,
            effect_slot_idx,
            param_idx,
            value,
        } => {
            let _ = app.set_rack_slot_effect_plocks_no_publish(
                track,
                rack_slot_idx,
                effect_slot_idx,
                &steps,
                param_idx,
                value,
            );
        }

        AppCommand::ClearRackSlotEffectPlockMulti {
            track,
            steps,
            rack_slot_idx,
            effect_slot_idx,
            param_idx,
        } => {
            app.state.update_rack_slot_in_current_pattern(
                track,
                rack_slot_idx,
                |slot| {
                    if let Some(effect) = slot.effect_slots.get_mut(effect_slot_idx) {
                        for step in &steps {
                            effect.clear_plock(*step, param_idx);
                        }
                    }
                },
            );
        }

        // ── Project-global macros ─────────────────────────────────────────
        AppCommand::MacroCreate { name } => {
            if let Err(error) = app
                .macro_engine
                .create_macro(name, crate::macro_engine::MacroKind::Mapped)
            {
                eprintln!("macro-create failed: {error:?}");
            }
        }
        AppCommand::MacroCreateScene { name, target_scene } => {
            let config = crate::macro_engine::SceneMacroConfig {
                target_scene,
                morph_params: true,
                steal_patterns: false,
                quantize: crate::macro_engine::StealQuantize::Bar,
                track_mask: None,
            };
            if let Err(error) = app
                .macro_engine
                .create_macro(name, crate::macro_engine::MacroKind::Scene(config))
            {
                eprintln!("macro-create-scene failed: {error:?}");
            }
        }
        AppCommand::MacroSceneConfig { id, config } => {
            app.cancel_scene_macro(id);
            match app.macro_engine.set_scene_config(id, config) {
                Ok(touched) => app.send_macro_targets(touched),
                Err(error) => eprintln!("macro-scene-config failed: {error:?}"),
            }
        }
        AppCommand::MacroEnsure { key, name } => {
            if let Err(error) = app.macro_engine.ensure_macro(key, name) {
                eprintln!("macro-ensure failed: {error:?}");
            }
        }
        AppCommand::MacroDelete { id } => {
            app.cancel_scene_macro(id);
            match app.macro_engine.delete_macro(id) {
                Ok(touched) => app.send_macro_targets(touched),
                Err(error) => eprintln!("macro-delete failed: {error:?}"),
            }
        }
        AppCommand::MacroRename { id, name } => {
            if let Err(error) = app.macro_engine.rename_macro(id, name) {
                eprintln!("macro-rename failed: {error:?}");
            }
        }
        AppCommand::MacroSetValue { id, value } => app.set_macro_value(id, value),
        AppCommand::MacroRelease { id } => app.release_macro(id),
        AppCommand::ScenePushBegin {
            target_scene,
            value,
        } => app.begin_scene_push(target_scene, value),
        AppCommand::ScenePushSetValue { value } => app.set_scene_push_value(value),
        AppCommand::ScenePushEnd => app.end_scene_push(),
        AppCommand::MacroMapParam { id, track, target } => {
            if let Err(error) = app.map_macro_param(id, track, target) {
                eprintln!("macro-map-param failed: {error:?}");
            }
        }
        AppCommand::MacroSetRange {
            id,
            mapping_idx,
            min,
            max,
        } => match app
            .macro_engine
            .set_mapping_range(id, mapping_idx, min, max)
        {
            Ok(touched) => app.send_macro_targets(touched),
            Err(error) => eprintln!("macro-set-range failed: {error:?}"),
        },
        AppCommand::MacroSetCurve {
            id,
            mapping_idx,
            curve,
        } => match app.macro_engine.set_mapping_curve(id, mapping_idx, curve) {
            Ok(touched) => app.send_macro_targets(touched),
            Err(error) => eprintln!("macro-set-curve failed: {error:?}"),
        },
        AppCommand::MacroUnmap { id, mapping_idx } => {
            match app.macro_engine.remove_mapping(id, mapping_idx) {
                Ok(touched) => app.send_macro_targets(touched),
                Err(error) => eprintln!("macro-unmap failed: {error:?}"),
            }
        }

        // ── Transport ─────────────────────────────────────────────────────
        AppCommand::TogglePlay => {
            app.state.toggle_play_no_publish();
        }

        AppCommand::SetBpm { bpm } => {
            app.state
                .transport
                .bpm
                .store(bpm.clamp(20, 999), Ordering::Relaxed);
            app.push_all_delay_bpm();
        }

        AppCommand::AdjustRecordQuantizeThresh { delta } => {
            let current = f32::from_bits(
                app.state
                    .transport
                    .record_quantize_thresh
                    .load(Ordering::Relaxed),
            );
            app.state.transport.record_quantize_thresh.store(
                (current + delta).clamp(0.1, 0.9).to_bits(),
                Ordering::Relaxed,
            );
        }
    }
}
