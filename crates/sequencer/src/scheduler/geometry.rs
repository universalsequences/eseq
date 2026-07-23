/*!
Chord construction, trigger placement, and active note-span geometry.
*/

#[allow(unused_imports)]
use super::*;

pub(super) fn chord_data_from_parts(
    notes: &[f32],
    durations: &[f32],
    delays: &[f32],
    fallback_duration: f32,
    step_transpose: f32,
) -> ScheduledChordData {
    let mut chord = ScheduledChordData {
        count: notes.len().min(MAX_VOICES),
        notes: [0.0; MAX_VOICES],
        durations: [0.0; MAX_VOICES],
        delays: [0.0; MAX_VOICES],
        step_transpose,
    };
    for (idx, note) in notes.iter().take(MAX_VOICES).enumerate() {
        chord.notes[idx] = *note;
        chord.durations[idx] = durations
            .get(idx)
            .copied()
            .filter(|duration| *duration > 0.0)
            .unwrap_or(fallback_duration);
        chord.delays[idx] = delays
            .get(idx)
            .copied()
            .unwrap_or(0.0)
            .clamp(StepParam::Delay.min(), StepParam::Delay.max());
    }
    chord
}

pub(super) fn step_chord_data(
    snapshot: &SequencerSnapshot,
    track_idx: usize,
    step_idx: usize,
) -> ScheduledChordData {
    let step = &snapshot.tracks[track_idx].steps[step_idx];
    chord_data_from_parts(
        &step.chord,
        &step.chord_durations,
        &step.chord_delays,
        step.params[StepParam::Duration.index()],
        step.params[StepParam::Transpose.index()],
    )
}

pub(super) fn track_step_boundaries(track: &crate::sequencer::SequencerTrackSnapshot) -> Vec<f32> {
    const EPS: f64 = 1e-9;
    let ns = track.params.num_steps;
    let mut boundaries = vec![0.0_f32; ns + 1];
    let mut accum = 0.0_f64;
    for step in 0..ns {
        let tb = track.steps[step]
            .timebase_override
            .unwrap_or(track.params.timebase);
        let sync_b = sync_beats(track.steps[step].params[StepParam::Sync.index()]);
        if sync_b > EPS {
            accum = ceil_to_grid(accum, sync_b);
        }
        boundaries[step] = accum as f32;
        accum += tb.step_beats(ns);
    }
    boundaries[ns] = accum as f32;
    boundaries
}

pub(super) fn delayed_step_start_beats(
    track: &crate::sequencer::SequencerTrackSnapshot,
    step: usize,
    boundaries: &[f32],
) -> f32 {
    let step_beats = track.steps[step]
        .timebase_override
        .unwrap_or(track.params.timebase)
        .step_beats(track.params.num_steps) as f32;
    let delay = track.steps[step].params[StepParam::Delay.index()]
        .clamp(StepParam::Delay.min(), StepParam::Delay.max());
    boundaries[step] + delay * step_beats.max(0.0)
}

pub(super) fn explicit_note_delay_beats(
    step_snapshot: &crate::sequencer::SequencerStepSnapshot,
    note_idx: usize,
    step_beats: f32,
) -> f32 {
    step_snapshot
        .chord_delays
        .get(note_idx)
        .copied()
        .unwrap_or(0.0)
        .clamp(StepParam::Delay.min(), StepParam::Delay.max())
        * step_beats.max(0.0)
}

pub(super) fn step_trigger_start_beats(
    track: &crate::sequencer::SequencerTrackSnapshot,
    step: usize,
    boundaries: &[f32],
) -> f32 {
    if track.steps[step].chord.is_empty() {
        delayed_step_start_beats(track, step, boundaries)
    } else {
        boundaries[step]
    }
}

pub(super) fn track_note_spans_for_trigger(
    snapshot: &SequencerSnapshot,
    track_idx: usize,
    step_idx: usize,
) -> Vec<AccumulatorNoteSpan> {
    const EPS: f32 = 1e-5;
    let Some(track) = snapshot.tracks.get(track_idx) else {
        return Vec::new();
    };
    let ns = track.params.num_steps;
    if step_idx >= ns {
        return Vec::new();
    }
    let boundaries = track_step_boundaries(track);
    let trigger_start = step_trigger_start_beats(track, step_idx, &boundaries);
    let mut candidates = Vec::new();

    for step in 0..ns {
        let step_snapshot = &track.steps[step];
        if !step_snapshot.active {
            continue;
        }
        let step_beats = step_snapshot
            .timebase_override
            .unwrap_or(track.params.timebase)
            .step_beats(ns) as f32;
        if step_beats <= 0.0 {
            continue;
        }
        let fallback_duration = step_snapshot.params[StepParam::Duration.index()].max(0.0);
        if step_snapshot.chord.is_empty() {
            let step_start = delayed_step_start_beats(track, step, &boundaries);
            candidates.push(AccumulatorNoteSpan {
                transpose: step_snapshot.params[StepParam::Transpose.index()],
                start_beats: step_start,
                end_beats: step_start + fallback_duration * step_beats,
            });
        } else {
            for (idx, note) in step_snapshot.chord.iter().enumerate() {
                let step_start =
                    boundaries[step] + explicit_note_delay_beats(step_snapshot, idx, step_beats);
                let duration = step_snapshot
                    .chord_durations
                    .get(idx)
                    .copied()
                    .filter(|duration| *duration > 0.0)
                    .unwrap_or(fallback_duration)
                    .max(0.0);
                candidates.push(AccumulatorNoteSpan {
                    transpose: *note,
                    start_beats: step_start,
                    end_beats: step_start + duration * step_beats,
                });
            }
        }
    }

    candidates.sort_by(|a, b| {
        a.start_beats
            .partial_cmp(&b.start_beats)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    let Some(group_anchor) = candidates
        .iter()
        .filter(|note| note.start_beats >= trigger_start - EPS)
        .map(|note| note.start_beats)
        .next()
    else {
        return Vec::new();
    };

    if candidates
        .iter()
        .any(|note| note.start_beats < group_anchor - EPS && note.end_beats > group_anchor + EPS)
    {
        return Vec::new();
    }

    let mut group_end = candidates
        .iter()
        .filter(|note| (note.start_beats - group_anchor).abs() <= EPS)
        .map(|note| note.end_beats)
        .fold(group_anchor, f32::max);
    if group_end <= group_anchor + EPS {
        return Vec::new();
    }

    let mut out = Vec::new();
    for note in candidates {
        if note.start_beats < trigger_start - EPS {
            continue;
        }
        if note.start_beats > group_anchor + EPS && note.start_beats >= group_end - EPS {
            break;
        }
        if note.end_beats <= group_anchor + EPS {
            continue;
        }
        group_end = group_end.max(note.end_beats);
        out.push(AccumulatorNoteSpan {
            transpose: note.transpose,
            start_beats: (note.start_beats - trigger_start).max(0.0),
            end_beats: (note.end_beats - trigger_start).max(0.0),
        });
    }
    out
}

pub(super) fn track_active_note_spans_at_beat(
    snapshot: &SequencerSnapshot,
    track_idx: usize,
    position_beats: f32,
    window_beats: f32,
) -> Vec<AccumulatorNoteSpan> {
    const EPS: f32 = 1e-5;
    let Some(track) = snapshot.tracks.get(track_idx) else {
        return Vec::new();
    };
    if window_beats <= 0.0 {
        return Vec::new();
    }
    let ns = track.params.num_steps;
    let boundaries = track_step_boundaries(track);
    let cycle_beats = boundaries.get(ns).copied().unwrap_or(0.0).max(EPS);
    let position = position_beats.rem_euclid(cycle_beats);
    let window_end = position + window_beats;
    let mut spans = Vec::new();

    for cycle_offset in [0.0, cycle_beats] {
        for step in 0..ns {
            let step_snapshot = &track.steps[step];
            if !step_snapshot.active {
                continue;
            }
            let step_beats = step_snapshot
                .timebase_override
                .unwrap_or(track.params.timebase)
                .step_beats(ns) as f32;
            if step_beats <= 0.0 {
                continue;
            }
            let fallback_duration = step_snapshot.params[StepParam::Duration.index()].max(0.0);
            if step_snapshot.chord.is_empty() {
                let step_start = delayed_step_start_beats(track, step, &boundaries) + cycle_offset;
                let note_end = step_start + fallback_duration * step_beats;
                if note_end > position + EPS && step_start < window_end - EPS {
                    spans.push(AccumulatorNoteSpan {
                        transpose: step_snapshot.params[StepParam::Transpose.index()],
                        start_beats: (step_start - position).max(0.0),
                        end_beats: (note_end - position).min(window_beats).max(0.0),
                    });
                }
            } else {
                for (idx, note) in step_snapshot.chord.iter().enumerate() {
                    let step_start = boundaries[step]
                        + explicit_note_delay_beats(step_snapshot, idx, step_beats)
                        + cycle_offset;
                    let duration = step_snapshot
                        .chord_durations
                        .get(idx)
                        .copied()
                        .filter(|duration| *duration > 0.0)
                        .unwrap_or(fallback_duration)
                        .max(0.0);
                    let note_end = step_start + duration * step_beats;
                    if note_end > position + EPS && step_start < window_end - EPS {
                        spans.push(AccumulatorNoteSpan {
                            transpose: *note,
                            start_beats: (step_start - position).max(0.0),
                            end_beats: (note_end - position).min(window_beats).max(0.0),
                        });
                    }
                }
            }
        }
    }

    spans
        .into_iter()
        .filter(|span| span.end_beats > span.start_beats + EPS)
        .collect()
}

