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
    /// (track, transpose) pairs whose NoteOn arrived since the last emission
    /// pass — candidates for boundary catch-up (a press claims the grid line
    /// the lookahead frontier already passed, as long as the audio hasn't
    /// rendered it yet; see `schedule_roll_hits`). Consumed by the next pass;
    /// pruned on NoteOff/ClearAll so a tap released before the pass leaves
    /// nothing.
    pub(super) newly_pressed: Vec<(usize, f32)>,
}

impl RollState {
    pub(super) fn new() -> Self {
        Self {
            held: std::array::from_fn(|_| Vec::new()),
            generation: 0,
            newly_pressed: Vec::new(),
        }
    }

    pub(super) fn any_held(&self) -> bool {
        self.held.iter().any(|held| !held.is_empty())
    }

    pub(super) fn clear_all(&mut self) {
        self.newly_pressed.clear();
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
                        self.newly_pressed.push((track, transpose));
                    }
                }
                RollCommand::NoteOff { track, transpose } => {
                    if track < MAX_TRACKS {
                        self.held[track].retain(|held| *held != transpose);
                        self.newly_pressed
                            .retain(|(t, held)| *t != track || *held != transpose);
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
    roll: &mut RollState,
    grid_beats: f64,
    chunk_start_beats: f64,
    chunk_end_beats: f64,
    chunk_start_sample: u64,
    rendered: u64,
    samples_per_quarter: f64,
    pattern_epoch: u64,
    global_transpose: f32,
) -> bool {
    const EPS: f64 = 1.0e-9;
    if grid_beats <= EPS || !roll.any_held() {
        roll.newly_pressed.clear();
        return true;
    }

    // Boundary catch-up: for keys pressed since the last pass, the most
    // recent grid line at or before the scheduling frontier is emitted
    // retroactively — but ONLY while the audio render head has not reached it
    // yet. That line is still the next AUDIBLE boundary, so the retroactive
    // emission is sample-exact and never late. A line the head has already
    // rendered is simply missed: the press waits for the next boundary (F1) —
    // emitting it would fire immediately and land audibly behind the grid,
    // which reads as a one-off swung hit.
    let pressed = std::mem::take(&mut roll.newly_pressed);
    if !pressed.is_empty() {
        let line_beats = ((chunk_start_beats + EPS) / grid_beats).floor() * grid_beats;
        let rendered_beats = chunk_start_beats
            - chunk_start_sample.saturating_sub(rendered) as f64 / samples_per_quarter;
        // A line at (or an epsilon before) chunk_start is emitted by the
        // regular scan below — no catch-up needed, no double fire.
        if line_beats < chunk_start_beats - EPS && line_beats >= rendered_beats - EPS {
            let sample_time = chunk_start_sample.saturating_sub(
                ((chunk_start_beats - line_beats) * samples_per_quarter).round() as u64,
            );
            for track in 0..roll.held.len().min(snapshot.tracks.len()) {
                // Only the freshly pressed notes: any longer-held key on this
                // track already sounded this line.
                let notes: Vec<f32> = pressed
                    .iter()
                    .filter(|(t, transpose)| {
                        *t == track && roll.held[track].contains(transpose)
                    })
                    .map(|(_, transpose)| *transpose)
                    .collect();
                if notes.is_empty() {
                    continue;
                }
                if !emit_roll_hit(
                    queue,
                    snapshot,
                    track_output_events,
                    state,
                    clock,
                    track,
                    &notes,
                    grid_beats,
                    line_beats,
                    sample_time,
                    samples_per_quarter,
                    pattern_epoch,
                    global_transpose,
                ) {
                    return false;
                }
            }
        }
    }

    let mut index = (((chunk_start_beats - EPS) / grid_beats).floor() as i64 + 1).max(0);
    loop {
        let boundary_beats = index as f64 * grid_beats;
        if boundary_beats >= chunk_end_beats - EPS {
            return true;
        }
        let sample_time = chunk_start_sample.saturating_add(
            ((boundary_beats - chunk_start_beats).max(0.0) * samples_per_quarter).round() as u64,
        );
        for track in 0..roll.held.len().min(snapshot.tracks.len()) {
            if roll.held[track].is_empty() {
                continue;
            }
            let notes = roll.held[track].clone();
            if !emit_roll_hit(
                queue,
                snapshot,
                track_output_events,
                state,
                clock,
                track,
                &notes,
                grid_beats,
                boundary_beats,
                sample_time,
                samples_per_quarter,
                pattern_epoch,
                global_transpose,
            ) {
                return false;
            }
        }
        index += 1;
    }
}

/// One roll hit on one track: the enqueue (a normal ResolvedTrigger built
/// from track defaults, one chord note per transpose — F4) plus the
/// record-as-heard feedback (spec 6) stamping each note with the track-local
/// (step, delay) from the boundary geometry that scheduled it.
#[allow(clippy::too_many_arguments)]
fn emit_roll_hit<const QUEUE_CAP: usize>(
    queue: &ScheduledEventQueue<QUEUE_CAP>,
    snapshot: &SequencerSnapshot,
    track_output_events: &mut Vec<TrackOutputEvent>,
    state: &SequencerState,
    clock: &SnapshotSequencerClock,
    track: usize,
    notes: &[f32],
    grid_beats: f64,
    boundary_beats: f64,
    sample_time: u64,
    samples_per_quarter: f64,
    pattern_epoch: u64,
    global_transpose: f32,
) -> bool {
    // Track defaults (F4): default step params; step and resolved transpose
    // stay at the shared default so `resolved_chord_transpose` passes the
    // notes through as-is.
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
    let chord = chord_data_from_parts(notes, &[], &[], resolved.duration, resolved.transpose);
    let event = StepEvent {
        track,
        samples_per_step: (grid_beats * samples_per_quarter) as f32,
        resolved,
        chord,
        effect_params: resolve_effect_defaults(snapshot, track),
        instrument_params: resolve_instrument_defaults(snapshot, track),
        instrument_tensor_params: resolve_instrument_tensor_defaults(snapshot, track),
        sampler_params: resolve_sampler_defaults(snapshot, track),
        rack_macro_values: [None; crate::sequencer::RACK_MACRO_COUNT],
        // Step 0 stands in as the source step: roll hits are live events,
        // not pattern reads. The fingerprint is recomputed by
        // `enqueue_resolved_trigger`.
        source: EventSource::Step {
            track,
            step: 0,
            instrument_fingerprint: 0,
        },
    };
    // Play swung, record straight (eseq-767.10): the audible enqueue gets the
    // track's swing offset — re-read from the snapshot each hit, so turning
    // the knob mid-hold moves subsequent hits — while the record feedback
    // below stays keyed to the straight boundary geometry.
    let swung_sample_time = clock.roll_swung_sample_time(
        snapshot,
        track,
        boundary_beats,
        sample_time,
        samples_per_quarter,
    );
    if !enqueue_step_event(
        queue,
        snapshot,
        track_output_events,
        pattern_epoch,
        swung_sample_time,
        boundary_beats,
        samples_per_quarter as f32,
        global_transpose,
        event,
    ) {
        return false;
    }
    let (step, delay, step_dur_beats) = clock.roll_record_position(
        track,
        boundary_beats,
        snapshot.tracks[track].params.num_steps,
    );
    let duration_steps = (grid_beats / step_dur_beats) as f32;
    for transpose in notes {
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
    true
}
