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
    delay: f32,
}

fn piano_roll_sanitize_duration(duration: f32) -> f32 {
    duration.max(PIANO_ROLL_MIN_DURATION)
}

fn piano_roll_sanitize_delay(delay: f32) -> f32 {
    delay.clamp(StepParam::Delay.min(), StepParam::Delay.max())
}

fn piano_roll_step_delay(state: &Arc<SequencerState>, track: usize, step: usize) -> f32 {
    piano_roll_sanitize_delay(state.pattern.step_data[track].get(step, StepParam::Delay))
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
    let step_delay = piano_roll_step_delay(state, track, step);
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

    state.pattern.chord_data[track].clear_step(step);
    match notes.as_slice() {
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

fn piano_roll_find_note_index(
    state: &Arc<SequencerState>,
    track: usize,
    step: usize,
    transpose: f32,
    duration: f32,
    delay: f32,
) -> Option<usize> {
    piano_roll_step_note_entries(state, track, step)
        .iter()
        .position(|note| {
            (note.transpose - transpose).abs() < f32::EPSILON
                && (note.duration - duration).abs() < f32::EPSILON
                && (note.delay - delay).abs() < f32::EPSILON
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
    apply_pending_piano_roll_fit(rt);
}

fn apply_pending_piano_roll_fit(rt: &mut Runtime) {
    if !matches!(
        rt.global_value("piano-roll-fit-pending"),
        Some(Value::Bool(true))
    ) {
        return;
    }
    let Some(callback) = rt.global_value("piano-roll-apply-pending-fit") else {
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
        "resize-item-absolute" => {
            Some(PianoRollGestureCommand::Update(PianoRollDragKind::Resize))
        }
        "finish-move-items" => Some(PianoRollGestureCommand::Finish(PianoRollDragKind::Move)),
        "finish-resize-items" => {
            Some(PianoRollGestureCommand::Finish(PianoRollDragKind::Resize))
        }
        _ => None,
    }
}

pub(crate) fn piano_roll_gesture_touched_steps(
    state: &Arc<SequencerState>,
    track: usize,
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
        .filter_map(|id| piano_roll_item_parts(*id).map(|(step, _)| step))
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
        if let Some(existing) = guard
            .as_ref()
            .filter(|existing| {
                existing.kind == PianoRollDragKind::Move && existing.ids == sorted_ids
            })
        {
            (
                existing.originals.clone(),
                existing.last_positions.clone(),
                existing.anchor_start,
            )
        } else {
            let (anchor_step, anchor_voice_idx) = piano_roll_item_parts(anchor_id)
                .ok_or_else(|| "move anchor was invalid".to_string())?;
            let anchor_note = piano_roll_step_note_entries(state, track, anchor_step)
                .get(anchor_voice_idx)
                .cloned()
                .ok_or_else(|| "move anchor no longer exists".to_string())?;
            let originals = sorted_ids
                .iter()
                .filter_map(|id| {
                    let (step, voice_idx) = piano_roll_item_parts(*id)?;
                    let note = piano_roll_step_note_entries(state, track, step)
                        .get(voice_idx)
                        .cloned()?;
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
    let num_steps = state.pattern.track_params[track]
        .get_num_steps()
        .min(MAX_STEPS)
        .max(1);
    for item in originals {
        let time_offset = item.step as f32 + item.delay - anchor_start;
        let (next_step, _) =
            piano_roll_time_to_step_delay((start + time_offset) as f64, num_steps);
        steps.push(next_step);
    }
    steps.sort_unstable();
    steps.dedup();
    Ok(steps)
}

pub(crate) fn piano_roll_history_plan(
    state: &Arc<SequencerState>,
    track: usize,
    action: &Value,
    clipboard: &PianoRollClipboard,
) -> Result<Option<PianoRollHistoryPlan>, String> {
    let map = cloned_map(action)?;
    let Some(action_type) = value_as_keyword_or_string(map.get("type")) else {
        return Err("piano roll action missing :type".to_string());
    };
    let (label, mut steps) = match action_type.as_str() {
        "finish-create-item" => {
            let num_steps = state.pattern.track_params[track]
                .get_num_steps()
                .min(MAX_STEPS)
                .max(1);
            let start = value_as_number(map.get("start")).unwrap_or(0.0);
            let (step, _) = piano_roll_time_to_step_delay(start, num_steps);
            ("Create piano-roll note", vec![step])
        }
        "delete-items" => (
            "Delete piano-roll notes",
            parse_piano_roll_ids(map.get("ids"))
                .into_iter()
                .filter_map(piano_roll_item_parts)
                .map(|(step, _)| step)
                .collect(),
        ),
        "nudge-selection" => {
            let num_steps = state.pattern.track_params[track]
                .get_num_steps()
                .min(MAX_STEPS)
                .max(1);
            let delta_time = value_as_number(map.get("delta-time"))
                .unwrap_or(0.0)
                .round() as isize;
            let source_steps = parse_piano_roll_ids(map.get("ids"))
                .into_iter()
                .filter_map(piano_roll_item_parts)
                .map(|(step, _)| step)
                .collect::<Vec<_>>();
            let mut affected_steps = source_steps.clone();
            affected_steps.extend(source_steps.into_iter().map(|step| {
                (step as isize + delta_time).clamp(0, (num_steps - 1) as isize) as usize
            }));
            ("Nudge piano-roll notes", affected_steps)
        }
        "paste-items" => {
            let num_steps = state.pattern.track_params[track]
                .get_num_steps()
                .min(MAX_STEPS)
                .max(1);
            let start = value_as_number(map.get("time")).unwrap_or(0.0);
            let notes = clipboard.lock().unwrap().clone().unwrap_or_default();
            let affected_steps = notes
                .into_iter()
                .map(|note| {
                    piano_roll_time_to_step_delay(
                        start + note.start_offset as f64,
                        num_steps,
                    )
                    .0
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
    state: &Arc<SequencerState>,
    track: usize,
    ids: &[u64],
    clipboard: &PianoRollClipboard,
) -> usize {
    let mut notes = ids
        .iter()
        .filter_map(|&id| {
            let (step, voice_idx) = piano_roll_item_parts(id)?;
            let note = piano_roll_step_note_entries(state, track, step)
                .get(voice_idx)
                .cloned()?;
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
    state: &Arc<SequencerState>,
    track: usize,
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
        let mut step_notes = piano_roll_step_note_entries(state, track, step);
        step_notes.push(PianoRollNote {
            transpose: note.transpose,
            duration: note.duration,
            delay,
        });
        set_piano_roll_step_note_entries(state, track, step, &step_notes);
        if let Some(voice_idx) =
            piano_roll_find_note_index(state, track, step, note.transpose, note.duration, delay)
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
    state: &Arc<SequencerState>,
    track: usize,
    selection: &Arc<Mutex<HashSet<u64>>>,
    move_state: &Arc<Mutex<Option<PianoRollMoveState>>>,
    action: &Value,
) -> Result<String, String> {
    let clipboard = new_piano_roll_clipboard();
    apply_piano_roll_action_with_clipboard(state, track, selection, move_state, &clipboard, action)
}

pub(crate) fn apply_piano_roll_action_with_clipboard(
    state: &Arc<SequencerState>,
    track: usize,
    selection: &Arc<Mutex<HashSet<u64>>>,
    move_state: &Arc<Mutex<Option<PianoRollMoveState>>>,
    clipboard: &PianoRollClipboard,
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
                for (voice_idx, note) in piano_roll_step_note_entries(state, track, step)
                    .into_iter()
                    .enumerate()
                {
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
        "copy-items" => {
            *move_state.lock().unwrap() = None;
            let ids = parse_piano_roll_ids(action.get("ids"));
            let count = copy_piano_roll_items(state, track, &ids, clipboard);
            Ok(format!("copied {} note(s)", count))
        }
        "paste-items" => {
            *move_state.lock().unwrap() = None;
            let start = value_as_number(action.get("time")).unwrap_or(0.0);
            let count =
                paste_piano_roll_items(state, track, num_steps, start, selection, clipboard);
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
            let mut notes = piano_roll_step_note_entries(state, track, step);
            notes.push(PianoRollNote {
                transpose,
                duration,
                delay,
            });
            set_piano_roll_step_note_entries(state, track, step, &notes);
            let id = piano_roll_step_note_entries(state, track, step)
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
            let start = value_as_number(action.get("start")).unwrap_or(0.0) as f32;
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
                let resized = resize_piano_roll_items_absolute(
                    state,
                    track,
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
        let notes = piano_roll_step_note_entries(state, track, step)
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
        set_piano_roll_step_note_entries(state, track, step, &notes);
    }

    let mut next_ids = Vec::with_capacity(originals.len());
    for &(_, step, _, transpose, duration, delay) in &originals {
        let next_step = (step as isize + delta_time).clamp(0, (num_steps - 1) as isize) as usize;
        let lane = piano_roll_transpose_to_lane(transpose) as isize + delta_lane;
        let next_transpose = piano_roll_lane_to_transpose(lane.max(0) as usize);
        let mut notes = piano_roll_step_note_entries(state, track, next_step);
        notes.push(PianoRollNote {
            transpose: next_transpose,
            duration,
            delay,
        });
        set_piano_roll_step_note_entries(state, track, next_step, &notes);
        if let Some(next_voice_idx) =
            piano_roll_find_note_index(state, track, next_step, next_transpose, duration, delay)
        {
            next_ids.push(piano_roll_item_id(next_step, next_voice_idx));
        }
    }
    next_ids
}

fn resize_piano_roll_items_absolute(
    state: &Arc<SequencerState>,
    track: usize,
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
                let (step, voice_idx) = piano_roll_item_parts(id)?;
                let notes = piano_roll_step_note_entries(state, track, step);
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
            anchor_start: anchor_step as f32 + piano_roll_step_delay(state, track, anchor_step),
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
            piano_roll_step_note_entries(state, track, item.step)
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
        set_piano_roll_step_note_entries(state, track, step, &notes);
    }
    resized
}

fn move_piano_roll_items_absolute(
    state: &Arc<SequencerState>,
    track: usize,
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
        let mut notes = piano_roll_step_note_entries(state, track, item.step);
        if let Some(pos) = notes.iter().position(|note| {
            (note.transpose - item.transpose).abs() < f32::EPSILON
                && (note.delay - item.delay).abs() < f32::EPSILON
        }) {
            notes.remove(pos);
            set_piano_roll_step_note_entries(state, track, item.step, &notes);
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
        let mut notes = piano_roll_step_note_entries(state, track, next_step);
        notes.push(PianoRollNote {
            transpose: next_transpose,
            duration: item.duration,
            delay: next_delay,
        });
        set_piano_roll_step_note_entries(state, track, next_step, &notes);
        let next_voice_idx = piano_roll_find_note_index(
            state,
            track,
            next_step,
            next_transpose,
            item.duration,
            next_delay,
        )
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
