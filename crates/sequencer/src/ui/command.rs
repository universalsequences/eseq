//! `AppCommand` — unified mutation boundary for all sequencer state changes.
//!
//! All mutations to `app.state.pattern.*` or `app.state.transport.*` that need
//! to be visible to the audio thread should go through `apply_command`.  After
//! executing the command, `apply_command` calls
//! `app.state.publish_scheduler_snapshot()` so that any future snapshot-based
//! audio-thread readers can pick up the change.  (Currently `publish` is a
//! cheap no-op because the audio thread reads atomics directly; the hook is
//! here for the planned Arc<SequencerSnapshot> architecture.)
//!
//! Pure UI-state changes (cursor movement, mode changes, etc.) can also be
//! routed through `apply_command` for uniformity — they just don't trigger a
//! publish.

use std::sync::atomic::Ordering;

use crate::sequencer::{
    StepParam, StepSnapshot, SwingResolution, Timebase, TrackOutput, TrackSendSnapshot,
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
    slot.plocks
        .set(step, active_param_idx, if active { 1.0 } else { 0.0 });
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

    /// Set the instrument base-note offset.
    SetInstrumentBaseNoteOffset {
        track: usize,
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
    use super::sanitize_pasted_step_snapshot;
    use crate::sequencer::{StepSlotPlocks, StepSnapshot, SwingResolution, Timebase, NUM_PARAMS};

    #[test]
    fn paste_sanitizer_clears_audio_plocks_but_keeps_sequencer_plocks() {
        let mut params = [0.0; NUM_PARAMS];
        params[0] = 0.75;
        let snapshot = StepSnapshot {
            active: true,
            params,
            chord: vec![0.0, 7.0],
            timebase: Some(Timebase::Eighth),
            swing: Some(62.0),
            swing_resolution: Some(SwingResolution::Eighth),
            effect_plocks: vec![StepSlotPlocks {
                params: vec![Some(0.1), None, Some(0.9)],
            }],
            instrument_plocks: StepSlotPlocks {
                params: vec![Some(0.2), Some(0.8)],
            },
        };

        let sanitized = sanitize_pasted_step_snapshot(&snapshot, false);

        assert!(sanitized.active);
        assert_eq!(sanitized.params, params);
        assert_eq!(sanitized.chord, vec![0.0, 7.0]);
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
    }

    #[test]
    fn paste_sanitizer_preserves_audio_plocks_for_same_track_paste() {
        let snapshot = StepSnapshot {
            active: true,
            params: [0.0; NUM_PARAMS],
            chord: vec![],
            timebase: None,
            swing: None,
            swing_resolution: None,
            effect_plocks: vec![StepSlotPlocks {
                params: vec![Some(0.1), None, Some(0.9)],
            }],
            instrument_plocks: StepSlotPlocks {
                params: vec![Some(0.2), Some(0.8)],
            },
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
    }
}

/// Returns `true` for commands that write to `app.state.pattern` or
/// `app.state.transport` and therefore need a snapshot publish.
///
/// Currently all `AppCommand` variants mutate sequencer or transport state,
/// so this always returns `true`.  The function exists as a hook for future
/// pure-UI command variants that should NOT trigger a publish.
fn command_mutates_sequencer_state(_cmd: &AppCommand) -> bool {
    true
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
                slot.plocks.set(step, param_idx, value);
            }
        }

        AppCommand::SetEffectPlockMulti {
            track,
            steps,
            slot_idx,
            param_idx,
            value,
        } => {
            let chain = &app.state.pattern.effect_chains[track];
            if let Some(slot) = chain.get(slot_idx) {
                for step in steps {
                    slot.plocks.set(step, param_idx, value);
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
            app.state.pattern.instrument_slots[track]
                .plocks
                .set(step, param_idx, value);
            sync_instrument_mod_active_plock(app, track, step, param_idx);
        }

        AppCommand::SetInstrumentPlockMulti {
            track,
            steps,
            param_idx,
            value,
        } => {
            for step in steps {
                app.state.pattern.instrument_slots[track]
                    .plocks
                    .set(step, param_idx, value);
                sync_instrument_mod_active_plock(app, track, step, param_idx);
            }
        }

        AppCommand::SetInstrumentBaseNoteOffset { track, value } => {
            app.state.pattern.instrument_base_note_offsets[track]
                .store(value.to_bits(), Ordering::Relaxed);
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
