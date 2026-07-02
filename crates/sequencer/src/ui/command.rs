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

use crate::sequencer::{
    RackSlotParam, StepParam, StepSnapshot, SwingResolution, Timebase, TrackOutput,
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
    let active = desc
        .instrument_modulation_targets
        .iter()
        .filter(|target| target.active_param_idx == Some(active_param_idx))
        .any(|target| {
            slot.plocks
                .get(step, target.depth_param_idx)
                .unwrap_or_else(|| slot.defaults.get(target.depth_param_idx))
                .abs()
                > f32::EPSILON
        });
    slot.set_plock(step, active_param_idx, if active { 1.0 } else { 0.0 });
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
    let active = desc
        .instrument_modulation_targets
        .iter()
        .filter(|target| target.active_param_idx == Some(active_param_idx))
        .any(|target| {
            slot.plocks
                .get(step, target.depth_param_idx)
                .unwrap_or_else(|| slot.defaults.get(target.depth_param_idx))
                .abs()
                > f32::EPSILON
        });
    slot.set_plock(step, active_param_idx, if active { 1.0 } else { 0.0 });
}

fn sanitize_pasted_step_snapshot(
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

    AdjustTrackMaxPolyphony {
        track: usize,
        delta: isize,
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

    /// Set master volume; also pushes to the live audio graph.
    SetMasterVolume {
        value: f32,
    },
    AdjustMasterVolume {
        delta: f32,
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

/// Execute `cmd` against `app`, calling
/// `app.state.publish_scheduler_snapshot()` afterwards when the command
/// mutated sequencer/transport state.
///
/// Audio-graph side-effects (volume, pan, send, reverb, effect params) are
/// performed inside this function alongside the state mutation.
#[allow(dead_code)]
pub fn apply_command(app: &mut App, cmd: AppCommand) {
    let needs_publish = command_mutates_sequencer_state(&cmd);

    execute_command(app, cmd);

    if needs_publish {
        app.state.publish_scheduler_snapshot();
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use super::{
        apply_command, command_mutates_sequencer_state, sanitize_pasted_step_snapshot, AppCommand,
    };
    use crate::audiograph::LiveGraphPtr;
    use crate::effects::{
        EffectDescriptor, EffectSlotSnapshot, InstrumentModulationTarget, ParamDescriptor,
        ParamKind, ParamScaling, TensorParamDescriptor,
    };
    use crate::recorder::MasterRecorder;
    use crate::sequencer::{
        default_empty_effect_chain, CustomInstrumentRunMode, InstrumentType, RackRouting,
        RackSlotParam, RackSlotParamPlocks, RackSlotSnapshot, RackTrackSnapshot, SequencerState,
        StepSlotPlocks, StepSnapshot, SwingResolution, Timebase, TrackSendSnapshot,
        TrackSoundState, NUM_PARAMS,
    };
    use crate::ui::{App, AudioBuses};

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
            RackTrackSnapshot {
                routing: RackRouting::Broadcast,
                slots: vec![RackSlotSnapshot {
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
                    track_sound_state: TrackSoundState::default(),
                    sample_id: Some((1, "test.wav".to_string(), 44_100)),
                }],
            },
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
            effect_plocks: vec![StepSlotPlocks {
                params: vec![Some(0.1), None, Some(0.9)],
            }],
            instrument_plocks: StepSlotPlocks {
                params: vec![Some(0.2), Some(0.8)],
            },
            rack_slot_param_plocks: vec![StepSlotPlocks {
                params: vec![Some(12.0), Some(0.4), None],
            }],
            rack_slot_instrument_plocks: vec![StepSlotPlocks {
                params: vec![Some(0.3), None, Some(0.7)],
            }],
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
            .effect_plocks
            .iter()
            .flat_map(|plocks| plocks.params.iter())
            .all(Option::is_none));
        assert!(sanitized
            .instrument_plocks
            .params
            .iter()
            .all(Option::is_none));
        assert!(sanitized
            .rack_slot_param_plocks
            .iter()
            .flat_map(|plocks| plocks.params.iter())
            .all(Option::is_none));
        assert!(sanitized
            .rack_slot_instrument_plocks
            .iter()
            .flat_map(|plocks| plocks.params.iter())
            .all(Option::is_none));
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
            effect_plocks: vec![StepSlotPlocks {
                params: vec![Some(0.1), None, Some(0.9)],
            }],
            instrument_plocks: StepSlotPlocks {
                params: vec![Some(0.2), Some(0.8)],
            },
            rack_slot_param_plocks: vec![StepSlotPlocks {
                params: vec![Some(12.0), Some(0.4), None],
            }],
            rack_slot_instrument_plocks: vec![StepSlotPlocks {
                params: vec![Some(0.3), None, Some(0.7)],
            }],
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
}

/// Returns `true` for commands whose state changes must be visible to the
/// scheduler's immutable pattern snapshot.
///
/// Continuous live-mixer controls are pushed directly to the audio graph by
/// `execute_command`. Publishing a full scheduler snapshot for each drag tick
/// makes the app loop perform broad pattern/UI sync work even though event
/// scheduling cannot observe those fields.
fn command_mutates_sequencer_state(cmd: &AppCommand) -> bool {
    !matches!(
        cmd,
        AppCommand::SetTrackVolume { .. }
            | AppCommand::AdjustTrackVolume { .. }
            | AppCommand::SetTrackPan { .. }
            | AppCommand::AdjustTrackPan { .. }
            | AppCommand::SetTrackSend { .. }
            | AppCommand::AdjustTrackSend { .. }
            | AppCommand::SetTrackSends { .. }
            | AppCommand::SetMasterVolume { .. }
            | AppCommand::AdjustMasterVolume { .. }
    )
}

fn execute_command(app: &mut App, cmd: AppCommand) {
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

        AppCommand::DuplicateTrackPattern { track } => {
            app.state.duplicate_track_pattern(track);
        }

        AppCommand::HalveTrackPattern { track } => {
            app.state.halve_track_pattern(track);
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

        AppCommand::AdjustTrackMaxPolyphony { track, delta } => {
            let tp = &app.state.pattern.track_params[track];
            let cur = tp.get_max_polyphony() as isize;
            tp.set_max_polyphony((cur + delta).max(1) as usize);
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
        } => {
            app.state.pattern.track_params[track].set_accumulator_idx(idx);
            if let Some(limit) = default_limit {
                app.state.pattern.track_params[track].set_accum_limit(limit);
            }
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

        // ── Effect params ─────────────────────────────────────────────────
        AppCommand::SetEffectParam {
            track,
            slot_idx,
            param_idx,
            value,
        } => {
            let chain = &app.state.pattern.effect_chains[track];
            if let Some(slot) = chain.get(slot_idx) {
                slot.defaults.set(param_idx, value);
                app.send_slot_param(track, slot_idx, param_idx, value);
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

        // ── Instrument params ─────────────────────────────────────────────
        AppCommand::SetInstrumentParam {
            track,
            param_idx,
            value,
        } => {
            let slot = &app.state.pattern.instrument_slots[track];
            slot.defaults.set(param_idx, value);
            app.send_instrument_param(track, param_idx, value);
            sync_instrument_mod_active_default(app, track, param_idx);
            app.mark_track_sound_dirty(track);
        }

        AppCommand::SetInstrumentPlock {
            track,
            step,
            param_idx,
            value,
        } => {
            app.state.pattern.instrument_slots[track].set_plock(step, param_idx, value);
            sync_instrument_mod_active_plock(app, track, step, param_idx);
        }

        AppCommand::SetInstrumentPlockMulti {
            track,
            steps,
            param_idx,
            value,
        } => {
            for step in steps {
                app.state.pattern.instrument_slots[track].set_plock(step, param_idx, value);
                sync_instrument_mod_active_plock(app, track, step, param_idx);
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

        // ── Transport ─────────────────────────────────────────────────────
        AppCommand::TogglePlay => {
            app.state.toggle_play();
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
