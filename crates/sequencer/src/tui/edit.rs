use crate::plock_variants::PlockVariantRegistry;
use crate::sequencer::{StepCellSnapshot, TrackId, TrackPatternId, MAX_STEPS};

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
    ReplayFailed(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EditOutcome {
    NoOp,
    Applied(HistoryMove),
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MutationEffects {
    pub publish_scheduler: bool,
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
}

impl ResolvedStepCommand<'_> {
    fn affected_steps(&self) -> &[usize] {
        match self {
            Self::Toggle { step }
            | Self::SetActive { step, .. }
            | Self::SetParam { step, .. }
            | Self::AdjustParam { step, .. } => std::slice::from_ref(step),
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
    let affected = resolved.affected_steps();
    if affected.is_empty() {
        return Ok(EditOutcome::NoOp);
    }
    let (before, registry_before) = app
        .state
        .capture_pattern_step_cells(track, pattern_id, affected)
        .map_err(EditError::ReplayFailed)?;

    execute_step_command_no_publish(app, track, &resolved);
    let (after, _) = match app
        .state
        .capture_pattern_step_cells(track, pattern_id, affected)
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
        .capture_pattern_step_cells(track, pattern_id, affected)
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
        resolved.label(),
        None,
        EditPatch::StepCells(patch),
        retained_bytes,
    );
    Ok(EditOutcome::Applied(history_move))
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
}
