use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

use eseqlisp::vm::Value;
use eseqlisp::Runtime;

use sequencer::app::focus::EditFocus;
use sequencer::sequencer::{
    PatternId, SequencerState, StepParam, TakeId, TrackPatternData, MAX_STEPS,
};

use super::values::{list_value, map_value};

const PIANO_ROLL_ID_STRIDE: usize = 16;
const PIANO_ROLL_MIN_TRANSPOSE: i32 = -48;
const PIANO_ROLL_MAX_TRANSPOSE: i32 = 48;
const PIANO_ROLL_MIN_DURATION: f32 = 0.03125;

#[derive(Clone)]
struct PianoRollNote {
    transpose: f32,
    duration: f32,
    delay: f32,
}

/// Which note storage the piano roll is pointed at (clip-edit-target spec 3),
/// the UI projection of `sequencer::app::focus::EditFocus`. Shared with the
/// `seq-piano-roll-action` native through a cell the reactive tick refreshes;
/// the host-command layer re-resolves from `App` authoritatively.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PianoRollFocusSpec {
    /// The live mirror lanes (follow mode / effective pattern) — today's path.
    Live,
    /// A pinned pool pattern that is not currently effective.
    Pool(PatternId),
    /// A pinned take: the continuous take-step axis over its chunk patterns.
    Take(TakeId),
}

impl PianoRollFocusSpec {
    pub(crate) fn from_focus(focus: EditFocus) -> Self {
        match focus {
            EditFocus::Live { .. } => PianoRollFocusSpec::Live,
            EditFocus::Pattern { pattern, .. } => PianoRollFocusSpec::Pool(pattern),
            EditFocus::Take { take, .. } => PianoRollFocusSpec::Take(take),
        }
    }
}

pub(crate) type SharedPianoRollFocus = Arc<Mutex<PianoRollFocusSpec>>;

pub(crate) fn new_shared_piano_roll_focus() -> SharedPianoRollFocus {
    Arc::new(Mutex::new(PianoRollFocusSpec::Live))
}

/// The ONE reader/writer for piano-roll note entries (spec 3.4, locked
/// decision 6): every piano-roll code path goes through this handle, which
/// addresses the live lanes, a pool pattern, or a take's chunk patterns
/// according to the resolved focus. There is deliberately no second copy of
/// the note-entry logic for pool storage — `data_*` below are the same
/// semantics over `TrackPatternData`.
pub(crate) struct PianoRollLanes {
    state: Arc<SequencerState>,
    track: usize,
    focus: PianoRollFocusSpec,
    /// `(chunks, total_len_steps)` for a take focus, resolved once at
    /// construction: `state.track_take` locks `pattern.scenes` (which song
    /// playback also needs) and clones the whole `TrackTake`, so reading it
    /// per step would cost a lock + clone per note lookup.
    take_shape: Option<(Vec<PatternId>, usize)>,
}

impl PianoRollLanes {
    pub(crate) fn new(
        state: &Arc<SequencerState>,
        track: usize,
        focus: PianoRollFocusSpec,
    ) -> Self {
        let take_shape = match focus {
            PianoRollFocusSpec::Take(take) => state
                .track_take(track, take)
                .map(|take| (take.chunks.clone(), take.total_len_steps as usize)),
            _ => None,
        };
        Self {
            state: state.clone(),
            track,
            focus,
            take_shape,
        }
    }

    pub(crate) fn live(state: &Arc<SequencerState>, track: usize) -> Self {
        Self::new(state, track, PianoRollFocusSpec::Live)
    }

    pub(crate) fn track(&self) -> usize {
        self.track
    }

    pub(crate) fn focus(&self) -> PianoRollFocusSpec {
        self.focus
    }

    /// `(chunks, total_len_steps)` for a take focus, from the cache.
    fn take_shape(&self) -> Option<(&[PatternId], usize)> {
        self.take_shape
            .as_ref()
            .map(|(chunks, total)| (chunks.as_slice(), *total))
    }

    /// The focus axis length in steps (pattern `num_steps`, take playable
    /// length), clamped to at least 1.
    pub(crate) fn num_steps(&self) -> usize {
        let steps = match self.focus {
            PianoRollFocusSpec::Live => self.state.pattern.track_params[self.track]
                .get_num_steps()
                .min(MAX_STEPS),
            PianoRollFocusSpec::Pool(pattern) => self
                .state
                .with_pool_pattern(self.track, pattern, |data| data.track_params.num_steps)
                .unwrap_or(16)
                .min(MAX_STEPS),
            PianoRollFocusSpec::Take(_) => self.take_shape().map(|(_, total)| total).unwrap_or(16),
        };
        steps.max(1)
    }

    /// Upper bound of addressable steps: `MAX_STEPS` for pattern targets, the
    /// chunk span for takes (item ids are `step * 16 + voice`, so the take
    /// axis simply extends the id space past `MAX_STEPS`).
    fn step_capacity(&self) -> usize {
        match self.focus {
            PianoRollFocusSpec::Live | PianoRollFocusSpec::Pool(_) => MAX_STEPS,
            PianoRollFocusSpec::Take(_) => self
                .take_shape()
                .map(|(chunks, _)| chunks.len() * MAX_STEPS)
                .unwrap_or(MAX_STEPS),
        }
    }

    fn item_parts(&self, id: u64) -> Option<(usize, usize)> {
        let id = id as usize;
        let step = id / PIANO_ROLL_ID_STRIDE;
        let voice_idx = id % PIANO_ROLL_ID_STRIDE;
        (step < self.step_capacity()).then_some((step, voice_idx))
    }

    /// Map a take-axis step onto its owning chunk pattern and local step;
    /// identity for pattern targets. `None` past the take's playable end (the
    /// silent tail must not accept notes, takes spec 6.1).
    fn resolve_step(&self, step: usize) -> Option<(PianoRollStepAddress, usize)> {
        match self.focus {
            PianoRollFocusSpec::Live => {
                (step < MAX_STEPS).then_some((PianoRollStepAddress::Live, step))
            }
            PianoRollFocusSpec::Pool(pattern) => {
                (step < MAX_STEPS).then_some((PianoRollStepAddress::Pool(pattern), step))
            }
            PianoRollFocusSpec::Take(_) => {
                let (chunks, total_len) = self.take_shape()?;
                if step >= total_len {
                    return None;
                }
                let chunk = chunks.get(step / MAX_STEPS).copied()?;
                Some((PianoRollStepAddress::Pool(chunk), step % MAX_STEPS))
            }
        }
    }

    fn note_entries(&self, step: usize) -> Vec<PianoRollNote> {
        let Some((address, local)) = self.resolve_step(step) else {
            return Vec::new();
        };
        match address {
            PianoRollStepAddress::Live => live_note_entries(&self.state, self.track, local),
            PianoRollStepAddress::Pool(pattern) => self
                .state
                .with_pool_pattern(self.track, pattern, |data| data_note_entries(data, local))
                .unwrap_or_default(),
        }
    }

    /// Notes for `0..num_steps` in one pass. Unlike per-step `note_entries`,
    /// this opens `pattern.scenes` once per distinct pool pattern (once total
    /// for a pool focus, once per chunk for a take) instead of once per step —
    /// the items build walks the whole axis on every sync.
    fn note_entries_batch(&self, num_steps: usize) -> Vec<Vec<PianoRollNote>> {
        let mut out = vec![Vec::new(); num_steps];
        match self.focus {
            PianoRollFocusSpec::Live => {
                for step in 0..num_steps.min(MAX_STEPS) {
                    out[step] = live_note_entries(&self.state, self.track, step);
                }
            }
            PianoRollFocusSpec::Pool(pattern) => {
                self.state.with_pool_pattern(self.track, pattern, |data| {
                    for step in 0..num_steps.min(MAX_STEPS) {
                        out[step] = data_note_entries(data, step);
                    }
                });
            }
            PianoRollFocusSpec::Take(_) => {
                let Some((chunks, total_len)) = self.take_shape() else {
                    return out;
                };
                let end = num_steps.min(total_len);
                for (chunk_idx, pattern) in chunks.iter().enumerate() {
                    let base = chunk_idx * MAX_STEPS;
                    if base >= end {
                        break;
                    }
                    let chunk_end = (base + MAX_STEPS).min(end);
                    self.state.with_pool_pattern(self.track, *pattern, |data| {
                        for step in base..chunk_end {
                            out[step] = data_note_entries(data, step - base);
                        }
                    });
                }
            }
        }
        out
    }

    fn set_note_entries(&self, step: usize, notes: &[PianoRollNote]) {
        let Some((address, local)) = self.resolve_step(step) else {
            return;
        };
        let notes = normalized_piano_roll_notes(notes);
        match address {
            PianoRollStepAddress::Live => {
                live_set_note_entries(&self.state, self.track, local, &notes)
            }
            PianoRollStepAddress::Pool(pattern) => {
                self.state
                    .with_pool_pattern_mut(self.track, pattern, |data| {
                        data_set_note_entries(data, local, &notes)
                    });
            }
        }
    }

    fn step_delay(&self, step: usize) -> f32 {
        let Some((address, local)) = self.resolve_step(step) else {
            return 0.0;
        };
        piano_roll_sanitize_delay(match address {
            PianoRollStepAddress::Live => {
                self.state.pattern.step_data[self.track].get(local, StepParam::Delay)
            }
            PianoRollStepAddress::Pool(pattern) => self
                .state
                .with_pool_pattern(self.track, pattern, |data| {
                    data.step_data
                        .get(local)
                        .map(|params| params[StepParam::Delay.index()])
                        .unwrap_or(0.0)
                })
                .unwrap_or(0.0),
        })
    }
}

#[derive(Clone, Copy)]
enum PianoRollStepAddress {
    Live,
    Pool(PatternId),
}

fn live_note_entries(state: &Arc<SequencerState>, track: usize, step: usize) -> Vec<PianoRollNote> {
    let step_duration = state.pattern.step_data[track]
        .get(step, StepParam::Duration)
        .max(PIANO_ROLL_MIN_DURATION);
    let step_delay =
        piano_roll_sanitize_delay(state.pattern.step_data[track].get(step, StepParam::Delay));
    let chord_count = state.pattern.chord_data[track].count(step);
    if chord_count == 0 {
        if state.pattern.patterns[track].is_active(step) {
            vec![PianoRollNote {
                transpose: state.pattern.step_data[track].get(step, StepParam::Transpose),
                duration: step_duration,
                delay: step_delay,
            }]
        } else {
            Vec::new()
        }
    } else {
        (0..chord_count)
            .map(|idx| {
                let duration = state.pattern.chord_data[track].get_duration(step, idx);
                PianoRollNote {
                    transpose: state.pattern.chord_data[track].get(step, idx),
                    duration: if duration > 0.0 {
                        duration
                    } else {
                        step_duration
                    },
                    delay: state.pattern.chord_data[track].get_delay(step, idx),
                }
            })
            .collect()
    }
}

fn data_note_entries(data: &TrackPatternData, step: usize) -> Vec<PianoRollNote> {
    let Some(params) = data.step_data.get(step) else {
        return Vec::new();
    };
    let step_duration = params[StepParam::Duration.index()].max(PIANO_ROLL_MIN_DURATION);
    let step_delay = piano_roll_sanitize_delay(params[StepParam::Delay.index()]);
    let chord = &data.chord_snapshot;
    let notes = chord.steps.get(step).cloned().unwrap_or_default();
    if notes.is_empty() {
        let active = data.track_bits[step / 64] >> (step % 64) & 1 == 1;
        if active {
            vec![PianoRollNote {
                transpose: params[StepParam::Transpose.index()],
                duration: step_duration,
                delay: step_delay,
            }]
        } else {
            Vec::new()
        }
    } else {
        notes
            .iter()
            .enumerate()
            .map(|(idx, transpose)| {
                let duration = chord
                    .durations
                    .get(step)
                    .and_then(|lane| lane.get(idx))
                    .copied()
                    .unwrap_or(0.0);
                PianoRollNote {
                    transpose: *transpose,
                    duration: if duration > 0.0 {
                        duration
                    } else {
                        step_duration
                    },
                    delay: chord
                        .delays
                        .get(step)
                        .and_then(|lane| lane.get(idx))
                        .copied()
                        .unwrap_or(0.0),
                }
            })
            .collect()
    }
}

/// Round/clamp/sort/dedup — the shared normalization both writers apply.
fn normalized_piano_roll_notes(notes: &[PianoRollNote]) -> Vec<PianoRollNote> {
    let mut notes = notes
        .iter()
        .map(|note| PianoRollNote {
            transpose: note
                .transpose
                .round()
                .clamp(StepParam::Transpose.min(), StepParam::Transpose.max()),
            duration: piano_roll_sanitize_duration(note.duration),
            delay: piano_roll_sanitize_delay(note.delay),
        })
        .collect::<Vec<_>>();
    notes.sort_by(|a, b| {
        (a.delay, a.transpose)
            .partial_cmp(&(b.delay, b.transpose))
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    notes.dedup_by(|a, b| {
        (a.transpose - b.transpose).abs() < f32::EPSILON && (a.delay - b.delay).abs() < f32::EPSILON
    });
    // Both writers share the LIVE chord lane's capacity: `ChordData` refuses
    // notes past MAX_VOICES, and a pool pattern holding more than that would
    // truncate (and mis-index) when it becomes effective.
    notes.truncate(sequencer::audio::MAX_VOICES.min(PIANO_ROLL_ID_STRIDE));
    notes
}

fn live_set_note_entries(
    state: &Arc<SequencerState>,
    track: usize,
    step: usize,
    notes: &[PianoRollNote],
) {
    state.pattern.chord_data[track].clear_step(step);
    match notes {
        [] => state.pattern.patterns[track].set_step_active(step, false),
        notes => {
            let max_duration = notes
                .iter()
                .map(|note| note.duration)
                .fold(PIANO_ROLL_MIN_DURATION, f32::max);
            for note in notes {
                state.pattern.chord_data[track].add_note_with_timing(
                    step,
                    note.transpose,
                    note.duration,
                    note.delay,
                );
            }
            state.pattern.step_data[track].set(step, StepParam::Transpose, notes[0].transpose);
            state.pattern.step_data[track].set(step, StepParam::Duration, max_duration);
            state.pattern.step_data[track].set(step, StepParam::Delay, 0.0);
            state.pattern.patterns[track].set_step_active(step, true);
        }
    }
}

/// The pool twin of `live_set_note_entries` — same layout rules over the
/// stored `TrackPatternData` lanes (spec 3.4: one writer semantics, two
/// storage backends).
fn data_set_note_entries(data: &mut TrackPatternData, step: usize, notes: &[PianoRollNote]) {
    let word = step / 64;
    let mask = 1u64 << (step % 64);
    for lane in [
        &mut data.chord_snapshot.steps,
        &mut data.chord_snapshot.durations,
        &mut data.chord_snapshot.delays,
    ] {
        if let Some(cell) = lane.get_mut(step) {
            cell.clear();
        }
    }
    let Some(params) = data.step_data.get_mut(step) else {
        return;
    };
    match notes {
        [] => data.track_bits[word] &= !mask,
        notes => {
            let max_duration = notes
                .iter()
                .map(|note| note.duration)
                .fold(PIANO_ROLL_MIN_DURATION, f32::max);
            for note in notes {
                if let Some(cell) = data.chord_snapshot.steps.get_mut(step) {
                    cell.push(note.transpose);
                }
                if let Some(cell) = data.chord_snapshot.durations.get_mut(step) {
                    cell.push(note.duration);
                }
                if let Some(cell) = data.chord_snapshot.delays.get_mut(step) {
                    cell.push(note.delay);
                }
            }
            params[StepParam::Transpose.index()] = notes[0].transpose;
            params[StepParam::Duration.index()] = max_duration;
            params[StepParam::Delay.index()] = 0.0;
            data.track_bits[word] |= mask;
        }
    }
}

fn piano_roll_sanitize_duration(duration: f32) -> f32 {
    duration.max(PIANO_ROLL_MIN_DURATION)
}

fn piano_roll_sanitize_delay(delay: f32) -> f32 {
    delay.clamp(StepParam::Delay.min(), StepParam::Delay.max())
}

fn piano_roll_time_to_step_delay(time: f64, num_steps: usize) -> (usize, f32) {
    let num_steps = num_steps.max(1);
    let clamped = time.clamp(0.0, num_steps as f64);
    if clamped >= num_steps as f64 {
        return (num_steps - 1, StepParam::Delay.max());
    }
    let step = clamped.floor() as usize;
    let delay = (clamped - step as f64) as f32;
    (step.min(num_steps - 1), piano_roll_sanitize_delay(delay))
}

#[derive(Clone)]
struct PianoRollMoveItem {
    id: u64,
    step: usize,
    transpose: f32,
    duration: f32,
    delay: f32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PianoRollDragKind {
    Move,
    Resize,
}

pub(crate) struct PianoRollMoveState {
    kind: PianoRollDragKind,
    ids: Vec<u64>,
    anchor_id: u64,
    anchor_start: f32,
    anchor_lane: isize,
    originals: Vec<PianoRollMoveItem>,
    last_positions: Vec<PianoRollMoveItem>,
}

#[derive(Clone)]
pub(crate) struct PianoRollClipboardNote {
    start_offset: f32,
    transpose: f32,
    duration: f32,
}

pub(crate) type PianoRollClipboard = Arc<Mutex<Option<Vec<PianoRollClipboardNote>>>>;

pub(crate) fn new_piano_roll_clipboard() -> PianoRollClipboard {
    Arc::new(Mutex::new(None))
}

fn piano_roll_lane_to_transpose(lane: usize) -> f32 {
    (PIANO_ROLL_MAX_TRANSPOSE - lane as i32)
        .clamp(PIANO_ROLL_MIN_TRANSPOSE, PIANO_ROLL_MAX_TRANSPOSE) as f32
}

fn piano_roll_transpose_to_lane(transpose: f32) -> usize {
    (PIANO_ROLL_MAX_TRANSPOSE - transpose.round() as i32)
        .clamp(0, PIANO_ROLL_MAX_TRANSPOSE - PIANO_ROLL_MIN_TRANSPOSE) as usize
}

fn piano_roll_transpose_label(transpose: f32) -> String {
    let rounded = transpose.round() as i32;
    let pitch = rounded + 60;
    let name = match pitch.rem_euclid(12) {
        0 => "C",
        1 => "C#",
        2 => "D",
        3 => "D#",
        4 => "E",
        5 => "F",
        6 => "F#",
        7 => "G",
        8 => "G#",
        9 => "A",
        10 => "A#",
        _ => "B",
    };
    format!("{name}{}", 4 + rounded.div_euclid(12))
}

fn piano_roll_note_label(note: &PianoRollNote) -> String {
    let pitch = piano_roll_transpose_label(note.transpose);
    if note.delay.abs() < 0.001 {
        pitch
    } else {
        format!("{pitch} +{:.2}", note.delay)
    }
}

pub(crate) fn piano_roll_item_id(step: usize, voice_idx: usize) -> u64 {
    (step * PIANO_ROLL_ID_STRIDE + voice_idx.min(PIANO_ROLL_ID_STRIDE - 1)) as u64
}

pub(crate) fn build_piano_roll_lanes_value() -> Value {
    list_value(
        (PIANO_ROLL_MIN_TRANSPOSE..=PIANO_ROLL_MAX_TRANSPOSE)
            .rev()
            .map(|transpose| {
                let pitch_class = (transpose + 60).rem_euclid(12);
                let is_black_key = matches!(pitch_class, 1 | 3 | 6 | 8 | 10);
                let label = if pitch_class == 0 {
                    format!("C{}", 4 + transpose.div_euclid(12))
                } else {
                    String::new()
                };
                map_value([
                    (
                        "id",
                        Value::Number((PIANO_ROLL_MAX_TRANSPOSE - transpose) as f64),
                    ),
                    ("label", Value::String(label)),
                    (
                        "sidebar-bg",
                        Value::Keyword(if is_black_key { "black" } else { "white" }.to_string()),
                    ),
                    (
                        "label-fg",
                        Value::Keyword(if is_black_key { "white" } else { "black" }.to_string()),
                    ),
                ])
            }),
    )
}

fn piano_roll_find_note_index(
    lanes: &PianoRollLanes,
    step: usize,
    transpose: f32,
    duration: f32,
    delay: f32,
) -> Option<usize> {
    let notes = lanes.note_entries(step);
    notes
        .iter()
        .position(|note| {
            (note.transpose - transpose).abs() < f32::EPSILON
                && (note.duration - duration).abs() < f32::EPSILON
                && (note.delay - delay).abs() < f32::EPSILON
        })
        .or_else(|| {
            notes
                .iter()
                .position(|note| (note.transpose - transpose).abs() < f32::EPSILON)
        })
}

pub(crate) fn build_piano_roll_items_value(
    lanes: &PianoRollLanes,
    selected: &Arc<Mutex<HashSet<u64>>>,
) -> Value {
    let num_steps = lanes.num_steps();
    let selected = selected.lock().unwrap();
    let mut items = Vec::new();
    for (step, notes) in lanes.note_entries_batch(num_steps).into_iter().enumerate() {
        for (voice_idx, note) in notes.into_iter().enumerate() {
            let id = piano_roll_item_id(step, voice_idx);
            items.push(map_value([
                ("id", Value::Number(id as f64)),
                (
                    "lane",
                    Value::Number(piano_roll_transpose_to_lane(note.transpose) as f64),
                ),
                ("start", Value::Number((step as f32 + note.delay) as f64)),
                (
                    "end",
                    Value::Number((step as f32 + note.delay + note.duration) as f64),
                ),
                ("selected", Value::Bool(selected.contains(&id))),
                ("label", Value::String(piano_roll_note_label(&note))),
            ]));
        }
    }
    list_value(items)
}

pub(crate) fn build_piano_roll_selection_value(selected: &Arc<Mutex<HashSet<u64>>>) -> Value {
    let mut ids: Vec<u64> = selected.lock().unwrap().iter().copied().collect();
    ids.sort_unstable();
    list_value(ids.into_iter().map(|id| Value::Number(id as f64)))
}

/// Refresh the piano roll's reactive surfaces from the resolved focus
/// (spec 3.5): items and selection, plus `SEQ.focus-num-steps` (the focus
/// axis length — `SEQ.tp-num-steps` keeps meaning the live value until the
/// step grid is ported) and `SEQ.focus-label` for the header.
pub(crate) fn sync_piano_roll_state(
    rt: &mut Runtime,
    app: &sequencer::app::App,
    state: &Arc<SequencerState>,
    track: usize,
    selected: &Arc<Mutex<HashSet<u64>>>,
) {
    let focus = PianoRollFocusSpec::from_focus(app.track_edit_focus(track));
    let lanes = PianoRollLanes::new(state, track, focus);
    rt.set_reactive(
        "SEQ",
        "focus-num-steps",
        Value::Number(app.focus_num_steps(track) as f64),
    );
    rt.set_reactive(
        "SEQ",
        "focus-label",
        Value::String(app.focus_label(track).unwrap_or_default()),
    );
    // Whether the focus is the live mirror. The loop bar's length write is
    // still live-track-shaped (`seq-set-track-param :num-steps`), so a
    // pinned focus must keep it read-only until slice C lands the
    // pattern-addressed write — otherwise the band would DISPLAY the pinned
    // length while EDITING the live pattern.
    rt.set_reactive(
        "SEQ",
        "focus-live",
        Value::Bool(matches!(focus, PianoRollFocusSpec::Live)),
    );
    rt.set_reactive(
        "SEQ",
        "focus-kind",
        Value::Keyword(
            match focus {
                PianoRollFocusSpec::Live => "live",
                PianoRollFocusSpec::Pool(_) => "pattern",
                PianoRollFocusSpec::Take(_) => "take",
            }
            .to_string(),
        ),
    );
    // The pinned CLIP's source kind (:none/:pattern/:take), independent of
    // the resolved write focus: a pinned clip whose pattern is the effective
    // one still shows clip-shaped surfaces (overlay, band slide, panel).
    rt.set_reactive(
        "SEQ",
        "focus-clip-kind",
        Value::Keyword(
            app.focus_clip_source_kind(track)
                .unwrap_or("none")
                .to_string(),
        ),
    );
    // Loop-window overlay (spec 5): start marker at the clip's offset, the
    // played window when the span is under one source pass, repeat badge
    // when it covers several. All sentinel-shaped for the float channels.
    let overlay = app.focus_window_overlay(track);
    rt.set_reactive(
        "SEQ",
        "focus-window-marker",
        Value::Number(overlay.map(|(marker, _, _)| marker).unwrap_or(-1.0)),
    );
    rt.set_reactive(
        "SEQ",
        "focus-window-span",
        match overlay.and_then(|(_, span, _)| span) {
            Some((start, end)) => list_value([Value::Number(start), Value::Number(end)]),
            None => Value::Nil,
        },
    );
    rt.set_reactive(
        "SEQ",
        "focus-window-repeat",
        Value::Number(overlay.and_then(|(_, _, repeat)| repeat).unwrap_or(0.0)),
    );
    // Clip-panel fields (spec 6): Start/End/Offset for the pinned clip,
    // Nil-shaped when hidden (follow mode).
    let clip_fields = app.focus_clip_fields(track);
    rt.set_reactive(
        "SEQ",
        "focus-clip-start",
        clip_fields
            .map(|(start, _, _)| Value::Number(start))
            .unwrap_or(Value::Nil),
    );
    rt.set_reactive(
        "SEQ",
        "focus-clip-end",
        clip_fields
            .map(|(_, end, _)| Value::Number(end))
            .unwrap_or(Value::Nil),
    );
    rt.set_reactive(
        "SEQ",
        "focus-clip-offset",
        clip_fields
            .map(|(_, _, offset)| Value::Number(offset))
            .unwrap_or(Value::Nil),
    );
    sync_piano_roll_note_state(rt, &lanes, selected);
}

/// The lanes-level half of `sync_piano_roll_state`: items, selection, and the
/// pending view fit. Split out so tests can drive it without an `App`.
pub(crate) fn sync_piano_roll_note_state(
    rt: &mut Runtime,
    lanes: &PianoRollLanes,
    selected: &Arc<Mutex<HashSet<u64>>>,
) {
    rt.set_reactive(
        "SEQ",
        "piano-roll-items",
        build_piano_roll_items_value(lanes, selected),
    );
    rt.set_reactive(
        "SEQ",
        "piano-roll-selection",
        build_piano_roll_selection_value(selected),
    );
    apply_pending_piano_roll_fit(rt);
}

/// Publish the piano roll's own playhead channel (spec 3.3.4): the live
/// playhead in follow mode, a clip-relative position while the song sounds a
/// pinned focus, `-1` (hidden) otherwise.
pub(crate) fn sync_piano_roll_playhead(
    rt: &mut Runtime,
    app: &sequencer::app::App,
    track: usize,
    live_playhead: usize,
) -> bool {
    let value = app
        .focus_playhead_step(track, live_playhead)
        .unwrap_or(-1.0);
    rt.set_reactive("SEQ", "piano-roll-playhead", Value::Number(value))
        .effects_dirty
}

fn apply_pending_piano_roll_fit(rt: &mut Runtime) {
    if !matches!(
        rt.global_value("piano-roll-fit-pending"),
        Some(Value::Bool(true))
    ) {
        return;
    }
    let Some(callback) = rt.global_value("eseq.piano-roll/piano-roll-apply-pending-fit") else {
        return;
    };
    if let Err(error) = rt.invoke(callback, vec![]) {
        eprintln!("piano-roll pending fit failed: {error:?}");
    }
}

fn value_as_number(value: Option<&Value>) -> Option<f64> {
    match value {
        Some(Value::Number(n)) => Some(*n),
        _ => None,
    }
}

fn value_as_usize(value: Option<&Value>) -> Option<usize> {
    value_as_number(value).map(|n| n.max(0.0).round() as usize)
}

fn value_as_u64(value: Option<&Value>) -> Option<u64> {
    value_as_number(value).map(|n| n.max(0.0).round() as u64)
}

fn value_as_keyword_or_string(value: Option<&Value>) -> Option<String> {
    match value {
        Some(Value::Keyword(s)) | Some(Value::String(s)) => Some(s.clone()),
        _ => None,
    }
}

fn cloned_map(value: &Value) -> Result<HashMap<String, Value>, String> {
    let Value::Map(map) = value else {
        return Err("expected action map".to_string());
    };
    Ok(map
        .iter()
        .map(|(key, value)| (key.clone(), value.borrow().clone()))
        .collect())
}

fn parse_piano_roll_ids(value: Option<&Value>) -> Vec<u64> {
    let Some(Value::List(items)) = value else {
        return Vec::new();
    };
    items
        .iter()
        .filter_map(|item| value_as_u64(Some(&item.borrow())))
        .collect()
}

pub(crate) fn piano_roll_action_mutates_pattern(action: &Value) -> bool {
    let Ok(map) = cloned_map(action) else {
        return false;
    };
    matches!(
        value_as_keyword_or_string(map.get("type")).as_deref(),
        Some(
            "delete-items"
                | "nudge-selection"
                | "move-items-absolute"
                | "resize-item-absolute"
                | "paste-items"
                | "finish-create-item"
        )
    )
}

pub(crate) struct PianoRollHistoryPlan {
    pub(crate) label: &'static str,
    pub(crate) steps: Vec<usize>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum PianoRollGestureCommand {
    Update(PianoRollDragKind),
    Finish(PianoRollDragKind),
}

pub(crate) fn piano_roll_gesture_command(action: &Value) -> Option<PianoRollGestureCommand> {
    let map = cloned_map(action).ok()?;
    match value_as_keyword_or_string(map.get("type"))?.as_str() {
        "move-items-absolute" => Some(PianoRollGestureCommand::Update(PianoRollDragKind::Move)),
        "resize-item-absolute" => Some(PianoRollGestureCommand::Update(PianoRollDragKind::Resize)),
        "finish-move-items" => Some(PianoRollGestureCommand::Finish(PianoRollDragKind::Move)),
        "finish-resize-items" => Some(PianoRollGestureCommand::Finish(PianoRollDragKind::Resize)),
        _ => None,
    }
}

pub(crate) fn piano_roll_gesture_touched_steps(
    lanes: &PianoRollLanes,
    move_state: &Arc<Mutex<Option<PianoRollMoveState>>>,
    action: &Value,
) -> Result<Vec<usize>, String> {
    let map = cloned_map(action)?;
    let action_type = value_as_keyword_or_string(map.get("type"))
        .ok_or_else(|| "piano roll action missing :type".to_string())?;
    let mut ids = parse_piano_roll_ids(map.get("ids"));
    if ids.is_empty() {
        if let Some(id) = value_as_u64(map.get("id")) {
            ids.push(id);
        }
    }
    let mut steps = ids
        .iter()
        .filter_map(|id| lanes.item_parts(*id).map(|(step, _)| step))
        .collect::<Vec<_>>();
    if action_type == "resize-item-absolute" {
        steps.sort_unstable();
        steps.dedup();
        return Ok(steps);
    }
    if action_type != "move-items-absolute" {
        return Err(format!("{action_type} is not a piano-roll gesture update"));
    }

    let anchor_id = value_as_u64(map.get("anchor-id"))
        .or_else(|| ids.first().copied())
        .ok_or_else(|| "move-items-absolute missing anchor-id".to_string())?;
    let start = value_as_number(map.get("start")).unwrap_or(0.0) as f32;
    let mut sorted_ids = ids;
    sorted_ids.sort_unstable();
    let (originals, last_positions, anchor_start) = {
        let guard = move_state.lock().unwrap();
        if let Some(existing) = guard.as_ref().filter(|existing| {
            existing.kind == PianoRollDragKind::Move && existing.ids == sorted_ids
        }) {
            (
                existing.originals.clone(),
                existing.last_positions.clone(),
                existing.anchor_start,
            )
        } else {
            let (anchor_step, anchor_voice_idx) = lanes
                .item_parts(anchor_id)
                .ok_or_else(|| "move anchor was invalid".to_string())?;
            let anchor_note = lanes
                .note_entries(anchor_step)
                .get(anchor_voice_idx)
                .cloned()
                .ok_or_else(|| "move anchor no longer exists".to_string())?;
            let originals = sorted_ids
                .iter()
                .filter_map(|id| {
                    let (step, voice_idx) = lanes.item_parts(*id)?;
                    let note = lanes.note_entries(step).get(voice_idx).cloned()?;
                    Some(PianoRollMoveItem {
                        id: *id,
                        step,
                        transpose: note.transpose,
                        duration: note.duration,
                        delay: note.delay,
                    })
                })
                .collect::<Vec<_>>();
            (
                originals.clone(),
                originals,
                anchor_step as f32 + anchor_note.delay,
            )
        }
    };
    steps.extend(originals.iter().map(|item| item.step));
    steps.extend(last_positions.iter().map(|item| item.step));
    let num_steps = lanes.num_steps();
    for item in originals {
        let time_offset = item.step as f32 + item.delay - anchor_start;
        let (next_step, _) = piano_roll_time_to_step_delay((start + time_offset) as f64, num_steps);
        steps.push(next_step);
    }
    steps.sort_unstable();
    steps.dedup();
    Ok(steps)
}

pub(crate) fn piano_roll_history_plan(
    lanes: &PianoRollLanes,
    action: &Value,
    clipboard: &PianoRollClipboard,
) -> Result<Option<PianoRollHistoryPlan>, String> {
    let map = cloned_map(action)?;
    let Some(action_type) = value_as_keyword_or_string(map.get("type")) else {
        return Err("piano roll action missing :type".to_string());
    };
    let (label, mut steps) = match action_type.as_str() {
        "finish-create-item" => {
            let num_steps = lanes.num_steps();
            let start = value_as_number(map.get("start")).unwrap_or(0.0);
            let (step, _) = piano_roll_time_to_step_delay(start, num_steps);
            ("Create piano-roll note", vec![step])
        }
        "delete-items" => (
            "Delete piano-roll notes",
            parse_piano_roll_ids(map.get("ids"))
                .into_iter()
                .filter_map(|id| lanes.item_parts(id))
                .map(|(step, _)| step)
                .collect(),
        ),
        "nudge-selection" => {
            let num_steps = lanes.num_steps();
            let delta_time = value_as_number(map.get("delta-time"))
                .unwrap_or(0.0)
                .round() as isize;
            let source_steps = parse_piano_roll_ids(map.get("ids"))
                .into_iter()
                .filter_map(|id| lanes.item_parts(id))
                .map(|(step, _)| step)
                .collect::<Vec<_>>();
            let mut affected_steps = source_steps.clone();
            affected_steps.extend(source_steps.into_iter().map(|step| {
                (step as isize + delta_time).clamp(0, (num_steps - 1) as isize) as usize
            }));
            ("Nudge piano-roll notes", affected_steps)
        }
        "paste-items" => {
            let num_steps = lanes.num_steps();
            let start = value_as_number(map.get("time")).unwrap_or(0.0);
            let notes = clipboard.lock().unwrap().clone().unwrap_or_default();
            let affected_steps = notes
                .into_iter()
                .map(|note| {
                    piano_roll_time_to_step_delay(start + note.start_offset as f64, num_steps).0
                })
                .collect();
            ("Paste piano-roll notes", affected_steps)
        }
        _ => return Ok(None),
    };
    steps.sort_unstable();
    steps.dedup();
    Ok(Some(PianoRollHistoryPlan { label, steps }))
}

fn copy_piano_roll_items(
    lanes: &PianoRollLanes,
    ids: &[u64],
    clipboard: &PianoRollClipboard,
) -> usize {
    let mut notes = ids
        .iter()
        .filter_map(|&id| {
            let (step, voice_idx) = lanes.item_parts(id)?;
            let note = lanes.note_entries(step).get(voice_idx).cloned()?;
            Some((step as f32 + note.delay, note))
        })
        .collect::<Vec<_>>();
    notes.sort_by(|(start_a, note_a), (start_b, note_b)| {
        (*start_a, note_a.transpose)
            .partial_cmp(&(*start_b, note_b.transpose))
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    let Some(anchor_start) = notes.first().map(|(start, _)| *start) else {
        return 0;
    };
    let copied = notes
        .into_iter()
        .map(|(start, note)| PianoRollClipboardNote {
            start_offset: start - anchor_start,
            transpose: note.transpose,
            duration: note.duration,
        })
        .collect::<Vec<_>>();
    let count = copied.len();
    *clipboard.lock().unwrap() = Some(copied);
    count
}

fn paste_piano_roll_items(
    lanes: &PianoRollLanes,
    num_steps: usize,
    start: f64,
    selection: &Arc<Mutex<HashSet<u64>>>,
    clipboard: &PianoRollClipboard,
) -> usize {
    let Some(notes) = clipboard.lock().unwrap().clone() else {
        return 0;
    };
    let mut pasted_ids = Vec::new();
    for note in notes {
        let next_start = start + note.start_offset as f64;
        let (step, delay) = piano_roll_time_to_step_delay(next_start, num_steps);
        let mut step_notes = lanes.note_entries(step);
        step_notes.push(PianoRollNote {
            transpose: note.transpose,
            duration: note.duration,
            delay,
        });
        lanes.set_note_entries(step, &step_notes);
        if let Some(voice_idx) =
            piano_roll_find_note_index(lanes, step, note.transpose, note.duration, delay)
        {
            pasted_ids.push(piano_roll_item_id(step, voice_idx));
        }
    }
    let count = pasted_ids.len();
    let mut selected = selection.lock().unwrap();
    selected.clear();
    selected.extend(pasted_ids);
    count
}

pub(crate) fn apply_piano_roll_action(
    lanes: &PianoRollLanes,
    selection: &Arc<Mutex<HashSet<u64>>>,
    move_state: &Arc<Mutex<Option<PianoRollMoveState>>>,
    action: &Value,
) -> Result<String, String> {
    let clipboard = new_piano_roll_clipboard();
    apply_piano_roll_action_with_clipboard(lanes, selection, move_state, &clipboard, action)
}

pub(crate) fn apply_piano_roll_action_with_clipboard(
    lanes: &PianoRollLanes,
    selection: &Arc<Mutex<HashSet<u64>>>,
    move_state: &Arc<Mutex<Option<PianoRollMoveState>>>,
    clipboard: &PianoRollClipboard,
    action: &Value,
) -> Result<String, String> {
    let action = cloned_map(action)?;
    let action_type = value_as_keyword_or_string(action.get("type"))
        .ok_or_else(|| "piano roll action missing :type".to_string())?;
    let num_steps = lanes.num_steps();

    match action_type.as_str() {
        "select" => {
            *move_state.lock().unwrap() = None;
            let ids = parse_piano_roll_ids(action.get("ids"));
            let mut selected = selection.lock().unwrap();
            selected.clear();
            selected.extend(ids.iter().copied());
            Ok(format!("selected {} note(s)", ids.len()))
        }
        "clear-selection" => {
            *move_state.lock().unwrap() = None;
            selection.lock().unwrap().clear();
            Ok("piano roll selection cleared".to_string())
        }
        "marquee-select" | "finish-marquee-select" => {
            *move_state.lock().unwrap() = None;
            let time_a = value_as_number(action.get("time-a")).unwrap_or(0.0);
            let time_b = value_as_number(action.get("time-b")).unwrap_or(0.0);
            let lane_a = value_as_usize(action.get("lane-a")).unwrap_or(0);
            let lane_b = value_as_usize(action.get("lane-b")).unwrap_or(0);
            let lo_time = time_a.min(time_b);
            let hi_time = time_a.max(time_b);
            let lo_lane = lane_a.min(lane_b);
            let hi_lane = lane_a.max(lane_b);
            let mut ids = Vec::new();
            for step in 0..num_steps {
                for (voice_idx, note) in lanes.note_entries(step).into_iter().enumerate() {
                    let start = step as f64 + note.delay as f64;
                    let end = start + note.duration as f64;
                    if start >= hi_time || end <= lo_time {
                        continue;
                    }
                    let lane = piano_roll_transpose_to_lane(note.transpose);
                    if lane >= lo_lane && lane <= hi_lane {
                        ids.push(piano_roll_item_id(step, voice_idx));
                    }
                }
            }
            let mut selected = selection.lock().unwrap();
            selected.clear();
            selected.extend(ids.iter().copied());
            Ok(format!("marquee selected {} note(s)", ids.len()))
        }
        "delete-items" => {
            *move_state.lock().unwrap() = None;
            let ids = parse_piano_roll_ids(action.get("ids"));
            let mut by_step: HashMap<usize, Vec<usize>> = HashMap::new();
            for id in ids.iter().copied() {
                if let Some((step, voice_idx)) = lanes.item_parts(id) {
                    by_step.entry(step).or_default().push(voice_idx);
                }
            }
            for (step, mut voice_indices) in by_step {
                let notes = lanes.note_entries(step);
                let original_len = notes.len();
                voice_indices.sort_unstable();
                voice_indices.dedup();
                let remaining = notes
                    .into_iter()
                    .enumerate()
                    .filter_map(|(idx, note)| {
                        if voice_indices.binary_search(&idx).is_ok() {
                            None
                        } else {
                            Some(note)
                        }
                    })
                    .collect::<Vec<_>>();
                if remaining.len() != original_len {
                    lanes.set_note_entries(step, &remaining);
                }
            }
            selection.lock().unwrap().clear();
            Ok(format!("deleted {} note(s)", ids.len()))
        }
        "copy-items" => {
            *move_state.lock().unwrap() = None;
            let ids = parse_piano_roll_ids(action.get("ids"));
            let count = copy_piano_roll_items(lanes, &ids, clipboard);
            Ok(format!("copied {} note(s)", count))
        }
        "paste-items" => {
            *move_state.lock().unwrap() = None;
            let start = value_as_number(action.get("time")).unwrap_or(0.0);
            let count = paste_piano_roll_items(lanes, num_steps, start, selection, clipboard);
            Ok(format!("pasted {} note(s)", count))
        }
        "create-item" => {
            *move_state.lock().unwrap() = None;
            Ok("drawing note".to_string())
        }
        "finish-create-item" => {
            *move_state.lock().unwrap() = None;
            let start = value_as_number(action.get("start")).unwrap_or(0.0);
            let (step, delay) = piano_roll_time_to_step_delay(start, num_steps);
            let lane = value_as_usize(action.get("lane")).unwrap_or(0);
            let duration =
                (value_as_number(action.get("end")).unwrap_or(start + 1.0) - start) as f32;
            let duration = piano_roll_sanitize_duration(duration);
            let transpose = piano_roll_lane_to_transpose(lane);
            let mut notes = lanes.note_entries(step);
            notes.push(PianoRollNote {
                transpose,
                duration,
                delay,
            });
            lanes.set_note_entries(step, &notes);
            let id = lanes
                .note_entries(step)
                .iter()
                .position(|note| {
                    (note.transpose - transpose).abs() < f32::EPSILON
                        && (note.delay - delay).abs() < f32::EPSILON
                })
                .map(|voice_idx| piano_roll_item_id(step, voice_idx))
                .unwrap_or_else(|| piano_roll_item_id(step, 0));
            let mut selected = selection.lock().unwrap();
            selected.clear();
            selected.insert(id);
            Ok(format!("created note step {} {transpose:+.0}", step + 1))
        }
        "nudge-selection" => {
            *move_state.lock().unwrap() = None;
            let ids = parse_piano_roll_ids(action.get("ids"));
            let delta_time = value_as_number(action.get("delta-time"))
                .unwrap_or(0.0)
                .round() as isize;
            let delta_lane = value_as_number(action.get("delta-lane"))
                .unwrap_or(0.0)
                .round() as isize;
            let next_ids =
                move_piano_roll_items_by_delta(lanes, num_steps, &ids, delta_time, delta_lane);
            let mut selected = selection.lock().unwrap();
            selected.clear();
            selected.extend(next_ids);
            Ok(format!("nudged {} note(s)", ids.len()))
        }
        "move-items-absolute" => {
            let ids = parse_piano_roll_ids(action.get("ids"));
            let anchor_id = value_as_u64(action.get("anchor-id"))
                .or_else(|| ids.first().copied())
                .ok_or_else(|| "move-items-absolute missing anchor-id".to_string())?;
            let start = value_as_number(action.get("start")).unwrap_or(0.0) as f32;
            let lane = value_as_usize(action.get("lane")).unwrap_or(0) as isize;
            let next_ids = move_piano_roll_items_absolute(
                lanes, num_steps, &ids, anchor_id, start, lane, move_state,
            );
            let mut selected = selection.lock().unwrap();
            selected.clear();
            selected.extend(next_ids);
            Ok(format!("moved {} note(s)", ids.len()))
        }
        "resize-item-absolute" => {
            let id = value_as_u64(action.get("id"))
                .ok_or_else(|| "resize-item-absolute missing id".to_string())?;
            if value_as_keyword_or_string(action.get("edge")).as_deref() == Some("start") {
                return Ok("piano roll start resize ignored".to_string());
            }
            let time = value_as_number(action.get("time")).unwrap_or(0.0) as f32;
            let ids = parse_piano_roll_ids(action.get("ids"));
            let duration_delta = value_as_number(action.get("duration-delta")).map(|n| n as f32);
            if let Some((step, _voice_idx)) = lanes.item_parts(id) {
                let resize_ids = if ids.is_empty() { vec![id] } else { ids };
                let resized = resize_piano_roll_items_absolute(
                    lanes,
                    &resize_ids,
                    id,
                    step,
                    time,
                    duration_delta,
                    move_state,
                );
                Ok(format!("resized {} note(s)", resized))
            } else {
                Ok("resize ignored".to_string())
            }
        }
        "set-cursor" => Ok("piano roll cursor".to_string()),
        "scroll-view" | "zoom-view" | "set-tool" => Ok("piano roll view".to_string()),
        other => Ok(format!("ignored piano roll action {other}")),
    }
}

fn move_piano_roll_items_by_delta(
    lanes: &PianoRollLanes,
    num_steps: usize,
    ids: &[u64],
    delta_time: isize,
    delta_lane: isize,
) -> Vec<u64> {
    let originals = ids
        .iter()
        .filter_map(|&id| {
            let (step, voice_idx) = lanes.item_parts(id)?;
            let notes = lanes.note_entries(step);
            let note = notes.get(voice_idx)?;
            Some((
                id,
                step,
                voice_idx,
                note.transpose,
                note.duration,
                note.delay,
            ))
        })
        .collect::<Vec<_>>();
    let mut removals_by_step: HashMap<usize, Vec<usize>> = HashMap::new();
    for &(_, step, voice_idx, _, _, _) in &originals {
        removals_by_step.entry(step).or_default().push(voice_idx);
    }
    for (step, mut voice_indices) in removals_by_step {
        voice_indices.sort_unstable();
        voice_indices.dedup();
        let notes = lanes
            .note_entries(step)
            .into_iter()
            .enumerate()
            .filter_map(|(idx, note)| {
                if voice_indices.binary_search(&idx).is_ok() {
                    None
                } else {
                    Some(note)
                }
            })
            .collect::<Vec<_>>();
        lanes.set_note_entries(step, &notes);
    }

    let mut next_ids = Vec::with_capacity(originals.len());
    for &(_, step, _, transpose, duration, delay) in &originals {
        let next_step = (step as isize + delta_time).clamp(0, (num_steps - 1) as isize) as usize;
        let lane = piano_roll_transpose_to_lane(transpose) as isize + delta_lane;
        let next_transpose = piano_roll_lane_to_transpose(lane.max(0) as usize);
        let mut notes = lanes.note_entries(next_step);
        notes.push(PianoRollNote {
            transpose: next_transpose,
            duration,
            delay,
        });
        lanes.set_note_entries(next_step, &notes);
        if let Some(next_voice_idx) =
            piano_roll_find_note_index(lanes, next_step, next_transpose, duration, delay)
        {
            next_ids.push(piano_roll_item_id(next_step, next_voice_idx));
        }
    }
    next_ids
}

fn resize_piano_roll_items_absolute(
    lanes: &PianoRollLanes,
    ids: &[u64],
    anchor_id: u64,
    anchor_step: usize,
    time: f32,
    duration_delta: Option<f32>,
    move_state: &Arc<Mutex<Option<PianoRollMoveState>>>,
) -> usize {
    let mut sorted_ids = ids.to_vec();
    sorted_ids.sort_unstable();

    let mut guard = move_state.lock().unwrap();
    let needs_new_state = guard
        .as_ref()
        .map(|state| {
            state.kind != PianoRollDragKind::Resize
                || state.ids != sorted_ids
                || state.anchor_id != anchor_id
        })
        .unwrap_or(true);

    if needs_new_state {
        let originals = ids
            .iter()
            .filter_map(|&id| {
                let (step, voice_idx) = lanes.item_parts(id)?;
                let notes = lanes.note_entries(step);
                let note = notes.get(voice_idx)?;
                Some(PianoRollMoveItem {
                    id,
                    step,
                    transpose: note.transpose,
                    duration: note.duration,
                    delay: note.delay,
                })
            })
            .collect::<Vec<_>>();
        if originals.is_empty() {
            return 0;
        }
        *guard = Some(PianoRollMoveState {
            kind: PianoRollDragKind::Resize,
            ids: sorted_ids,
            anchor_id,
            anchor_start: anchor_step as f32 + lanes.step_delay(anchor_step),
            anchor_lane: 0,
            last_positions: originals.clone(),
            originals,
        });
    }

    let Some(resize_state) = guard.as_ref() else {
        return 0;
    };
    let Some(anchor) = resize_state
        .originals
        .iter()
        .find(|item| item.id == anchor_id)
    else {
        return 0;
    };
    let anchor_duration = piano_roll_sanitize_duration(time - resize_state.anchor_start);
    let delta = anchor_duration - anchor.duration;

    let mut next_by_step: HashMap<usize, Vec<PianoRollNote>> = HashMap::new();
    for item in &resize_state.originals {
        next_by_step.entry(item.step).or_insert_with(|| {
            lanes
                .note_entries(item.step)
                .into_iter()
                .map(|note| {
                    resize_state
                        .originals
                        .iter()
                        .find(|original| {
                            original.step == item.step
                                && (original.transpose - note.transpose).abs() < f32::EPSILON
                        })
                        .map(|original| PianoRollNote {
                            transpose: original.transpose,
                            duration: original.duration,
                            delay: original.delay,
                        })
                        .unwrap_or(note)
                })
                .collect()
        });
    }

    let mut resized = 0;
    for item in &resize_state.originals {
        let duration = if item.id == anchor_id {
            anchor_duration
        } else if duration_delta.is_some() {
            piano_roll_sanitize_duration(item.duration + delta)
        } else {
            continue;
        };
        let Some(notes) = next_by_step.get_mut(&item.step) else {
            continue;
        };
        if let Some(note) = notes.iter_mut().find(|note| {
            (note.transpose - item.transpose).abs() < f32::EPSILON
                && (note.delay - item.delay).abs() < f32::EPSILON
        }) {
            note.duration = duration;
            resized += 1;
        }
    }

    for (step, notes) in next_by_step {
        lanes.set_note_entries(step, &notes);
    }
    resized
}

fn move_piano_roll_items_absolute(
    lanes: &PianoRollLanes,
    num_steps: usize,
    ids: &[u64],
    anchor_id: u64,
    start: f32,
    lane: isize,
    move_state: &Arc<Mutex<Option<PianoRollMoveState>>>,
) -> Vec<u64> {
    let mut sorted_ids = ids.to_vec();
    sorted_ids.sort_unstable();

    let mut guard = move_state.lock().unwrap();
    let needs_new_state = guard
        .as_ref()
        .map(|state| state.kind != PianoRollDragKind::Move || state.ids != sorted_ids)
        .unwrap_or(true);

    if needs_new_state {
        let Some((anchor_step, anchor_voice_idx)) = lanes.item_parts(anchor_id) else {
            return Vec::new();
        };
        let anchor_notes = lanes.note_entries(anchor_step);
        let Some(anchor_note) = anchor_notes.get(anchor_voice_idx) else {
            return Vec::new();
        };
        let anchor_lane = piano_roll_transpose_to_lane(anchor_note.transpose) as isize;
        let originals = ids
            .iter()
            .filter_map(|&id| {
                let (step, voice_idx) = lanes.item_parts(id)?;
                let notes = lanes.note_entries(step);
                let note = notes.get(voice_idx)?;
                Some(PianoRollMoveItem {
                    id,
                    step,
                    transpose: note.transpose,
                    duration: note.duration,
                    delay: note.delay,
                })
            })
            .collect::<Vec<_>>();
        if originals.is_empty() {
            return Vec::new();
        }
        *guard = Some(PianoRollMoveState {
            kind: PianoRollDragKind::Move,
            ids: sorted_ids,
            anchor_id,
            anchor_start: anchor_step as f32 + anchor_note.delay,
            anchor_lane,
            last_positions: originals.clone(),
            originals,
        });
    }

    let Some(move_state) = guard.as_mut() else {
        return Vec::new();
    };

    for item in &move_state.last_positions {
        let mut notes = lanes.note_entries(item.step);
        if let Some(pos) = notes.iter().position(|note| {
            (note.transpose - item.transpose).abs() < f32::EPSILON
                && (note.delay - item.delay).abs() < f32::EPSILON
        }) {
            notes.remove(pos);
            lanes.set_note_entries(item.step, &notes);
        }
    }

    let mut next_positions = Vec::with_capacity(move_state.originals.len());
    for item in &move_state.originals {
        let item_start = item.step as f32 + item.delay;
        let time_offset = item_start - move_state.anchor_start;
        let lane_offset =
            piano_roll_transpose_to_lane(item.transpose) as isize - move_state.anchor_lane;
        let (next_step, next_delay) =
            piano_roll_time_to_step_delay((start + time_offset) as f64, num_steps);
        let next_lane = (lane + lane_offset).max(0) as usize;
        let next_transpose = piano_roll_lane_to_transpose(next_lane);
        let mut notes = lanes.note_entries(next_step);
        notes.push(PianoRollNote {
            transpose: next_transpose,
            duration: item.duration,
            delay: next_delay,
        });
        lanes.set_note_entries(next_step, &notes);
        let next_voice_idx =
            piano_roll_find_note_index(lanes, next_step, next_transpose, item.duration, next_delay)
                .unwrap_or(0);
        next_positions.push(PianoRollMoveItem {
            id: piano_roll_item_id(next_step, next_voice_idx),
            step: next_step,
            transpose: next_transpose,
            duration: item.duration,
            delay: next_delay,
        });
    }
    move_state.last_positions = next_positions;
    move_state
        .last_positions
        .iter()
        .map(|item| item.id)
        .collect()
}
