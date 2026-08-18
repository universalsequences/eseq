/*!
Rolled-hit recording (docs/rolling-core-spec.md 6, phase 3).

The scheduler stamps every audible roll hit with its exact track-local
(step, delay) and absolute beat, and pushes it on a feedback channel
(`SequencerState::push_roll_recorded_hit`). This module is the control-thread
half: hits are written into the live pattern state the moment they arrive —
the step grid reads live state, so triggers appear as they roll in — but the
SCHEDULER SNAPSHOT is only republished on note release (plus a grace window).
That deferral is what implements the spec's double-trigger rule: the audio
engine schedules from the snapshot, so the written steps stay inaudible and
the roll remains the sole voice while the key is held; on release the
publish hands playback over to a pattern sitting on the exact grid the roll
was emitting on. Record-as-heard (F5): one pattern hit per audible
retrigger, no `record_quantize` pass — the roll grid already quantized them,
and mid-roll rate switches land as steps with differing sub-step delays.
*/

use super::*;
use sequencer::sequencer::RollHitRecorded;

/// The publish waits until the scheduler's final feedback for a released key
/// has certainly arrived and been written: the worker applies the NoteOff
/// within ~1ms, so a couple of UI frames is plenty for the (at most one)
/// trailing hit.
const PUBLISH_GRACE: Duration = Duration::from_millis(40);

#[derive(Default)]
pub(crate) struct RollRecordBuffer {
    /// Pattern writes have landed in live state but not yet in the scheduler
    /// snapshot — the publish is owed on the next due release (or on the
    /// roll/record/transport turning off).
    dirty_unpublished: bool,
    /// Release instants awaiting their publish grace window.
    pending_publishes: Vec<Instant>,
}

impl RollRecordBuffer {
    /// A rolled key was released while recording: republish the scheduler
    /// snapshot once the grace window has passed, handing playback of the
    /// written steps over to the pattern.
    pub(crate) fn note_released(&mut self) {
        self.pending_publishes.push(Instant::now());
    }

    /// Pattern writes awaiting their release publish. While true, other
    /// writers (live step-param printing) must defer their own snapshot
    /// publish or the held roll's written steps become audible early.
    pub(crate) fn has_unpublished_writes(&self) -> bool {
        self.dirty_unpublished
    }
}

/// One rolled hit into the live pattern — the same write-back as the live-key
/// release path (`handle_recording_key`), minus the quantize pass (F5).
pub(crate) fn write_rolled_hit_to_pattern(state: &SequencerState, hit: &RollHitRecorded) {
    if !state.pattern.patterns[hit.track].is_active(hit.step) {
        state.pattern.patterns[hit.track].toggle_step(hit.step);
    }
    state.pattern.chord_data[hit.track].add_note_with_timing(
        hit.step,
        hit.transpose,
        hit.duration_steps,
        hit.delay,
    );
    let first_note = state.pattern.chord_data[hit.track].get(hit.step, 0);
    state.pattern.step_data[hit.track].set(hit.step, StepParam::Transpose, first_note);
    state.pattern.step_data[hit.track].set(hit.step, StepParam::Velocity, hit.velocity);
    state
        .pattern
        .step_data[hit.track]
        .set(hit.step, StepParam::Duration, hit.duration_steps);
}

/// Per-frame drain: write arriving hits into live pattern state immediately
/// (the step grid shows them as they roll in) and republish the scheduler
/// snapshot once a release's grace window passes. Returns (live pattern
/// changed, pending take changed) so the caller can republish the step grid /
/// refresh the timeline preview.
pub(crate) fn tick_roll_record(app: &mut app::App, shared: &SharedHandles) -> (bool, bool) {
    let state = &shared.state;
    let mut drained = state.drain_roll_recorded_hits();
    // Rolled hits recorded while step-param printing is armed take the
    // latched values at record time (bead eseq-jc9): the roll records the
    // trigger, the live print gives it velocity/duration/pitch.
    if !drained.is_empty() {
        let print = shared.step_print.lock().unwrap();
        for hit in &mut drained {
            print.override_roll_hit(hit);
        }
    }
    let mut buffer = shared.roll_record.lock().unwrap();
    let roll_active = state.transport.roll_mode.load(Ordering::Relaxed) && state.is_playing();
    let recording = shared.recording.load(Ordering::Relaxed);
    if !roll_active || !recording {
        // Roll mode off / transport stop / recording off ends the take
        // (spec 7): drop undrained feedback, but never strand written steps —
        // anything already visible in the grid gets published now.
        buffer.pending_publishes.clear();
        let owed_publish = std::mem::take(&mut buffer.dirty_unpublished);
        drop(buffer);
        if owed_publish {
            state.publish_scheduler_snapshot();
            shared.ui_epoch.fetch_add(1, Ordering::Relaxed);
        }
        return (owed_publish, false);
    }

    let mut pattern_changed = false;
    let mut take_changed = false;
    if !drained.is_empty() {
        // Same recording-kind fork as the live-key release path
        // (`handle_recording_key`): arrangement capture retargets into the
        // pending take; loop overdub claims the lane; song authority without
        // overdub drops rather than folding into the looping pattern.
        app.stamp_recording_kind_for_note();
        let song_authority = app.song_playback_authority_active();
        let overdub =
            app.recording_kind == Some(sequencer::app::song_transport::RecordingKind::Overdub);
        for hit in &drained {
            if !overdub
                && app.take_record_note_at_beats(
                    hit.track,
                    hit.beat,
                    hit.transpose,
                    hit.duration_steps,
                    sequencer::record_quantize::RecordQuantize::Off,
                )
            {
                take_changed = true;
                continue;
            }
            if song_authority {
                if !overdub {
                    continue;
                }
                if !app.claim_overdub_lane(hit.track) {
                    continue;
                }
            }
            write_rolled_hit_to_pattern(state, hit);
            pattern_changed = true;
            buffer.dirty_unpublished = true;
        }
    }

    // Publish on release (+grace): the audio engine schedules from the
    // snapshot, so this is the moment the written steps become audible —
    // after the roll for that key has stopped emitting.
    let now = Instant::now();
    let before = buffer.pending_publishes.len();
    buffer
        .pending_publishes
        .retain(|requested_at| now.duration_since(*requested_at) < PUBLISH_GRACE);
    let publish_due = buffer.pending_publishes.len() < before;
    let owed_publish = publish_due && std::mem::take(&mut buffer.dirty_unpublished);
    drop(buffer);
    if owed_publish {
        state.publish_scheduler_snapshot();
        shared.ui_epoch.fetch_add(1, Ordering::Relaxed);
        pattern_changed = true;
    }
    (pattern_changed, take_changed)
}

#[cfg(test)]
mod roll_record_tests {
    use super::write_rolled_hit_to_pattern;
    use sequencer::sequencer::{
        default_empty_effect_chain, RollHitRecorded, SequencerState, StepParam,
    };

    #[test]
    fn write_rolled_hit_activates_step_and_stacks_sub_step_delays() {
        let state = SequencerState::new(1, vec![default_empty_effect_chain()]);
        let hit = |step: usize, delay: f32, beat: f64| RollHitRecorded {
            track: 0,
            step,
            delay,
            transpose: 3.0,
            velocity: 1.0,
            duration_steps: 0.5,
            beat,
        };
        // A 1/32 roll on a 1/16 track: two hits per step, the second as a
        // half-step delay — record-as-heard, no re-quantize (F5).
        for hit in [hit(2, 0.0, 0.5), hit(2, 0.5, 0.625), hit(3, 0.0, 0.75)] {
            write_rolled_hit_to_pattern(&state, &hit);
        }
        assert!(state.pattern.patterns[0].is_active(2));
        assert!(state.pattern.patterns[0].is_active(3));
        assert!(!state.pattern.patterns[0].is_active(1));
        let chord = &state.pattern.chord_data[0];
        assert_eq!(chord.count(2), 2);
        assert_eq!(chord.get(2, 0), 3.0);
        assert_eq!(chord.get(2, 1), 3.0);
        assert_eq!(chord.get_delay(2, 0), 0.0);
        assert_eq!(chord.get_delay(2, 1), 0.5);
        assert_eq!(chord.count(3), 1);
        let steps = &state.pattern.step_data[0];
        assert_eq!(steps.get(2, StepParam::Transpose), 3.0);
        assert_eq!(steps.get(2, StepParam::Velocity), 1.0);
        assert_eq!(steps.get(2, StepParam::Duration), 0.5);
    }
}
