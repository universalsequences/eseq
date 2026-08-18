/*!
Track rolling (docs/rolling-core-spec.md): scheduler-side held-note state and
roll-grid emission. Phase 1 — audible track rolls; recording is phase 3.
*/

#[allow(unused_imports)]
use super::*;

use crate::sequencer::RollCommand;

/// Scheduler-side roll state (spec 3). Deliberately NOT stored here: the roll
/// rate (F2 — re-read from the transport atomics every chunk, which is what
/// makes mid-hold rate switching free) and any start time (F7 — boundaries
/// live on the absolute transport beat grid).
pub(super) struct RollState {
    /// Held notes per track. Set semantics: duplicate presses of the same
    /// transpose collapse to one roll voice; empty = not rolling.
    pub(super) held: [Vec<f32>; MAX_TRACKS],
    /// Bumped on every NoteOff/ClearAll. Unused in phase 1; phase 4 stamps it
    /// onto enqueued roll events for exact audio-side cancel.
    pub(super) generation: u64,
}

impl RollState {
    pub(super) fn new() -> Self {
        Self {
            held: std::array::from_fn(|_| Vec::new()),
            generation: 0,
        }
    }

    pub(super) fn any_held(&self) -> bool {
        self.held.iter().any(|held| !held.is_empty())
    }

    pub(super) fn clear_all(&mut self) {
        if self.any_held() {
            for held in &mut self.held {
                held.clear();
            }
            self.generation += 1;
        }
    }

    pub(super) fn apply_commands(&mut self, commands: &[RollCommand]) {
        for command in commands {
            match *command {
                RollCommand::NoteOn { track, transpose } => {
                    if track < MAX_TRACKS
                        && !self.held[track].iter().any(|held| *held == transpose)
                    {
                        self.held[track].push(transpose);
                    }
                }
                RollCommand::NoteOff { track, transpose } => {
                    if track < MAX_TRACKS {
                        self.held[track].retain(|held| *held != transpose);
                        self.generation += 1;
                    }
                }
                // Rate switches need no scheduler-side state (F2): the next
                // chunk simply scans the grid it reads from the atomics.
                // Sequence rolling is phase 2; the command is accepted so its
                // ordering is already right when the window capture lands.
                RollCommand::SetRate { .. } | RollCommand::SequenceRoll { .. } => {}
                RollCommand::ClearAll => self.clear_all(),
            }
        }
    }
}

/// Emit roll hits for every roll-grid boundary inside
/// `[chunk_start_beats, chunk_end_beats)` (spec 4.2). Boundaries sit on the
/// absolute transport beat grid at `k * grid_beats` — the same
/// transport-locked convention as `quantized_launch::launch_deadline` — so
/// the first hit after a press waits for the next boundary (F1) and hits from
/// every track land phase-locked. Cancel is implicit: a NoteOff drained
/// before this pass removes every boundary not yet inside the lookahead
/// horizon (F3).
pub(super) fn schedule_roll_hits<const QUEUE_CAP: usize>(
    queue: &ScheduledEventQueue<QUEUE_CAP>,
    snapshot: &SequencerSnapshot,
    track_output_events: &mut Vec<TrackOutputEvent>,
    state: &SequencerState,
    clock: &SnapshotSequencerClock,
    roll: &RollState,
    grid_beats: f64,
    chunk_start_beats: f64,
    chunk_end_beats: f64,
    chunk_start_sample: u64,
    samples_per_quarter: f64,
    pattern_epoch: u64,
    global_transpose: f32,
) -> bool {
    const EPS: f64 = 1.0e-9;
    if grid_beats <= EPS || !roll.any_held() {
        return true;
    }
    let samples_per_step = (grid_beats * samples_per_quarter) as f32;
    let mut index = (((chunk_start_beats - EPS) / grid_beats).floor() as i64 + 1).max(0);
    loop {
        let boundary_beats = index as f64 * grid_beats;
        if boundary_beats >= chunk_end_beats - EPS {
            return true;
        }
        let sample_time = chunk_start_sample.saturating_add(
            ((boundary_beats - chunk_start_beats).max(0.0) * samples_per_quarter).round() as u64,
        );
        for (track, held) in roll.held.iter().enumerate() {
            if held.is_empty() || track >= snapshot.tracks.len() {
                continue;
            }
            // Track defaults (F4): default step params, one chord note per
            // held transpose (a chord if several keys are held). Step and
            // resolved transpose stay at the shared default so
            // `resolved_chord_transpose` passes the held notes through as-is.
            let resolved = ResolvedStep {
                duration: StepParam::Duration.default_value(),
                velocity: StepParam::Velocity.default_value(),
                speed: StepParam::Speed.default_value(),
                aux_a: StepParam::AuxA.default_value(),
                aux_b: StepParam::AuxB.default_value(),
                transpose: StepParam::Transpose.default_value(),
                pan: StepParam::Pan.default_value(),
                chop: StepParam::Chop.default_value(),
            };
            let chord =
                chord_data_from_parts(held, &[], &[], resolved.duration, resolved.transpose);
            let event = StepEvent {
                track,
                samples_per_step,
                resolved,
                chord,
                effect_params: resolve_effect_defaults(snapshot, track),
                instrument_params: resolve_instrument_defaults(snapshot, track),
                instrument_tensor_params: resolve_instrument_tensor_defaults(snapshot, track),
                sampler_params: resolve_sampler_defaults(snapshot, track),
                rack_macro_values: [None; crate::sequencer::RACK_MACRO_COUNT],
                // Step 0 stands in as the source step: roll hits are live
                // events, not pattern reads. The fingerprint is recomputed by
                // `enqueue_resolved_trigger`.
                source: EventSource::Step {
                    track,
                    step: 0,
                    instrument_fingerprint: 0,
                },
            };
            if !enqueue_step_event(
                queue,
                snapshot,
                track_output_events,
                pattern_epoch,
                sample_time,
                boundary_beats,
                samples_per_quarter as f32,
                global_transpose,
                event,
            ) {
                return false;
            }
            // Record-as-heard feedback (spec 6): stamp every emitted hit with
            // its track-local (step, delay) from the boundary geometry that
            // scheduled it. The control thread batches these per held key and
            // writes them back on note release; when recording is off it
            // simply discards the drain.
            let (step, delay, step_dur_beats) = clock.roll_record_position(
                track,
                boundary_beats,
                snapshot.tracks[track].params.num_steps,
            );
            let duration_steps = (grid_beats / step_dur_beats) as f32;
            for transpose in held {
                state.push_roll_recorded_hit(crate::sequencer::RollHitRecorded {
                    track,
                    step,
                    delay,
                    transpose: *transpose,
                    velocity: resolved.velocity,
                    duration_steps,
                    beat: boundary_beats,
                });
            }
        }
        index += 1;
    }
}
