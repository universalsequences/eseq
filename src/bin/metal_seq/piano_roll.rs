use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

use eseqlisp::vm::Value;
use eseqlisp::Runtime;

use sequencer::sequencer::{SequencerState, StepParam, MAX_STEPS};

use super::values::{list_value, map_value};

const PIANO_ROLL_ID_STRIDE: usize = 16;
const PIANO_ROLL_MIN_TRANSPOSE: i32 = -48;
const PIANO_ROLL_MAX_TRANSPOSE: i32 = 48;
const PIANO_ROLL_MIN_DURATION: f32 = 0.03125;

#[derive(Clone)]
struct PianoRollNote {
    transpose: f32,
    duration: f32,
}

fn piano_roll_sanitize_duration(duration: f32) -> f32 {
    duration.max(PIANO_ROLL_MIN_DURATION)
}

#[derive(Clone)]
struct PianoRollMoveItem {
    id: u64,
    step: usize,
    transpose: f32,
    duration: f32,
}

pub(crate) struct PianoRollMoveState {
    ids: Vec<u64>,
    anchor_step: usize,
    anchor_lane: isize,
    originals: Vec<PianoRollMoveItem>,
    last_positions: Vec<PianoRollMoveItem>,
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

pub(crate) fn piano_roll_item_id(step: usize, voice_idx: usize) -> u64 {
    (step * PIANO_ROLL_ID_STRIDE + voice_idx.min(PIANO_ROLL_ID_STRIDE - 1)) as u64
}

fn piano_roll_item_parts(id: u64) -> Option<(usize, usize)> {
    let id = id as usize;
    let step = id / PIANO_ROLL_ID_STRIDE;
    let voice_idx = id % PIANO_ROLL_ID_STRIDE;
    if step < MAX_STEPS {
        Some((step, voice_idx))
    } else {
        None
    }
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

fn piano_roll_step_note_entries(
    state: &Arc<SequencerState>,
    track: usize,
    step: usize,
) -> Vec<PianoRollNote> {
    let step_duration = state.pattern.step_data[track]
        .get(step, StepParam::Duration)
        .max(PIANO_ROLL_MIN_DURATION);
    let chord_count = state.pattern.chord_data[track].count(step);
    if chord_count == 0 {
        if state.pattern.patterns[track].is_active(step) {
            vec![PianoRollNote {
                transpose: state.pattern.step_data[track].get(step, StepParam::Transpose),
                duration: step_duration,
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
                }
            })
            .collect()
    }
}

fn set_piano_roll_step_note_entries(
    state: &Arc<SequencerState>,
    track: usize,
    step: usize,
    notes: &[PianoRollNote],
) {
    let mut notes = notes
        .iter()
        .map(|note| PianoRollNote {
            transpose: note
                .transpose
                .round()
                .clamp(StepParam::Transpose.min(), StepParam::Transpose.max()),
            duration: piano_roll_sanitize_duration(note.duration),
        })
        .collect::<Vec<_>>();
    notes.sort_by(|a, b| {
        a.transpose
            .partial_cmp(&b.transpose)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    notes.dedup_by(|a, b| (a.transpose - b.transpose).abs() < f32::EPSILON);

    state.pattern.chord_data[track].clear_step(step);
    match notes.as_slice() {
        [] => state.pattern.patterns[track].set_step_active(step, false),
        [note] => {
            state.pattern.step_data[track].set(step, StepParam::Transpose, note.transpose);
            state.pattern.step_data[track].set(step, StepParam::Duration, note.duration);
            state.pattern.patterns[track].set_step_active(step, true);
        }
        notes => {
            let max_duration = notes
                .iter()
                .map(|note| note.duration)
                .fold(PIANO_ROLL_MIN_DURATION, f32::max);
            for note in notes {
                state.pattern.chord_data[track].add_note_with_duration(
                    step,
                    note.transpose,
                    note.duration,
                );
            }
            state.pattern.step_data[track].set(step, StepParam::Transpose, notes[0].transpose);
            state.pattern.step_data[track].set(step, StepParam::Duration, max_duration);
            state.pattern.patterns[track].set_step_active(step, true);
        }
    }
}

fn piano_roll_find_note_index(
    state: &Arc<SequencerState>,
    track: usize,
    step: usize,
    transpose: f32,
    duration: f32,
) -> Option<usize> {
    piano_roll_step_note_entries(state, track, step)
        .iter()
        .position(|note| {
            (note.transpose - transpose).abs() < f32::EPSILON
                && (note.duration - duration).abs() < f32::EPSILON
        })
        .or_else(|| {
            piano_roll_step_note_entries(state, track, step)
                .iter()
                .position(|note| (note.transpose - transpose).abs() < f32::EPSILON)
        })
}

pub(crate) fn build_piano_roll_items_value(
    state: &Arc<SequencerState>,
    track: usize,
    selected: &Arc<Mutex<HashSet<u64>>>,
) -> Value {
    let num_steps = state.pattern.track_params[track]
        .get_num_steps()
        .min(MAX_STEPS);
    let selected = selected.lock().unwrap();
    let mut items = Vec::new();
    for step in 0..num_steps {
        let notes = piano_roll_step_note_entries(state, track, step);
        for (voice_idx, note) in notes.into_iter().enumerate() {
            let id = piano_roll_item_id(step, voice_idx);
            items.push(map_value([
                ("id", Value::Number(id as f64)),
                (
                    "lane",
                    Value::Number(piano_roll_transpose_to_lane(note.transpose) as f64),
                ),
                ("start", Value::Number(step as f64)),
                ("end", Value::Number((step as f32 + note.duration) as f64)),
                ("selected", Value::Bool(selected.contains(&id))),
                (
                    "label",
                    Value::String(piano_roll_transpose_label(note.transpose)),
                ),
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

pub(crate) fn sync_piano_roll_state(
    rt: &mut Runtime,
    state: &Arc<SequencerState>,
    track: usize,
    selected: &Arc<Mutex<HashSet<u64>>>,
) {
    rt.set_reactive(
        "SEQ",
        "piano-roll-items",
        build_piano_roll_items_value(state, track, selected),
    );
    rt.set_reactive(
        "SEQ",
        "piano-roll-selection",
        build_piano_roll_selection_value(selected),
    );
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
                | "finish-create-item"
        )
    )
}

pub(crate) fn apply_piano_roll_action(
    state: &Arc<SequencerState>,
    track: usize,
    selection: &Arc<Mutex<HashSet<u64>>>,
    move_state: &Arc<Mutex<Option<PianoRollMoveState>>>,
    action: &Value,
) -> Result<String, String> {
    let action = cloned_map(action)?;
    let action_type = value_as_keyword_or_string(action.get("type"))
        .ok_or_else(|| "piano roll action missing :type".to_string())?;
    let num_steps = state.pattern.track_params[track]
        .get_num_steps()
        .min(MAX_STEPS)
        .max(1);

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
                let start = step as f64;
                for (voice_idx, note) in piano_roll_step_note_entries(state, track, step)
                    .into_iter()
                    .enumerate()
                {
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
                if let Some((step, voice_idx)) = piano_roll_item_parts(id) {
                    by_step.entry(step).or_default().push(voice_idx);
                }
            }
            for (step, mut voice_indices) in by_step {
                let notes = piano_roll_step_note_entries(state, track, step);
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
                    set_piano_roll_step_note_entries(state, track, step, &remaining);
                }
            }
            selection.lock().unwrap().clear();
            Ok(format!("deleted {} note(s)", ids.len()))
        }
        "create-item" => {
            *move_state.lock().unwrap() = None;
            Ok("drawing note".to_string())
        }
        "finish-create-item" => {
            *move_state.lock().unwrap() = None;
            let step = value_as_number(action.get("start"))
                .unwrap_or(0.0)
                .round()
                .clamp(0.0, (num_steps - 1) as f64) as usize;
            let lane = value_as_usize(action.get("lane")).unwrap_or(0);
            let duration = (value_as_number(action.get("end")).unwrap_or(step as f64 + 1.0)
                - step as f64) as f32;
            let duration = piano_roll_sanitize_duration(duration);
            let transpose = piano_roll_lane_to_transpose(lane);
            let mut notes = piano_roll_step_note_entries(state, track, step);
            notes.push(PianoRollNote {
                transpose,
                duration,
            });
            set_piano_roll_step_note_entries(state, track, step, &notes);
            let id = piano_roll_step_note_entries(state, track, step)
                .iter()
                .position(|note| (note.transpose - transpose).abs() < f32::EPSILON)
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
            let next_ids = move_piano_roll_items_by_delta(
                state, track, num_steps, &ids, delta_time, delta_lane,
            );
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
            let start = value_as_number(action.get("start")).unwrap_or(0.0).round() as isize;
            let lane = value_as_usize(action.get("lane")).unwrap_or(0) as isize;
            let next_ids = move_piano_roll_items_absolute(
                state, track, num_steps, &ids, anchor_id, start, lane, move_state,
            );
            let mut selected = selection.lock().unwrap();
            selected.clear();
            selected.extend(next_ids);
            Ok(format!("moved {} note(s)", ids.len()))
        }
        "resize-item-absolute" => {
            *move_state.lock().unwrap() = None;
            let id = value_as_u64(action.get("id"))
                .ok_or_else(|| "resize-item-absolute missing id".to_string())?;
            if value_as_keyword_or_string(action.get("edge")).as_deref() == Some("start") {
                return Ok("piano roll start resize ignored".to_string());
            }
            let time = value_as_number(action.get("time")).unwrap_or(0.0) as f32;
            let ids = parse_piano_roll_ids(action.get("ids"));
            let duration_delta = value_as_number(action.get("duration-delta")).map(|n| n as f32);
            if let Some((step, _voice_idx)) = piano_roll_item_parts(id) {
                let resize_ids = if ids.is_empty() { vec![id] } else { ids };
                let mut resized = 0;
                for resize_id in resize_ids {
                    let Some((resize_step, resize_voice_idx)) = piano_roll_item_parts(resize_id)
                    else {
                        continue;
                    };
                    let mut notes = piano_roll_step_note_entries(state, track, resize_step);
                    let duration = if resize_id == id {
                        piano_roll_sanitize_duration(time - step as f32)
                    } else if let Some(delta) = duration_delta {
                        let Some(note) = notes.get(resize_voice_idx) else {
                            continue;
                        };
                        piano_roll_sanitize_duration(note.duration + delta)
                    } else {
                        continue;
                    };
                    if let Some(note) = notes.get_mut(resize_voice_idx) {
                        note.duration = duration;
                        set_piano_roll_step_note_entries(state, track, resize_step, &notes);
                        resized += 1;
                    } else if resize_id == id {
                        state.pattern.step_data[track].set(
                            resize_step,
                            StepParam::Duration,
                            duration,
                        );
                        resized += 1;
                    }
                }
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
    state: &Arc<SequencerState>,
    track: usize,
    num_steps: usize,
    ids: &[u64],
    delta_time: isize,
    delta_lane: isize,
) -> Vec<u64> {
    let originals = ids
        .iter()
        .filter_map(|&id| {
            let (step, voice_idx) = piano_roll_item_parts(id)?;
            let notes = piano_roll_step_note_entries(state, track, step);
            let note = notes.get(voice_idx)?;
            Some((id, step, voice_idx, note.transpose, note.duration))
        })
        .collect::<Vec<_>>();
    let mut next_ids = Vec::with_capacity(originals.len());
    for &(_, step, voice_idx, _, _) in &originals {
        let mut notes = piano_roll_step_note_entries(state, track, step);
        if voice_idx < notes.len() {
            notes.remove(voice_idx);
            set_piano_roll_step_note_entries(state, track, step, &notes);
        }
    }
    for &(_, step, _, transpose, duration) in &originals {
        let next_step = (step as isize + delta_time).clamp(0, (num_steps - 1) as isize) as usize;
        let lane = piano_roll_transpose_to_lane(transpose) as isize + delta_lane;
        let next_transpose = piano_roll_lane_to_transpose(lane.max(0) as usize);
        let mut notes = piano_roll_step_note_entries(state, track, next_step);
        notes.push(PianoRollNote {
            transpose: next_transpose,
            duration,
        });
        set_piano_roll_step_note_entries(state, track, next_step, &notes);
        if let Some(next_voice_idx) =
            piano_roll_find_note_index(state, track, next_step, next_transpose, duration)
        {
            next_ids.push(piano_roll_item_id(next_step, next_voice_idx));
        }
    }
    next_ids
}

fn move_piano_roll_items_absolute(
    state: &Arc<SequencerState>,
    track: usize,
    num_steps: usize,
    ids: &[u64],
    anchor_id: u64,
    start: isize,
    lane: isize,
    move_state: &Arc<Mutex<Option<PianoRollMoveState>>>,
) -> Vec<u64> {
    let mut sorted_ids = ids.to_vec();
    sorted_ids.sort_unstable();

    let mut guard = move_state.lock().unwrap();
    let needs_new_state = guard
        .as_ref()
        .map(|state| state.ids != sorted_ids)
        .unwrap_or(true);

    if needs_new_state {
        let Some((anchor_step, anchor_voice_idx)) = piano_roll_item_parts(anchor_id) else {
            return Vec::new();
        };
        let anchor_notes = piano_roll_step_note_entries(state, track, anchor_step);
        let Some(anchor_note) = anchor_notes.get(anchor_voice_idx) else {
            return Vec::new();
        };
        let anchor_lane = piano_roll_transpose_to_lane(anchor_note.transpose) as isize;
        let originals = ids
            .iter()
            .filter_map(|&id| {
                let (step, voice_idx) = piano_roll_item_parts(id)?;
                let notes = piano_roll_step_note_entries(state, track, step);
                let note = notes.get(voice_idx)?;
                Some(PianoRollMoveItem {
                    id,
                    step,
                    transpose: note.transpose,
                    duration: note.duration,
                })
            })
            .collect::<Vec<_>>();
        if originals.is_empty() {
            return Vec::new();
        }
        *guard = Some(PianoRollMoveState {
            ids: sorted_ids,
            anchor_step,
            anchor_lane,
            last_positions: originals.clone(),
            originals,
        });
    }

    let Some(move_state) = guard.as_mut() else {
        return Vec::new();
    };

    for item in &move_state.last_positions {
        let mut notes = piano_roll_step_note_entries(state, track, item.step);
        if let Some(pos) = notes
            .iter()
            .position(|note| (note.transpose - item.transpose).abs() < f32::EPSILON)
        {
            notes.remove(pos);
            set_piano_roll_step_note_entries(state, track, item.step, &notes);
        }
    }

    let mut next_positions = Vec::with_capacity(move_state.originals.len());
    for item in &move_state.originals {
        let step_offset = item.step as isize - move_state.anchor_step as isize;
        let lane_offset =
            piano_roll_transpose_to_lane(item.transpose) as isize - move_state.anchor_lane;
        let next_step = (start + step_offset).clamp(0, (num_steps - 1) as isize) as usize;
        let next_lane = (lane + lane_offset).max(0) as usize;
        let next_transpose = piano_roll_lane_to_transpose(next_lane);
        let mut notes = piano_roll_step_note_entries(state, track, next_step);
        notes.push(PianoRollNote {
            transpose: next_transpose,
            duration: item.duration,
        });
        set_piano_roll_step_note_entries(state, track, next_step, &notes);
        let next_voice_idx =
            piano_roll_find_note_index(state, track, next_step, next_transpose, item.duration)
                .unwrap_or(0);
        next_positions.push(PianoRollMoveItem {
            id: piano_roll_item_id(next_step, next_voice_idx),
            step: next_step,
            transpose: next_transpose,
            duration: item.duration,
        });
    }
    move_state.last_positions = next_positions;
    move_state
        .last_positions
        .iter()
        .map(|item| item.id)
        .collect()
}
