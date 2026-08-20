/*!
Implementation bodies for the MIDI-FX event-scripting natives (`fx-*`).

MIDI FX are eseqlisp scripts that transform sequencer events as they pass
through the accumulator pipeline. Each `eval_*` function here implements one
native against the `SharedAccumulatorEvalContext` holding the event being
processed: suppress/emit (`fx-suppress`, `fx-emit`), arpeggiator helpers
(`fx-arp-note`, `fx-arp-emit`, `fx-arp-emit-directed`), timing (`fx-time`,
`fx-source-time`, `fx-phase-time`, `fx-phase-tick`), parameter and track
reads (`fx-param`, `midi-fx-param`, `fx-track`, `fx-velocity`), note spans
(`fx-note`, `fx-notes`, `fx-note-start`/`-end`), and persistent per-FX state
(`fx-state-get`/`-set`). Registration of these names lives in
`sequencer_natives`; descriptor loading lives in `scratch_runtime`.
*/

use super::super::*;

pub(in crate::lisp_host) fn eval_suppress_current_event(
    accumulator_eval: &SharedAccumulatorEvalContext,
    label: &str,
) -> Result<EValue, String> {
    let mut guard = accumulator_eval
        .lock()
        .map_err(|_| format!("failed to lock {label} eval context"))?;
    let Some(eval) = guard.as_mut() else {
        return Err(format!("{label} context not active"));
    };
    eval.suppressed = true;
    Ok(EValue::Bool(true))
}

pub(in crate::lisp_host) fn eval_emit_current_event(
    accumulator_eval: &SharedAccumulatorEvalContext,
    args: &[EValue],
    label: &str,
) -> Result<EValue, String> {
    let mut guard = accumulator_eval
        .lock()
        .map_err(|_| format!("failed to lock {label} eval context"))?;
    let Some(eval) = guard.as_mut() else {
        return Err(format!("{label} context not active"));
    };
    let (offset_beats, idx) = parse_acc_emit_offset(args, eval.step_beats, eval.num_steps)?;
    let mut resolved = eval.resolved;
    let mut chord = eval.chord.clone();
    let mut chord_durations = eval.chord_durations.clone();
    let chord_step_transpose = eval.chord_step_transpose;
    let target_track =
        apply_acc_emit_overrides(args, idx, &mut resolved, &mut chord, &mut chord_durations)?;
    eval.emitted.push(EmittedAccumulatorEvent {
        offset_beats,
        track: target_track,
        resolved,
        chord,
        chord_durations,
        chord_step_transpose,
        effect_params: eval.effect_params.clone(),
        instrument_params: eval.instrument_params.clone(),
    });
    Ok(EValue::Bool(true))
}

pub(in crate::lisp_host) fn eval_arp_count_current_event(
    accumulator_eval: &SharedAccumulatorEvalContext,
    args: &[EValue],
    label: &str,
) -> Result<EValue, String> {
    let guard = accumulator_eval
        .lock()
        .map_err(|_| format!("failed to lock {label} eval context"))?;
    let Some(eval) = guard.as_ref() else {
        return Err(format!("{label} context not active"));
    };
    let timebase = parse_timebase_arg(args, 0)?;
    let rate_beats = timebase.step_beats(eval.num_steps).max(0.0) as f32;
    Ok(EValue::Number(
        accumulator_arp_count(eval, rate_beats) as f64
    ))
}

pub(in crate::lisp_host) fn eval_arp_note_current_event(
    accumulator_eval: &SharedAccumulatorEvalContext,
    args: &[EValue],
    label: &str,
) -> Result<EValue, String> {
    let guard = accumulator_eval
        .lock()
        .map_err(|_| format!("failed to lock {label} eval context"))?;
    let Some(eval) = guard.as_ref() else {
        return Err(format!("{label} context not active"));
    };
    let timebase = parse_timebase_arg(args, 0)?;
    let Some(EValue::Number(tick)) = args.get(1) else {
        return Err("arp note helper expects numeric tick".to_string());
    };
    if *tick < 0.0 {
        return Ok(EValue::Nil);
    }
    let rate_beats = timebase.step_beats(eval.num_steps).max(0.0) as f32;
    Ok(accumulator_arp_note(eval, rate_beats, *tick as usize)
        .map(|note| EValue::Number(note as f64))
        .unwrap_or(EValue::Nil))
}

pub(in crate::lisp_host) fn eval_arp_emit_current_event(
    accumulator_eval: &SharedAccumulatorEvalContext,
    args: &[EValue],
    label: &str,
) -> Result<EValue, String> {
    let mut guard = accumulator_eval
        .lock()
        .map_err(|_| format!("failed to lock {label} eval context"))?;
    let Some(eval) = guard.as_mut() else {
        return Err(format!("{label} context not active"));
    };
    let timebase = parse_timebase_arg(args, 0)?;
    let Some(EValue::Number(tick)) = args.get(1) else {
        return Err("arp emit helper expects numeric tick".to_string());
    };
    if *tick < 0.0 {
        return Ok(EValue::Bool(false));
    }
    let rate_beats = timebase.step_beats(eval.num_steps).max(0.0) as f32;
    let Some(note) = accumulator_arp_note(eval, rate_beats, *tick as usize) else {
        return Ok(EValue::Bool(false));
    };

    let mut resolved = eval.resolved;
    resolved.transpose = note;
    if eval.step_beats > 0.0 {
        resolved.duration = (rate_beats / eval.step_beats).max(0.0);
    }
    let mut chord = Vec::new();
    let mut chord_durations = Vec::new();
    let target_track =
        apply_acc_emit_overrides(args, 2, &mut resolved, &mut chord, &mut chord_durations)?;
    eval.emitted.push(EmittedAccumulatorEvent {
        offset_beats: *tick as f32 * rate_beats,
        track: target_track,
        resolved,
        chord,
        chord_durations,
        chord_step_transpose: eval.chord_step_transpose,
        effect_params: eval.effect_params.clone(),
        instrument_params: eval.instrument_params.clone(),
    });
    Ok(EValue::Bool(true))
}

pub(in crate::lisp_host) fn eval_arp_emit_directed_current_event(
    accumulator_eval: &SharedAccumulatorEvalContext,
    args: &[EValue],
    label: &str,
) -> Result<EValue, String> {
    let mut guard = accumulator_eval
        .lock()
        .map_err(|_| format!("failed to lock {label} eval context"))?;
    let Some(eval) = guard.as_mut() else {
        return Err(format!("{label} context not active"));
    };
    let timebase = parse_timebase_arg(args, 0)?;
    let Some(EValue::Number(tick)) = args.get(1) else {
        return Err("directed arp emit expects numeric tick".to_string());
    };
    let Some(EValue::Number(direction)) = args.get(2) else {
        return Err("directed arp emit expects numeric direction".to_string());
    };
    if *tick < 0.0 {
        return Ok(EValue::Bool(false));
    }
    let rate_beats = timebase.step_beats(eval.num_steps).max(0.0) as f32;
    let Some(note) =
        accumulator_arp_note_directed(eval, rate_beats, *tick as usize, *direction as i32)
    else {
        return Ok(EValue::Bool(false));
    };

    let mut resolved = eval.resolved;
    resolved.transpose = note;
    if eval.step_beats > 0.0 {
        resolved.duration = (rate_beats / eval.step_beats).max(0.0);
    }
    let mut chord = Vec::new();
    let mut chord_durations = Vec::new();
    let target_track =
        apply_acc_emit_overrides(args, 3, &mut resolved, &mut chord, &mut chord_durations)?;
    eval.emitted.push(EmittedAccumulatorEvent {
        offset_beats: *tick as f32 * rate_beats,
        track: target_track,
        resolved,
        chord,
        chord_durations,
        chord_step_transpose: eval.chord_step_transpose,
        effect_params: eval.effect_params.clone(),
        instrument_params: eval.instrument_params.clone(),
    });
    Ok(EValue::Bool(true))
}

pub(in crate::lisp_host) fn eval_fx_time(
    accumulator_eval: &SharedAccumulatorEvalContext,
    args: &[EValue],
) -> Result<EValue, String> {
    let guard = accumulator_eval
        .lock()
        .map_err(|_| "failed to lock MIDI FX eval context".to_string())?;
    let Some(eval) = guard.as_ref() else {
        return Err("MIDI FX context not active".to_string());
    };
    let timebase = parse_timebase_arg(args, 0)?;
    let units = match args.get(1) {
        Some(EValue::Number(units)) => *units as f32,
        None => 1.0,
        _ => return Err("fx-time units must be numeric".to_string()),
    };
    Ok(EValue::Number(
        (timebase.step_beats(eval.num_steps).max(0.0) as f32 * units) as f64,
    ))
}

pub(in crate::lisp_host) fn eval_fx_source_time(
    accumulator_eval: &SharedAccumulatorEvalContext,
    args: &[EValue],
) -> Result<EValue, String> {
    let guard = accumulator_eval
        .lock()
        .map_err(|_| "failed to lock MIDI FX eval context".to_string())?;
    let Some(eval) = guard.as_ref() else {
        return Err("MIDI FX context not active".to_string());
    };
    let units = match args.first() {
        Some(EValue::Number(units)) => *units as f32,
        None => 1.0,
        _ => return Err("fx-source-time units must be numeric".to_string()),
    };
    Ok(EValue::Number((eval.step_beats * units) as f64))
}

pub(in crate::lisp_host) fn eval_fx_phase_time(accumulator_eval: &SharedAccumulatorEvalContext) -> Result<EValue, String> {
    let guard = accumulator_eval
        .lock()
        .map_err(|_| "failed to lock MIDI FX eval context".to_string())?;
    let Some(eval) = guard.as_ref() else {
        return Err("MIDI FX context not active".to_string());
    };
    Ok(EValue::Number(eval.arp_phase_beats.max(0.0) as f64))
}

pub(in crate::lisp_host) fn eval_fx_phase_tick(
    accumulator_eval: &SharedAccumulatorEvalContext,
    args: &[EValue],
) -> Result<EValue, String> {
    let guard = accumulator_eval
        .lock()
        .map_err(|_| "failed to lock MIDI FX eval context".to_string())?;
    let Some(eval) = guard.as_ref() else {
        return Err("MIDI FX context not active".to_string());
    };
    let timebase = parse_timebase_arg(args, 0)?;
    let rate_beats = timebase.step_beats(eval.num_steps).max(0.0) as f32;
    if rate_beats <= 0.0 {
        return Ok(EValue::Number(0.0));
    }
    Ok(EValue::Number(
        (eval.arp_phase_beats.max(0.0) / rate_beats).floor() as f64,
    ))
}

pub(in crate::lisp_host) fn parse_midi_fx_param_ref(eval: &AccumulatorEvalContext, value: &EValue) -> Result<usize, String> {
    match value {
        EValue::Number(index) if *index >= 0.0 => Ok(*index as usize),
        EValue::String(name) | EValue::Keyword(name) | EValue::Symbol(name) => eval
            .midi_fx_param_names
            .iter()
            .position(|param| param.eq_ignore_ascii_case(name))
            .ok_or_else(|| format!("unknown MIDI FX param '{name}'")),
        _ => Err("MIDI FX param ref must be a name or index".to_string()),
    }
}

pub(in crate::lisp_host) fn eval_midi_fx_param(
    accumulator_eval: &SharedAccumulatorEvalContext,
    args: &[EValue],
) -> Result<EValue, String> {
    let Some(param_ref) = args.first() else {
        return Err("fx-param expects a name or index".to_string());
    };
    let guard = accumulator_eval
        .lock()
        .map_err(|_| "failed to lock MIDI FX eval context".to_string())?;
    let Some(eval) = guard.as_ref() else {
        return Err("MIDI FX context not active".to_string());
    };
    let param_idx = parse_midi_fx_param_ref(eval, param_ref)?;
    if param_idx >= eval.midi_fx_slot.num_params as usize {
        return Err("MIDI FX param index out of range".to_string());
    }
    let value = eval
        .midi_fx_slot
        .plocks
        .get(eval.step_index)
        .and_then(|step| step.get(param_idx))
        .copied()
        .flatten()
        .unwrap_or_else(|| {
            eval.midi_fx_slot
                .defaults
                .get(param_idx)
                .copied()
                .unwrap_or(0.0)
        });
    Ok(EValue::Number(value as f64))
}

pub(in crate::lisp_host) fn eval_midi_fx_track(accumulator_eval: &SharedAccumulatorEvalContext) -> Result<EValue, String> {
    let guard = accumulator_eval
        .lock()
        .map_err(|_| "failed to lock MIDI FX eval context".to_string())?;
    let Some(eval) = guard.as_ref() else {
        return Err("MIDI FX context not active".to_string());
    };
    let Some((track, _)) = eval.midi_fx_scope.as_ref() else {
        return Err("fx-track is only available inside def-midi-fx".to_string());
    };
    Ok(EValue::Number(*track as f64))
}

pub(in crate::lisp_host) fn eval_midi_fx_velocity(
    accumulator_eval: &SharedAccumulatorEvalContext,
) -> Result<EValue, String> {
    let guard = accumulator_eval
        .lock()
        .map_err(|_| "failed to lock MIDI FX eval context".to_string())?;
    let Some(eval) = guard.as_ref() else {
        return Err("MIDI FX context not active".to_string());
    };
    Ok(EValue::Number(eval.resolved.velocity as f64))
}

pub(in crate::lisp_host) enum FxNoteField {
    Transpose,
    Start,
    End,
}

pub(in crate::lisp_host) fn eval_note_span_field(
    accumulator_eval: &SharedAccumulatorEvalContext,
    args: &[EValue],
    field: FxNoteField,
) -> Result<EValue, String> {
    let Some(EValue::Number(index)) = args.first() else {
        return Err("note helper expects numeric index".to_string());
    };
    if *index < 0.0 {
        return Ok(EValue::Nil);
    }
    let index = *index as usize;
    let guard = accumulator_eval
        .lock()
        .map_err(|_| "failed to lock MIDI FX eval context".to_string())?;
    let Some(eval) = guard.as_ref() else {
        return Err("MIDI FX context not active".to_string());
    };
    if let Some(spans) = eval.note_spans.as_ref() {
        let Some(span) = spans.get(index) else {
            return Ok(EValue::Nil);
        };
        return Ok(EValue::Number(match field {
            FxNoteField::Transpose => span.transpose as f64,
            FxNoteField::Start => span.start_beats as f64,
            FxNoteField::End => span.end_beats as f64,
        }));
    }
    let notes = accumulator_chord_notes(eval);
    let Some(note) = notes.get(index) else {
        return Ok(EValue::Nil);
    };
    Ok(EValue::Number(match field {
        FxNoteField::Transpose => note.transpose as f64,
        FxNoteField::Start => 0.0,
        FxNoteField::End => (note.duration_steps * eval.step_beats) as f64,
    }))
}

pub(in crate::lisp_host) fn eval_note_spans_as_list(
    accumulator_eval: &SharedAccumulatorEvalContext,
) -> Result<EValue, String> {
    let guard = accumulator_eval
        .lock()
        .map_err(|_| "failed to lock MIDI FX eval context".to_string())?;
    let Some(eval) = guard.as_ref() else {
        return Err("MIDI FX context not active".to_string());
    };
    let notes = if let Some(spans) = eval.note_spans.as_ref() {
        spans
            .iter()
            .map(|span| (span.transpose, span.start_beats, span.end_beats))
            .collect::<Vec<_>>()
    } else {
        accumulator_chord_notes(eval)
            .into_iter()
            .map(|note| (note.transpose, 0.0, note.duration_steps * eval.step_beats))
            .collect::<Vec<_>>()
    };
    Ok(lisp_list(
        notes
            .into_iter()
            .map(|(transpose, start_beats, end_beats)| {
                let mut map = HashMap::new();
                map.insert("note".to_string(), lisp_number(transpose as f64));
                map.insert("start".to_string(), lisp_number(start_beats as f64));
                map.insert("end".to_string(), lisp_number(end_beats as f64));
                EValue::Map(map)
            })
            .collect(),
    ))
}

pub(in crate::lisp_host) fn midi_fx_state_user_key(value: &EValue) -> Result<String, String> {
    match value {
        EValue::String(key) | EValue::Keyword(key) => Ok(key.clone()),
        _ => Err("MIDI FX state key must be a string or keyword".to_string()),
    }
}

pub(in crate::lisp_host) fn current_midi_fx_state_key(
    accumulator_eval: &SharedAccumulatorEvalContext,
    user_key: &str,
) -> Result<String, String> {
    let guard = accumulator_eval
        .lock()
        .map_err(|_| "failed to lock MIDI FX eval context".to_string())?;
    let Some(eval) = guard.as_ref() else {
        return Err("MIDI FX context not active".to_string());
    };
    let Some((track, fx_name)) = eval.midi_fx_scope.as_ref() else {
        return Err("MIDI FX state is only available inside def-midi-fx".to_string());
    };
    Ok(format!("{track}\u{0}{fx_name}\u{0}{user_key}"))
}

pub(in crate::lisp_host) fn eval_midi_fx_state_get(
    accumulator_eval: &SharedAccumulatorEvalContext,
    midi_fx_state: &SharedMidiFxState,
    args: &[EValue],
) -> Result<EValue, String> {
    let Some(key_value) = args.first() else {
        return Err("fx-state-get expects a key".to_string());
    };
    let user_key = midi_fx_state_user_key(key_value)?;
    let key = current_midi_fx_state_key(accumulator_eval, &user_key)?;
    Ok(midi_fx_state
        .lock()
        .map_err(|_| "failed to lock MIDI FX state".to_string())?
        .get(&key)
        .cloned()
        .unwrap_or_else(|| args.get(1).cloned().unwrap_or(EValue::Nil)))
}

pub(in crate::lisp_host) fn eval_midi_fx_state_set(
    accumulator_eval: &SharedAccumulatorEvalContext,
    midi_fx_state: &SharedMidiFxState,
    args: &[EValue],
) -> Result<EValue, String> {
    let Some(key_value) = args.first() else {
        return Err("fx-state-set expects a key and value".to_string());
    };
    let Some(value) = args.get(1).cloned() else {
        return Err("fx-state-set expects a value".to_string());
    };
    let user_key = midi_fx_state_user_key(key_value)?;
    let key = current_midi_fx_state_key(accumulator_eval, &user_key)?;
    midi_fx_state
        .lock()
        .map_err(|_| "failed to lock MIDI FX state".to_string())?
        .insert(key, value.clone());
    Ok(value)
}

#[derive(Clone, Copy)]
pub(in crate::lisp_host) struct AccumulatorChordNote {
    pub(in crate::lisp_host) transpose: f32,
    pub(in crate::lisp_host) duration_steps: f32,
}

pub(in crate::lisp_host) fn accumulator_chord_notes(eval: &AccumulatorEvalContext) -> Vec<AccumulatorChordNote> {
    if eval.chord.is_empty() {
        return vec![AccumulatorChordNote {
            transpose: eval.resolved.transpose,
            duration_steps: eval.resolved.duration.max(0.0),
        }];
    }
    eval.chord
        .iter()
        .enumerate()
        .map(|(idx, note)| AccumulatorChordNote {
            transpose: *note,
            duration_steps: eval
                .chord_durations
                .get(idx)
                .copied()
                .filter(|duration| *duration > 0.0)
                .unwrap_or(eval.resolved.duration)
                .max(0.0),
        })
        .collect()
}

pub(in crate::lisp_host) fn accumulator_arp_count(eval: &AccumulatorEvalContext, rate_beats: f32) -> usize {
    if rate_beats <= 0.0 {
        return 0;
    }
    if let Some(note_spans) = eval.note_spans.as_ref() {
        let max_end = note_spans
            .iter()
            .map(|note| note.end_beats)
            .fold(0.0_f32, f32::max);
        return (max_end / rate_beats).ceil().max(0.0) as usize;
    }
    let notes = accumulator_chord_notes(eval);
    if notes.is_empty() || eval.step_beats <= 0.0 {
        return 0;
    }
    let max_duration_beats = notes
        .iter()
        .map(|note| note.duration_steps * eval.step_beats)
        .fold(0.0_f32, f32::max);
    (max_duration_beats / rate_beats).ceil().max(0.0) as usize
}

pub(in crate::lisp_host) fn accumulator_arp_note(
    eval: &AccumulatorEvalContext,
    rate_beats: f32,
    tick: usize,
) -> Option<f32> {
    accumulator_arp_note_directed(eval, rate_beats, tick, 0)
}

pub(in crate::lisp_host) fn directed_note_index(tick: usize, len: usize, direction: i32) -> usize {
    if len <= 1 {
        return 0;
    }
    match direction {
        1 => len - 1 - (tick % len),
        2 => {
            let period = len * 2 - 2;
            let pos = tick % period;
            if pos < len {
                pos
            } else {
                period - pos
            }
        }
        3 => {
            let mut x = tick as u64;
            x ^= x >> 12;
            x ^= x << 25;
            x ^= x >> 27;
            ((x.wrapping_mul(0x2545F4914F6CDD1D) >> 32) as usize) % len
        }
        _ => tick % len,
    }
}

pub(in crate::lisp_host) fn accumulator_arp_note_directed(
    eval: &AccumulatorEvalContext,
    rate_beats: f32,
    tick: usize,
    direction: i32,
) -> Option<f32> {
    if rate_beats <= 0.0 {
        return None;
    }
    let phase_tick = (eval.arp_phase_beats.max(0.0) / rate_beats).floor() as usize;
    let elapsed = tick as f32 * rate_beats;
    if let Some(note_spans) = eval.note_spans.as_ref() {
        let phased_tick = tick.saturating_add(phase_tick);
        let active = note_spans
            .iter()
            .filter(|note| {
                elapsed >= note.start_beats - f32::EPSILON
                    && elapsed < note.end_beats - f32::EPSILON
            })
            .collect::<Vec<_>>();
        if active.is_empty() {
            return None;
        }
        return Some(active[directed_note_index(phased_tick, active.len(), direction)].transpose);
    }
    let notes = accumulator_chord_notes(eval);
    if notes.is_empty() || eval.step_beats <= 0.0 {
        return None;
    }
    let note_idx = directed_note_index(tick.saturating_add(phase_tick), notes.len(), direction);
    let duration_beats = notes[note_idx].duration_steps * eval.step_beats;
    if elapsed < duration_beats - f32::EPSILON {
        Some(notes[note_idx].transpose)
    } else {
        None
    }
}

pub(in crate::lisp_host) fn parse_acc_emit_offset(
    args: &[EValue],
    default_step_beats: f32,
    num_steps: usize,
) -> Result<(f32, usize), String> {
    let Some(first) = args.first() else {
        return Err("acc-emit expects an offset".to_string());
    };
    match first {
        EValue::Number(offset) => Ok((*offset as f32 * default_step_beats, 1)),
        EValue::Keyword(_) | EValue::String(_) => {
            if matches!(first, EValue::Keyword(name) | EValue::String(name) if name == "beats") {
                let Some(EValue::Number(offset)) = args.get(1) else {
                    return Err("acc-emit :beats expects numeric offset".to_string());
                };
                return Ok((*offset as f32, 2));
            }
            let timebase = parse_timebase_arg(args, 0)?;
            let Some(EValue::Number(offset)) = args.get(1) else {
                return Err("acc-emit explicit timebase expects numeric offset".to_string());
            };
            Ok((
                *offset as f32 * timebase.step_beats(num_steps).max(0.0) as f32,
                2,
            ))
        }
        _ => Err("acc-emit expects numeric offset or timebase keyword".to_string()),
    }
}

pub(in crate::lisp_host) fn apply_step_param_set(resolved: &mut ResolvedStep, param: StepParam, value: f32) {
    match param {
        StepParam::Duration => resolved.duration = value.max(0.0),
        StepParam::Velocity => resolved.velocity = value.clamp(0.0, 1.0),
        StepParam::Speed => resolved.speed = value.max(0.0),
        StepParam::AuxA | StepParam::AuxB | StepParam::Sync | StepParam::Delay => {}
        StepParam::Transpose => resolved.transpose = value,
        StepParam::Pan => resolved.pan = value.clamp(-1.0, 1.0),
        StepParam::Chop => resolved.chop = value.max(1.0),
    }
}

pub(in crate::lisp_host) fn apply_step_param_add(resolved: &mut ResolvedStep, param: StepParam, delta: f32) {
    match param {
        StepParam::Duration => resolved.duration = (resolved.duration + delta).max(0.0),
        StepParam::Velocity => resolved.velocity = (resolved.velocity + delta).clamp(0.0, 1.0),
        StepParam::Speed => resolved.speed = (resolved.speed + delta).max(0.0),
        StepParam::AuxA | StepParam::AuxB | StepParam::Sync | StepParam::Delay => {}
        StepParam::Transpose => resolved.transpose += delta,
        StepParam::Pan => resolved.pan = (resolved.pan + delta).clamp(-1.0, 1.0),
        StepParam::Chop => resolved.chop = (resolved.chop + delta).max(1.0),
    }
}

pub(in crate::lisp_host) fn apply_step_param_scale(resolved: &mut ResolvedStep, param: StepParam, factor: f32) {
    match param {
        StepParam::Duration => resolved.duration = (resolved.duration * factor).max(0.0),
        StepParam::Velocity => resolved.velocity = (resolved.velocity * factor).clamp(0.0, 1.0),
        StepParam::Speed => resolved.speed = (resolved.speed * factor).max(0.0),
        StepParam::AuxA | StepParam::AuxB | StepParam::Sync | StepParam::Delay => {}
        StepParam::Transpose => resolved.transpose *= factor,
        StepParam::Pan => resolved.pan = (resolved.pan * factor).clamp(-1.0, 1.0),
        StepParam::Chop => resolved.chop = (resolved.chop * factor).max(1.0),
    }
}

pub(in crate::lisp_host) fn accumulator_effect_param_desc(
    metadata: &SharedSequencerNativeMetadata,
    track_idx: usize,
    slot_idx: usize,
    param_idx: usize,
) -> Result<EffectDescriptorParamSnapshot, String> {
    metadata
        .lock()
        .ok()
        .and_then(|metadata| metadata.effect_descriptors.get(track_idx).cloned())
        .as_ref()
        .and_then(|slots| slots.get(slot_idx))
        .and_then(|desc| desc.params.get(param_idx))
        .cloned()
        .map(EffectDescriptorParamSnapshot::from)
        .ok_or_else(|| "effect descriptor missing for parameter".to_string())
}

pub(in crate::lisp_host) fn accumulator_instrument_param_desc(
    metadata: &SharedSequencerNativeMetadata,
    track_idx: usize,
    param_idx: usize,
) -> Result<EffectDescriptorParamSnapshot, String> {
    metadata
        .lock()
        .ok()
        .and_then(|metadata| metadata.instrument_descriptors.get(track_idx).cloned())
        .as_ref()
        .and_then(|desc| desc.params.get(param_idx))
        .cloned()
        .map(EffectDescriptorParamSnapshot::from)
        .ok_or_else(|| "instrument descriptor missing for parameter".to_string())
}

#[derive(Clone)]
pub(in crate::lisp_host) struct EffectDescriptorParamSnapshot {
    min: f32,
    max: f32,
    default: f32,
    kind: crate::effects::ParamKind,
    scaling: crate::effects::ParamScaling,
}

impl From<crate::effects::ParamDescriptor> for EffectDescriptorParamSnapshot {
    fn from(value: crate::effects::ParamDescriptor) -> Self {
        Self {
            min: value.min,
            max: value.max,
            default: value.default,
            kind: value.kind,
            scaling: value.scaling,
        }
    }
}

impl EffectDescriptorParamSnapshot {
    fn clamp(&self, value: f32) -> f32 {
        value.clamp(self.min, self.max)
    }

    fn normalize(&self, value: f32) -> f32 {
        let value = self.clamp(value);
        let range = self.max - self.min;
        if range <= 0.0 {
            return 0.0;
        }
        match self.scaling {
            crate::effects::ParamScaling::Linear => ((value - self.min) / range).clamp(0.0, 1.0),
            crate::effects::ParamScaling::Exponential => {
                if self.min <= 0.0 || self.max <= 0.0 {
                    ((value - self.min) / range).clamp(0.0, 1.0)
                } else {
                    let log_min = self.min.ln();
                    let log_max = self.max.ln();
                    let log_range = log_max - log_min;
                    if log_range <= 0.0 {
                        0.0
                    } else {
                        ((value.max(self.min).ln() - log_min) / log_range).clamp(0.0, 1.0)
                    }
                }
            }
        }
    }

    fn denormalize(&self, normalized: f32) -> f32 {
        let normalized = normalized.clamp(0.0, 1.0);
        match &self.kind {
            crate::effects::ParamKind::Boolean => {
                if normalized >= 0.5 {
                    1.0
                } else {
                    0.0
                }
            }
            crate::effects::ParamKind::Enum { .. } => {
                let range = self.max - self.min;
                if range <= 0.0 {
                    self.min
                } else {
                    (self.min + normalized * range)
                        .round()
                        .clamp(self.min, self.max)
                }
            }
            crate::effects::ParamKind::Continuous { .. } => match self.scaling {
                crate::effects::ParamScaling::Linear => {
                    self.min + normalized * (self.max - self.min)
                }
                crate::effects::ParamScaling::Exponential => {
                    if self.min <= 0.0 || self.max <= 0.0 {
                        self.min + normalized * (self.max - self.min)
                    } else {
                        let log_min = self.min.ln();
                        let log_max = self.max.ln();
                        (log_min + normalized * (log_max - log_min)).exp()
                    }
                }
            },
        }
    }
}

pub(in crate::lisp_host) fn effect_param_ids(
    eval: &mut AccumulatorEvalContext,
    slot_idx: usize,
    param_idx: usize,
) -> Result<(u64, u64), String> {
    let slot = eval
        .effect_slots
        .get(slot_idx)
        .ok_or_else(|| "effect slot out of range".to_string())?;
    if slot.node_id == 0 {
        return Err("effect slot is empty".to_string());
    }
    let num_params = slot.num_params as usize;
    if param_idx >= num_params {
        return Err("effect param index out of range".to_string());
    }
    let idx = slot
        .node_param_idx(param_idx)
        .ok_or_else(|| "effect param index unresolved".to_string())? as u64;
    if idx == u32::MAX as u64 {
        return Err("effect param index unresolved".to_string());
    }
    Ok((slot.node_id as u64, idx))
}

pub(in crate::lisp_host) fn current_effect_param_raw(
    eval: &AccumulatorEvalContext,
    slot_idx: usize,
    param_idx: usize,
    desc: &EffectDescriptorParamSnapshot,
) -> Result<f32, String> {
    let slot = eval
        .effect_slots
        .get(slot_idx)
        .ok_or_else(|| "effect slot out of range".to_string())?;
    if slot.node_id == 0 {
        return Err("effect slot is empty".to_string());
    }
    let num_params = slot.num_params as usize;
    if param_idx >= num_params {
        return Err("effect param index out of range".to_string());
    }
    Ok(eval
        .effect_params
        .iter()
        .find(|param| {
            param.logical_id == slot.node_id as u64
                && Some(param.idx) == slot.node_param_idx(param_idx).map(u64::from)
        })
        .map(|param| param.value)
        .unwrap_or(desc.default))
}

pub(in crate::lisp_host) fn set_effect_param_raw(
    eval: &mut AccumulatorEvalContext,
    slot_idx: usize,
    param_idx: usize,
    value: f32,
    desc: &EffectDescriptorParamSnapshot,
) -> Result<(), String> {
    let (logical_id, idx) = effect_param_ids(eval, slot_idx, param_idx)?;
    let value = desc.clamp(value);
    if let Some(existing) = eval
        .effect_params
        .iter_mut()
        .find(|param| param.logical_id == logical_id && param.idx == idx)
    {
        existing.value = value;
    } else {
        eval.effect_params.push(ScheduledEffectParam {
            logical_id,
            idx,
            value,
        });
    }
    Ok(())
}

pub(in crate::lisp_host) fn set_effect_param_normalized(
    eval: &mut AccumulatorEvalContext,
    slot_idx: usize,
    param_idx: usize,
    normalized: f32,
    desc: &EffectDescriptorParamSnapshot,
) -> Result<(), String> {
    set_effect_param_raw(
        eval,
        slot_idx,
        param_idx,
        desc.denormalize(normalized),
        desc,
    )
}

pub(in crate::lisp_host) fn add_effect_param_raw(
    eval: &mut AccumulatorEvalContext,
    slot_idx: usize,
    param_idx: usize,
    delta: f32,
    desc: &EffectDescriptorParamSnapshot,
) -> Result<(), String> {
    let current = current_effect_param_raw(eval, slot_idx, param_idx, desc)?;
    set_effect_param_raw(eval, slot_idx, param_idx, current + delta, desc)
}

pub(in crate::lisp_host) fn add_effect_param_normalized(
    eval: &mut AccumulatorEvalContext,
    slot_idx: usize,
    param_idx: usize,
    normalized_delta: f32,
    desc: &EffectDescriptorParamSnapshot,
) -> Result<(), String> {
    let current = current_effect_param_raw(eval, slot_idx, param_idx, desc)?;
    let next = (desc.normalize(current) + normalized_delta).clamp(0.0, 1.0);
    set_effect_param_normalized(eval, slot_idx, param_idx, next, desc)
}

pub(in crate::lisp_host) fn instrument_param_target_and_idx(
    slot: &EffectSlotSnapshot,
    param_idx: usize,
) -> Result<(ScheduledInstrumentParamTarget, u64, u32), String> {
    let num_params = slot.num_params as usize;
    if param_idx >= num_params {
        return Err("instrument param index out of range".to_string());
    }
    let raw_idx = slot
        .node_param_idx(param_idx)
        .ok_or_else(|| "instrument param index unresolved".to_string())?;
    let span = slot
        .param_node_spans
        .get(param_idx)
        .copied()
        .unwrap_or(1)
        .max(1);
    let (target, idx) = if raw_idx >= crate::instruments::voice_modulator::MOD_PARAM_BASE {
        (
            ScheduledInstrumentParamTarget::Modulator,
            (raw_idx - crate::instruments::voice_modulator::MOD_PARAM_BASE) as u64,
        )
    } else {
        (ScheduledInstrumentParamTarget::Synth, raw_idx as u64)
    };
    Ok((target, idx, span))
}

pub(in crate::lisp_host) fn current_instrument_param_raw(
    eval: &AccumulatorEvalContext,
    param_idx: usize,
    desc: &EffectDescriptorParamSnapshot,
) -> Result<f32, String> {
    let (target, idx, _) = instrument_param_target_and_idx(&eval.instrument_slot, param_idx)?;
    Ok(eval
        .instrument_params
        .iter()
        .find(|param| param.target == target && param.idx == idx)
        .map(|param| param.value)
        .unwrap_or(desc.default))
}

pub(in crate::lisp_host) fn set_instrument_param_raw(
    eval: &mut AccumulatorEvalContext,
    param_idx: usize,
    value: f32,
    desc: &EffectDescriptorParamSnapshot,
) -> Result<(), String> {
    let (target, idx, span) = instrument_param_target_and_idx(&eval.instrument_slot, param_idx)?;
    let value = desc.clamp(value);
    if let Some(existing) = eval
        .instrument_params
        .iter_mut()
        .find(|param| param.target == target && param.idx == idx)
    {
        existing.span = span;
        existing.value = value;
    } else {
        eval.instrument_params.push(ScheduledInstrumentParam {
            target,
            idx,
            span,
            value,
        });
    }
    Ok(())
}

pub(in crate::lisp_host) fn set_instrument_param_normalized(
    eval: &mut AccumulatorEvalContext,
    param_idx: usize,
    normalized: f32,
    desc: &EffectDescriptorParamSnapshot,
) -> Result<(), String> {
    set_instrument_param_raw(eval, param_idx, desc.denormalize(normalized), desc)
}

pub(in crate::lisp_host) fn add_instrument_param_raw(
    eval: &mut AccumulatorEvalContext,
    param_idx: usize,
    delta: f32,
    desc: &EffectDescriptorParamSnapshot,
) -> Result<(), String> {
    let current = current_instrument_param_raw(eval, param_idx, desc)?;
    set_instrument_param_raw(eval, param_idx, current + delta, desc)
}

pub(in crate::lisp_host) fn add_instrument_param_normalized(
    eval: &mut AccumulatorEvalContext,
    param_idx: usize,
    normalized_delta: f32,
    desc: &EffectDescriptorParamSnapshot,
) -> Result<(), String> {
    let current = current_instrument_param_raw(eval, param_idx, desc)?;
    let next = (desc.normalize(current) + normalized_delta).clamp(0.0, 1.0);
    set_instrument_param_normalized(eval, param_idx, next, desc)
}

pub(in crate::lisp_host) fn parse_step_param_arg(args: &[EValue], idx: usize) -> Result<StepParam, String> {
    let Some(value) = args.get(idx) else {
        return Err("expected step param".to_string());
    };
    match value {
        EValue::Keyword(name) | EValue::String(name) | EValue::Symbol(name) => {
            let normalized = name.to_ascii_lowercase();
            match normalized.as_str() {
                "duration" | "dur" => Ok(StepParam::Duration),
                "velocity" | "vel" => Ok(StepParam::Velocity),
                "speed" | "spd" => Ok(StepParam::Speed),
                "auxa" | "aux-a" | "aux_a" | "axa" => Ok(StepParam::AuxA),
                "auxb" | "aux-b" | "aux_b" | "axb" => Ok(StepParam::AuxB),
                "transpose" | "trn" => Ok(StepParam::Transpose),
                "pan" => Ok(StepParam::Pan),
                "chop" | "chp" => Ok(StepParam::Chop),
                "sync" | "syn" => Ok(StepParam::Sync),
                "delay" | "dly" => Ok(StepParam::Delay),
                _ => Err("unknown step param".to_string()),
            }
        }
        _ => Err("expected step param keyword/string".to_string()),
    }
}
