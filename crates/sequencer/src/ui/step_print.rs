/*!
Live step-param printing (bead eseq-jc9).

While the transport is playing with record on, touching a param in the
`*step*` buffer's parameter box arms PRINT mode for that param: the touched
value latches and is written onto every trigger step the playhead passes on
the focused track, until the transport pauses, recording turns off, or the
focused track changes. Untouched params and trigger-less steps are left
alone. Sweeping a picker while the pattern loops therefore lays the gesture
into the steps.

Writes go straight into live `step_data` (the same base-value lane the
*step* panel and live note recording use) and ride the open
`RecordingHistoryTransaction`, so a whole record pass undoes as one
"Record take" entry. UI updates use targeted `UiInvalidation::Step` pushes —
never a `ui_epoch` bump — matching the `set-step-param-history` perf
contract. The scheduler snapshot republish is deferred while a roll hold has
unpublished writes so the roll's double-trigger rule keeps holding; rolled
hits recorded while print mode is armed take the latched values
(`override_roll_hit`), which is what lets a roll+sweep lay down triggers
with the intended durations in one motion.
*/

use super::*;
use sequencer::sequencer::RollHitRecorded;

#[derive(Default)]
pub(crate) struct StepPrintState {
    /// Track the latch belongs to (the focused track at touch time).
    track: usize,
    /// Latched (param, value) pairs — only touched params print.
    values: Vec<(StepParam, f32)>,
    /// Playhead step printed last tick, so each boundary prints once and a
    /// frame that skipped steps can catch up across the gap (wrap-aware).
    last_step: Option<usize>,
    /// A latch/value change since the last tick: the step under the playhead
    /// reprints even without a boundary crossing, so the touch itself lands
    /// on the current step.
    touch_dirty: bool,
    /// Printed values are in live pattern state but not yet in the scheduler
    /// snapshot.
    dirty_unpublished: bool,
}

/// One frame's print result: which steps on which track took which params.
#[derive(Default)]
pub(crate) struct PrintedSteps {
    pub(crate) track: usize,
    pub(crate) steps: Vec<usize>,
    pub(crate) params: Vec<StepParam>,
}

impl StepPrintState {
    pub(crate) fn armed(&self) -> bool {
        !self.values.is_empty()
    }

    /// Arm-on-touch: latch a param value for printing. A touch on a
    /// different track than the current latch restarts the latch there.
    pub(crate) fn latch(&mut self, track: usize, param: StepParam, value: f32) {
        if self.armed() && self.track != track {
            self.disarm();
        }
        self.track = track;
        match self.values.iter_mut().find(|(existing, _)| *existing == param) {
            Some(entry) => entry.1 = value,
            None => self.values.push((param, value)),
        }
        self.touch_dirty = true;
    }

    /// End the print latch. Deliberately keeps `dirty_unpublished`: written
    /// steps still owe a snapshot publish even after the latch ends.
    pub(crate) fn disarm(&mut self) {
        self.track = 0;
        self.values.clear();
        self.last_step = None;
        self.touch_dirty = false;
    }

    /// Rolled hits recorded while print mode is armed take the latched
    /// values at record time — only for touched params on the armed track.
    pub(crate) fn override_roll_hit(&self, hit: &mut RollHitRecorded) {
        if !self.armed() || hit.track != self.track {
            return;
        }
        for (param, value) in &self.values {
            match param {
                StepParam::Transpose => hit.transpose = *value,
                StepParam::Velocity => hit.velocity = *value,
                StepParam::Duration => hit.duration_steps = *value,
                _ => {}
            }
        }
    }
}

/// One frame of print mode: disarm when the (recording && playing && same
/// focused track) gate fails, otherwise write the latched params onto every
/// trigger step the playhead has passed since the previous frame (walking
/// the wrap, so a loop boundary keeps printing). Pure state + pattern
/// mutation; invalidations/publishing live in `tick_step_print`.
pub(crate) fn print_pass(
    state: &SequencerState,
    print: &mut StepPrintState,
    focused_track: usize,
    recording: bool,
) -> PrintedSteps {
    if !print.armed() {
        return PrintedSteps::default();
    }
    if !recording
        || !state.is_playing()
        || print.track != focused_track
        || print.track >= state.pattern.patterns.len()
    {
        // Pause/stop, record-off, or a focus move ends printing cleanly —
        // the next touch during the next record+play session re-arms.
        print.disarm();
        return PrintedSteps::default();
    }
    let track = print.track;
    let num_steps = state.pattern.track_params[track]
        .get_num_steps()
        .clamp(1, sequencer::sequencer::MAX_STEPS);
    let playhead =
        state.transport.track_playheads[track].load(Ordering::Relaxed) as usize % num_steps;
    let mut boundary_steps = Vec::new();
    match print.last_step {
        // First tick after the touch: print the step under the playhead.
        None => boundary_steps.push(playhead),
        Some(prev) => {
            if prev != playhead {
                // Walk (prev, playhead] modulo the track length so skipped
                // frames and the loop wrap both keep the latch printing.
                let mut step = (prev + 1) % num_steps;
                for _ in 0..num_steps {
                    boundary_steps.push(step);
                    if step == playhead {
                        break;
                    }
                    step = (step + 1) % num_steps;
                }
            } else if print.touch_dirty {
                // Value moved within the same step: reprint it.
                boundary_steps.push(playhead);
            }
        }
    }
    print.last_step = Some(playhead);
    print.touch_dirty = false;
    let mut printed = Vec::new();
    for step in boundary_steps {
        // Trigger steps only — printing never creates triggers.
        if !state.pattern.patterns[track].is_active(step) {
            continue;
        }
        for (param, value) in &print.values {
            state.pattern.step_data[track].set(step, *param, *value);
        }
        printed.push(step);
    }
    if printed.is_empty() {
        return PrintedSteps::default();
    }
    print.dirty_unpublished = true;
    PrintedSteps {
        track,
        steps: printed,
        params: print.values.iter().map(|(param, _)| *param).collect(),
    }
}

/// Per-frame drive, mirroring `tick_roll_record`: run the print pass, push
/// targeted step invalidations for what landed (no `ui_epoch` bump — see the
/// `set-step-param-history` contract), and republish the scheduler snapshot
/// so the printed values are audible on the same pass's later steps. The
/// publish defers while a roll hold has unpublished pattern writes; the
/// roll's own release publish carries the printed values along. Returns
/// whether any step was printed this frame.
pub(crate) fn tick_step_print(shared: &SharedHandles) -> bool {
    let mut print = shared.step_print.lock().unwrap();
    if !print.armed() && !print.dirty_unpublished {
        return false;
    }
    let recording = shared.recording.load(Ordering::Relaxed);
    let focused_track = shared.current_track.load(Ordering::Relaxed);
    let printed = print_pass(&shared.state, &mut print, focused_track, recording);
    let roll_publish_pending = shared
        .roll_record
        .lock()
        .unwrap()
        .has_unpublished_writes();
    let publish = print.dirty_unpublished && !roll_publish_pending;
    if publish {
        print.dirty_unpublished = false;
    }
    drop(print);
    if publish {
        shared.state.publish_scheduler_snapshot();
    }
    for step in &printed.steps {
        for param in &printed.params {
            shared.ui_invalidations.push(UiInvalidation::Step {
                track: printed.track,
                step: *step,
                change: StepInvalidation::Param((*param).into()),
            });
            if *param == StepParam::Duration {
                shared.ui_invalidations.push(UiInvalidation::Step {
                    track: printed.track,
                    step: *step,
                    change: StepInvalidation::DurationSpan,
                });
            }
        }
    }
    !printed.steps.is_empty()
}

#[cfg(test)]
mod step_print_tests {
    use super::{print_pass, StepPrintState};
    use sequencer::sequencer::{
        default_empty_effect_chain, RollHitRecorded, SequencerState, StepParam,
    };
    use std::sync::atomic::Ordering;

    fn playing_state() -> SequencerState {
        let state = SequencerState::new(1, vec![default_empty_effect_chain()]);
        state.transport.playing.store(true, Ordering::Relaxed);
        state
    }

    fn set_playhead(state: &SequencerState, step: usize) {
        state.transport.track_playheads[0].store(step as u32, Ordering::Relaxed);
    }

    #[test]
    fn playback_and_record_alone_change_nothing_until_a_param_is_touched() {
        let state = playing_state();
        state.pattern.patterns[0].toggle_step(0);
        let before = state.pattern.step_data[0].get(0, StepParam::Velocity);
        let mut print = StepPrintState::default();
        let printed = print_pass(&state, &mut print, 0, true);
        assert!(printed.steps.is_empty());
        assert_eq!(state.pattern.step_data[0].get(0, StepParam::Velocity), before);
    }

    #[test]
    fn touch_prints_the_step_under_the_playhead_and_only_the_touched_param() {
        let state = playing_state();
        state.pattern.patterns[0].toggle_step(4);
        set_playhead(&state, 4);
        let duration_before = state.pattern.step_data[0].get(4, StepParam::Duration);
        let mut print = StepPrintState::default();
        print.latch(0, StepParam::Velocity, 0.25);
        let printed = print_pass(&state, &mut print, 0, true);
        assert_eq!(printed.steps, vec![4]);
        assert_eq!(state.pattern.step_data[0].get(4, StepParam::Velocity), 0.25);
        // Untouched params on the printed step are left alone.
        assert_eq!(
            state.pattern.step_data[0].get(4, StepParam::Duration),
            duration_before
        );
    }

    #[test]
    fn latch_keeps_printing_across_the_loop_wrap_and_skips_trigger_less_steps() {
        let state = playing_state();
        // Triggers on 14 and 1; 15 and 0 stay empty across the wrap.
        state.pattern.patterns[0].toggle_step(14);
        state.pattern.patterns[0].toggle_step(1);
        set_playhead(&state, 13);
        let mut print = StepPrintState::default();
        print.latch(0, StepParam::Duration, 3.0);
        let printed = print_pass(&state, &mut print, 0, true);
        assert!(printed.steps.is_empty(), "step 13 has no trigger");
        // Frame gap: playhead moved 13 -> 1 across the wrap; the latch (not
        // a fresh touch) prints every passed trigger step.
        set_playhead(&state, 1);
        let printed = print_pass(&state, &mut print, 0, true);
        assert_eq!(printed.steps, vec![14, 1]);
        assert_eq!(state.pattern.step_data[0].get(14, StepParam::Duration), 3.0);
        assert_eq!(state.pattern.step_data[0].get(1, StepParam::Duration), 3.0);
        // Trigger-less steps stay at their defaults.
        let default_duration = StepParam::Duration.default_value();
        assert_eq!(
            state.pattern.step_data[0].get(15, StepParam::Duration),
            default_duration
        );
        assert_eq!(
            state.pattern.step_data[0].get(0, StepParam::Duration),
            default_duration
        );
    }

    #[test]
    fn value_moves_within_one_step_reprint_it_but_a_still_latch_does_not_rewrite() {
        let state = playing_state();
        state.pattern.patterns[0].toggle_step(2);
        set_playhead(&state, 2);
        let mut print = StepPrintState::default();
        print.latch(0, StepParam::Velocity, 0.5);
        assert_eq!(print_pass(&state, &mut print, 0, true).steps, vec![2]);
        // Playhead still on 2, no new touch: nothing to write.
        assert!(print_pass(&state, &mut print, 0, true).steps.is_empty());
        // The picker moved: the same step takes the new value.
        print.latch(0, StepParam::Velocity, 0.75);
        assert_eq!(print_pass(&state, &mut print, 0, true).steps, vec![2]);
        assert_eq!(state.pattern.step_data[0].get(2, StepParam::Velocity), 0.75);
    }

    #[test]
    fn pause_record_off_and_focus_change_each_disarm_without_stray_writes() {
        for (playing, recording, focused_track) in [(false, true, 0), (true, false, 0), (true, true, 1)] {
            let state = playing_state();
            state.transport.playing.store(playing, Ordering::Relaxed);
            state.pattern.patterns[0].toggle_step(0);
            set_playhead(&state, 0);
            let before = state.pattern.step_data[0].get(0, StepParam::Velocity);
            let mut print = StepPrintState::default();
            print.latch(0, StepParam::Velocity, 0.1);
            let printed = print_pass(&state, &mut print, focused_track, recording);
            assert!(printed.steps.is_empty());
            assert!(!print.armed(), "gate ({playing}, {recording}, track {focused_track}) must disarm");
            assert_eq!(state.pattern.step_data[0].get(0, StepParam::Velocity), before);
            // Disarmed: the next pass with the gate restored stays silent
            // until the next touch (re-arm on touch, not on transport).
            state.transport.playing.store(true, Ordering::Relaxed);
            assert!(print_pass(&state, &mut print, 0, true).steps.is_empty());
        }
    }

    #[test]
    fn rolled_hits_take_the_latched_values_for_touched_params_only() {
        let mut print = StepPrintState::default();
        print.latch(0, StepParam::Duration, 2.5);
        print.latch(0, StepParam::Velocity, 0.6);
        let mut hit = RollHitRecorded {
            track: 0,
            step: 3,
            delay: 0.0,
            transpose: 7.0,
            velocity: 1.0,
            duration_steps: 0.25,
            beat: 1.5,
        };
        print.override_roll_hit(&mut hit);
        assert_eq!(hit.duration_steps, 2.5);
        assert_eq!(hit.velocity, 0.6);
        // Transpose untouched: the rolled key's pitch survives.
        assert_eq!(hit.transpose, 7.0);
        // Hits on other tracks are never overridden.
        let mut other = RollHitRecorded { track: 1, ..hit };
        let velocity_before = other.velocity;
        print.override_roll_hit(&mut other);
        assert_eq!(other.velocity, velocity_before);
    }

    #[test]
    fn touching_a_param_on_another_track_restarts_the_latch_there() {
        let mut print = StepPrintState::default();
        print.latch(0, StepParam::Velocity, 0.5);
        print.latch(1, StepParam::Duration, 2.0);
        let mut hit = RollHitRecorded {
            track: 1,
            step: 0,
            delay: 0.0,
            transpose: 0.0,
            velocity: 1.0,
            duration_steps: 0.25,
            beat: 0.0,
        };
        print.override_roll_hit(&mut hit);
        assert_eq!(hit.duration_steps, 2.0);
        // The old track's velocity latch did not carry over.
        assert_eq!(hit.velocity, 1.0);
    }
}
