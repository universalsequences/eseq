/*!
Argument parsing and runtime-global helpers shared by the native
implementations.

`parse_step_arg` / `parse_slot_arg` / `parse_param_index_arg` and friends
convert raw `EValue` argument lists into validated indices and param targets
with consistent error messages, and `apply_acc_emit_overrides` folds
`acc-emit` keyword overrides into a `ResolvedStep`. Also provides the
fallback effect/instrument descriptor tables used when no live descriptors
are supplied, and `install_runtime_globals`, which (re)binds the current
track's effect/instrument descriptor globals into a `Runtime` before each
evaluation.
*/

use super::*;

pub(super) fn fallback_effect_descriptors(track_count: usize) -> Vec<Vec<EffectDescriptor>> {
    (0..track_count)
        .map(|_| EffectDescriptor::default_full_chain())
        .collect()
}

pub(super) fn fallback_instrument_descriptors(track_count: usize) -> Vec<EffectDescriptor> {
    (0..track_count)
        .map(|_| {
            let mut desc = EffectDescriptor::builtin_delay();
            for (idx, param) in desc.params.iter_mut().enumerate() {
                param.node_param_idx = idx as u32;
            }
            desc
        })
        .collect()
}

pub(super) fn shared_native_metadata(
    effect_descriptors: Vec<Vec<EffectDescriptor>>,
    instrument_descriptors: Vec<EffectDescriptor>,
) -> SharedSequencerNativeMetadata {
    Arc::new(Mutex::new(SequencerNativeMetadata {
        effect_descriptors,
        instrument_descriptors,
    }))
}

pub(super) fn install_runtime_globals(
    runtime: &mut Runtime,
    context: &SharedSequencerEvalContext,
    metadata: &SharedSequencerNativeMetadata,
    previous_globals: &[String],
) -> Vec<String> {
    for name in previous_globals {
        runtime.set_global_value(name, EValue::Nil);
    }

    let track = context.lock().map(|ctx| ctx.track).unwrap_or(0);
    let (effect_descriptors, instrument_descriptor) = metadata
        .lock()
        .ok()
        .map(|metadata| {
            (
                metadata
                    .effect_descriptors
                    .get(track)
                    .cloned()
                    .unwrap_or_default(),
                metadata.instrument_descriptors.get(track).cloned(),
            )
        })
        .unwrap_or_default();

    let mut installed = Vec::new();
    for (slot_idx, desc) in effect_descriptors.iter().enumerate() {
        let global_name = sanitize_symbol_name(&desc.name, true);
        if global_name.is_empty() {
            continue;
        }
        let mut fields: HashMap<String, Rc<RefCell<EValue>>> = HashMap::new();
        for (param_idx, param) in desc.params.iter().enumerate() {
            let field_name = sanitize_symbol_name(&param.name, false);
            if field_name.is_empty() {
                continue;
            }
            fields.insert(
                field_name,
                lisp_value(lisp_list(vec![
                    EValue::Number(slot_idx as f64),
                    EValue::Number(param_idx as f64),
                ])),
            );
        }
        runtime.set_global_value(&global_name, EValue::Map(fields));
        installed.push(global_name);
    }

    if let Some(desc) = instrument_descriptor {
        let global_name = sanitize_symbol_name(&desc.name, true);
        if !global_name.is_empty() {
            let mut fields: HashMap<String, Rc<RefCell<EValue>>> = HashMap::new();
            for (param_idx, param) in desc.params.iter().enumerate() {
                let field_name = sanitize_symbol_name(&param.name, false);
                if field_name.is_empty() {
                    continue;
                }
                fields.insert(
                    field_name,
                    lisp_value(lisp_list(vec![EValue::Number(param_idx as f64)])),
                );
            }
            runtime.set_global_value(&global_name, EValue::Map(fields));
            installed.push(global_name);
        }
    }

    installed
}

pub(super) fn parse_step_arg(args: &[EValue], idx: usize) -> Result<usize, String> {
    let Some(EValue::Number(step)) = args.get(idx) else {
        return Err("expected 0-based step index".to_string());
    };
    if *step < 0.0 {
        return Err("step index must be >= 0".to_string());
    }
    Ok(*step as usize)
}

pub(super) fn parse_slot_arg(args: &[EValue], idx: usize) -> Result<usize, String> {
    let Some(EValue::Number(slot)) = args.get(idx) else {
        return Err("expected 0-based slot index".to_string());
    };
    if *slot < 0.0 {
        return Err("slot index must be >= 0".to_string());
    }
    Ok(*slot as usize)
}

pub(super) fn parse_param_index_arg(args: &[EValue], idx: usize) -> Result<usize, String> {
    let Some(EValue::Number(param_idx)) = args.get(idx) else {
        return Err("expected 0-based parameter index".to_string());
    };
    if *param_idx < 0.0 {
        return Err("parameter index must be >= 0".to_string());
    }
    Ok(*param_idx as usize)
}

pub(super) fn parse_effect_param_ref_arg(value: &EValue) -> Result<(usize, usize), String> {
    let EValue::List(items) = value else {
        return Err("expected effect param ref or slot/param indices".to_string());
    };
    if items.len() != 2 {
        return Err("effect param ref must be a 2-item list".to_string());
    }
    let slot_idx = match &*items[0].borrow() {
        EValue::Number(slot_idx) if *slot_idx >= 0.0 => *slot_idx as usize,
        _ => return Err("effect param ref slot index must be >= 0".to_string()),
    };
    let param_idx = match &*items[1].borrow() {
        EValue::Number(param_idx) if *param_idx >= 0.0 => *param_idx as usize,
        _ => return Err("effect param ref param index must be >= 0".to_string()),
    };
    Ok((slot_idx, param_idx))
}

pub(super) fn parse_effect_param_target_arg(
    args: &[EValue],
    idx: usize,
) -> Result<(usize, usize, usize), String> {
    if let Some(value) = args.get(idx) {
        if matches!(value, EValue::List(_)) {
            let (slot_idx, param_idx) = parse_effect_param_ref_arg(value)?;
            return Ok((slot_idx, param_idx, idx + 1));
        }
    }
    Ok((
        parse_slot_arg(args, idx)?,
        parse_param_index_arg(args, idx + 1)?,
        idx + 2,
    ))
}

pub(super) fn parse_instrument_param_ref_arg(value: &EValue) -> Result<usize, String> {
    let EValue::List(items) = value else {
        return Err("expected instrument param ref or parameter index".to_string());
    };
    if items.len() != 1 {
        return Err("instrument param ref must be a 1-item list".to_string());
    }
    match &*items[0].borrow() {
        EValue::Number(param_idx) if *param_idx >= 0.0 => Ok(*param_idx as usize),
        _ => Err("instrument param ref index must be >= 0".to_string()),
    }
}

pub(super) fn parse_instrument_param_target_arg(
    args: &[EValue],
    idx: usize,
) -> Result<(usize, usize), String> {
    if let Some(value) = args.get(idx) {
        if matches!(value, EValue::List(_)) {
            return Ok((parse_instrument_param_ref_arg(value)?, idx + 1));
        }
    }
    Ok((parse_param_index_arg(args, idx)?, idx + 1))
}

pub(super) fn parse_value_arg(args: &[EValue], idx: usize, label: &str) -> Result<f32, String> {
    let Some(EValue::Number(value)) = args.get(idx) else {
        return Err(format!("expected {label} value"));
    };
    Ok(*value as f32)
}

pub(super) fn parse_normalized_arg(args: &[EValue], idx: usize, label: &str) -> Result<f32, String> {
    Ok(parse_value_arg(args, idx, label)?.clamp(0.0, 1.0))
}

pub(super) fn acc_emit_number(value: &EValue, label: &str) -> Result<f32, String> {
    match value {
        EValue::Number(value) => Ok(*value as f32),
        _ => Err(format!("acc-emit expected numeric {label}")),
    }
}

pub(super) fn apply_acc_emit_overrides(
    args: &[EValue],
    mut idx: usize,
    resolved: &mut ResolvedStep,
    chord: &mut Vec<f32>,
    chord_durations: &mut Vec<f32>,
) -> Result<Option<usize>, String> {
    let mut target_track = None;
    while idx < args.len() {
        let key = match &args[idx] {
            EValue::Keyword(name) | EValue::String(name) | EValue::Symbol(name) => {
                name.to_ascii_lowercase()
            }
            _ => return Err("acc-emit expects keyword/value override pairs".to_string()),
        };
        idx += 1;
        let Some(value) = args.get(idx) else {
            return Err(format!("acc-emit missing value for :{key}"));
        };
        match key.as_str() {
            "vel" | "velocity" => {
                resolved.velocity = acc_emit_number(value, "velocity")?.clamp(0.0, 1.0);
            }
            "transpose" | "trn" => {
                resolved.transpose = acc_emit_number(value, "transpose")?;
            }
            "note" => {
                resolved.transpose = acc_emit_number(value, "note")?;
                chord.clear();
                chord_durations.clear();
            }
            "duration" | "dur" => {
                resolved.duration = acc_emit_number(value, "duration")?.max(0.0);
            }
            "speed" | "spd" => {
                resolved.speed = acc_emit_number(value, "speed")?.max(0.0);
            }
            "pan" => {
                resolved.pan = acc_emit_number(value, "pan")?.clamp(-1.0, 1.0);
            }
            "chop" | "chp" => {
                resolved.chop = acc_emit_number(value, "chop")?.max(1.0);
            }
            "track" => {
                let track = acc_emit_number(value, "track")?;
                if track < 0.0 {
                    return Err("acc-emit :track must be >= 0".to_string());
                }
                target_track = Some(track as usize);
            }
            _ => return Err(format!("acc-emit unknown override :{key}")),
        }
        idx += 1;
    }
    Ok(target_track)
}
