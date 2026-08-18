/*!
Rolled-hit recording (docs/rolling-core-spec.md 6, phase 3).

The scheduler stamps every audible roll hit with its exact track-local
(step, delay) and absolute beat, and pushes it on a feedback channel
(`SequencerState::push_roll_recorded_hit`). This module is the control-thread
half: hits are batched per held key while the roll sounds, and written back
on note release (the v1 double-trigger rule — while the key is held the
audible roll stays authoritative; on release the written pattern takes over
on the same grid the roll was emitting on). Record-as-heard (F5): one pattern
hit per audible retrigger, no `record_quantize` pass — the roll grid already
quantized them, and mid-roll rate switches land as steps with differing
sub-step delays.
*/

use super::*;
use sequencer::sequencer::RollHitRecorded;

/// Batches drained until the scheduler's final feedback for a released key
/// has certainly arrived: the worker applies the NoteOff within ~1ms, so a
/// couple of UI frames is plenty for the (at most one) trailing hit.
const FLUSH_GRACE: Duration = Duration::from_millis(40);

struct PendingRollFlush {
    track: usize,
    transpose: f32,
    requested_at: Instant,
}

#[derive(Default)]
pub(crate) struct RollRecordBuffer {
    hits: Vec<RollHitRecorded>,
    flushes: Vec<PendingRollFlush>,
}

impl RollRecordBuffer {
    /// A rolled key was released while recording: schedule its batch for
    /// write-back once the grace window has passed.
    pub(crate) fn note_released(&mut self, track: usize, transpose: f32) {
        self.flushes.push(PendingRollFlush {
            track,
            transpose,
            requested_at: Instant::now(),
        });
    }

    fn clear(&mut self) {
        self.hits.clear();
        self.flushes.clear();
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

/// Per-frame drain + write-on-release flush, called from the reactive tick.
/// Returns (live pattern changed, pending take changed) so the caller can
/// republish the step grid / refresh the timeline preview.
pub(crate) fn tick_roll_record(app: &mut app::App, shared: &SharedHandles) -> (bool, bool) {
    let state = &shared.state;
    let drained = state.drain_roll_recorded_hits();
    let mut buffer = shared.roll_record.lock().unwrap();
    let roll_active = state.transport.roll_mode.load(Ordering::Relaxed) && state.is_playing();
    let recording = shared.recording.load(Ordering::Relaxed);
    if !roll_active || !recording {
        // Roll mode off / transport stop clears performance state (spec 7);
        // with recording off the feedback is pure telemetry — discard it.
        buffer.clear();
        return (false, false);
    }
    buffer.hits.extend(drained);
    // Unbounded-hold backstop; a flush drains its key's hits well before this.
    if buffer.hits.len() > 8192 {
        let excess = buffer.hits.len() - 8192;
        buffer.hits.drain(..excess);
    }

    let now = Instant::now();
    let mut due = Vec::new();
    buffer.flushes.retain(|flush| {
        if now.duration_since(flush.requested_at) >= FLUSH_GRACE {
            due.push((flush.track, flush.transpose));
            false
        } else {
            true
        }
    });
    if due.is_empty() {
        return (false, false);
    }

    let mut pattern_changed = false;
    let mut take_changed = false;
    // Same recording-kind fork as the live-key release path
    // (`handle_recording_key`): arrangement capture retargets into the
    // pending take; loop overdub claims the lane; song authority without
    // overdub drops rather than folding into the looping pattern.
    app.stamp_recording_kind_for_note();
    let song_authority = app.song_playback_authority_active();
    let overdub =
        app.recording_kind == Some(sequencer::app::song_transport::RecordingKind::Overdub);
    for (track, transpose) in due {
        let mut hits = Vec::new();
        buffer.hits.retain(|hit| {
            if hit.track == track && hit.transpose == transpose {
                hits.push(*hit);
                false
            } else {
                true
            }
        });
        for hit in hits {
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
            write_rolled_hit_to_pattern(state, &hit);
            pattern_changed = true;
        }
    }
    drop(buffer);
    if pattern_changed {
        state.publish_scheduler_snapshot();
        shared.ui_epoch.fetch_add(1, Ordering::Relaxed);
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
