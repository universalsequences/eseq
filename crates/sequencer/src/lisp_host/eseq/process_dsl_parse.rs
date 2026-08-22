/*!
Parsing layer for the process/channel DSL (`def-process` and friends).

Turns the s-expression surface of a process definition into the plain data
structures in `crate::process`: `parse_process_def` builds a `ProcessDef`
(inlets, fields, fold body), `parse_process_inlets` and
`parse_process_inlet_kind_and_range` handle inlet declarations, and the
field helpers normalize scalar/gate/pitch field values. Also owns the
ratchet-event value conversions and `register_process_ratchet_event_natives`
(the small read/write natives a ratchet callback uses). The full native
surface (`process`, `start`, `connect!`, ...) is registered in the sibling
`process_natives`.
*/

use super::super::*;

pub(in crate::lisp_host) fn process_symbol_name(value: &EValue) -> Result<String, String> {
    match value {
        EValue::String(name) | EValue::Symbol(name) | EValue::Keyword(name) => Ok(name
            .trim_start_matches(':')
            .trim_start_matches('@')
            .to_string()),
        _ => Err("expected symbol/string name".to_string()),
    }
}
pub(in crate::lisp_host) fn process_key_arg(value: Option<&EValue>, native: &str) -> Result<String, String> {
    process_symbol_name(value.ok_or_else(|| format!("{native} expects a key"))?)
}

pub(in crate::lisp_host) fn process_number_arg(value: Option<&EValue>, native: &str) -> Result<f64, String> {
    match value {
        Some(EValue::Number(value)) => Ok(*value),
        _ => Err(format!("{native} expects a number")),
    }
}

pub(in crate::lisp_host) fn process_field_domain(value: &EValue) -> Result<String, String> {
    let EValue::Map(map) = value else {
        return Err("expected a typed field value".to_string());
    };
    let domain = map
        .get("field-domain")
        .ok_or_else(|| "field value is missing field-domain".to_string())?
        .borrow();
    process_symbol_name(&domain)
}

pub(in crate::lisp_host) fn process_field_cell(value: &EValue, key: &str) -> Result<EValue, String> {
    let EValue::Map(map) = value else {
        return Err("expected a typed field value".to_string());
    };
    map.get(key)
        .map(|value| value.borrow().clone())
        .ok_or_else(|| format!("field value is missing {key}"))
}

pub(in crate::lisp_host) fn process_scalar_field(value: f64) -> Result<EValue, String> {
    if !value.is_finite() {
        return Err("scalar field value must be finite".to_string());
    }
    Ok(process_map([
        ("field-domain", EValue::Keyword("scalar".to_string())),
        ("value", EValue::Number(value)),
    ]))
}

pub(in crate::lisp_host) fn process_gate_field(value: bool) -> EValue {
    process_map([
        ("field-domain", EValue::Keyword("gate".to_string())),
        ("value", EValue::Bool(value)),
    ])
}

pub(in crate::lisp_host) fn normalize_process_field(value: &EValue) -> Result<EValue, String> {
    match value {
        EValue::Number(value) => process_scalar_field(*value),
        EValue::Bool(value) => Ok(process_gate_field(*value)),
        EValue::Map(_) => match process_field_domain(value)?.as_str() {
            "scalar" => {
                let scalar =
                    process_number_arg(Some(&process_field_cell(value, "value")?), "scalar field")?;
                process_scalar_field(scalar)
            }
            "gate" => match process_field_cell(value, "value")? {
                EValue::Bool(value) => Ok(process_gate_field(value)),
                _ => Err("gate field value must be boolean".to_string()),
            },
            "pitch-field" => {
                let pitches = process_field_cell(value, "pitches")?;
                let EValue::List(items) = &pitches else {
                    return Err("pitch-field pitches must be a list".to_string());
                };
                if items.is_empty() {
                    return Err("pitch-field requires at least one pitch".to_string());
                }
                for pitch in items {
                    let pitch = pitch.borrow();
                    let pitch = process_number_arg(Some(&pitch), "pitch-field")?;
                    if !pitch.is_finite() {
                        return Err("pitch-field pitches must be finite".to_string());
                    }
                }
                let weight = process_number_arg(
                    Some(&process_field_cell(value, "weight")?),
                    "pitch-field weight",
                )?;
                if !weight.is_finite() || !(0.0..=1.0).contains(&weight) {
                    return Err("pitch-field weight must be between 0 and 1".to_string());
                }
                Ok(value.clone())
            }
            domain => Err(format!("unknown field domain :{domain}")),
        },
        _ => Err("suggest expects a number, boolean, or typed field value".to_string()),
    }
}

pub(in crate::lisp_host) fn process_target_write_args(
    args: &[EValue],
    native: &str,
) -> Result<(Option<String>, f32), String> {
    match args {
        [EValue::Number(value)] => Ok((None, *value as f32)),
        [port, EValue::Number(value)] => Ok((Some(process_symbol_name(port)?), *value as f32)),
        _ => Err(format!("{native} expects (value) or (:port value)")),
    }
}

pub(in crate::lisp_host) fn ensure_process_run_scope(ctx: &ProcessEvalContext, native: &str) -> Result<(), String> {
    if ctx.scope == ProcessEvalScope::Run {
        Ok(())
    } else {
        Err(format!(
            "{native} cannot be used while shaping a ratchet event"
        ))
    }
}

pub(in crate::lisp_host) fn process_value_is_callable(value: &EValue) -> bool {
    matches!(
        value,
        EValue::Closure(_, _)
            | EValue::Function(_)
            | EValue::NativeFunction(_)
            | EValue::HostHandle { .. }
    )
}

pub(in crate::lisp_host) fn parse_process_ratchet_args(
    args: &[EValue],
) -> Result<
    (
        u32,
        crate::process::ProcessRatchetMode,
        Option<f32>,
        Option<EValue>,
    ),
    String,
> {
    if args.is_empty() {
        return Err("ratchet! requires keyword arguments".to_string());
    }
    if args.len() % 2 != 0 {
        return Err("ratchet! expects keyword/value pairs".to_string());
    }
    let mut times = None;
    let mut mode = crate::process::ProcessRatchetMode::Subdivide;
    let mut span_beats = None;
    let mut shape = None;
    let mut idx = 0;
    while idx < args.len() {
        let key = process_symbol_name(&args[idx])?.to_ascii_lowercase();
        let value = &args[idx + 1];
        match key.as_str() {
            "times" => {
                let n = process_number_arg(Some(value), "ratchet! :times")?;
                if !n.is_finite() || n < 0.0 || n.fract() != 0.0 {
                    return Err("ratchet! :times expects a non-negative integer".to_string());
                }
                if n > 1024.0 {
                    return Err("ratchet! :times must be <= 1024".to_string());
                }
                times = Some(n as u32);
            }
            "mode" => {
                let value = process_symbol_name(value)?.to_ascii_lowercase();
                mode = match value.as_str() {
                    "subdivide" => crate::process::ProcessRatchetMode::Subdivide,
                    "repeat" => crate::process::ProcessRatchetMode::Repeat,
                    _ => return Err("ratchet! :mode expects :subdivide or :repeat".to_string()),
                };
            }
            "span" => {
                let beats = process_number_arg(Some(value), "ratchet! :span")?;
                if !beats.is_finite() || beats < 0.0 {
                    return Err(
                        "ratchet! :span expects a non-negative finite beat value".to_string()
                    );
                }
                span_beats = Some(beats as f32);
            }
            "shape" => {
                if !process_value_is_callable(value) {
                    return Err("ratchet! :shape expects a callable value".to_string());
                }
                shape = Some(value.clone());
            }
            other => return Err(format!("ratchet! unknown key :{other}")),
        }
        idx += 2;
    }
    let times = times.ok_or_else(|| "ratchet! requires :times".to_string())?;
    Ok((times, mode, span_beats, shape))
}

pub(in crate::lisp_host) fn process_ratchet_event_value(event: crate::process::ProcessRatchetEvent) -> EValue {
    fn number(value: impl Into<f64>) -> Rc<RefCell<EValue>> {
        Rc::new(RefCell::new(EValue::Number(value.into())))
    }

    let mut map = HashMap::new();
    map.insert(
        "offset-beats".to_string(),
        number(event.offset_beats as f64),
    );
    map.insert(
        "duration".to_string(),
        number(event.resolved.duration as f64),
    );
    map.insert(
        "velocity".to_string(),
        number(event.resolved.velocity as f64),
    );
    map.insert("speed".to_string(), number(event.resolved.speed as f64));
    map.insert("aux-a".to_string(), number(event.resolved.aux_a as f64));
    map.insert("aux-b".to_string(), number(event.resolved.aux_b as f64));
    map.insert(
        "transpose".to_string(),
        number(event.resolved.transpose as f64),
    );
    map.insert("pan".to_string(), number(event.resolved.pan as f64));
    map.insert("chop".to_string(), number(event.resolved.chop as f64));
    EValue::Map(map)
}

pub(in crate::lisp_host) fn process_ratchet_event_number(
    map: &HashMap<String, Rc<RefCell<EValue>>>,
    key: &str,
) -> Result<f32, String> {
    match map.get(key).map(|value| value.borrow().clone()) {
        Some(EValue::Number(value)) if value.is_finite() => Ok(value as f32),
        Some(other) => Err(format!(
            "ratchet event field '{key}' must be a finite number, got {}",
            eseqlisp::vm::format_lisp_value(&other)
        )),
        None => Err(format!("ratchet event missing field '{key}'")),
    }
}

pub(in crate::lisp_host) fn process_ratchet_event_from_value(
    value: &EValue,
) -> Result<crate::process::ProcessRatchetEvent, String> {
    let EValue::Map(map) = value else {
        return Err("ratchet shape must return or mutate an event map".to_string());
    };
    Ok(crate::process::ProcessRatchetEvent {
        offset_beats: process_ratchet_event_number(map, "offset-beats")?,
        resolved: ResolvedStep {
            duration: process_ratchet_event_number(map, "duration")?,
            velocity: process_ratchet_event_number(map, "velocity")?,
            speed: process_ratchet_event_number(map, "speed")?,
            aux_a: process_ratchet_event_number(map, "aux-a")?,
            aux_b: process_ratchet_event_number(map, "aux-b")?,
            transpose: process_ratchet_event_number(map, "transpose")?,
            pan: process_ratchet_event_number(map, "pan")?,
            chop: process_ratchet_event_number(map, "chop")?,
        },
    })
}

pub(in crate::lisp_host) fn process_ratchet_event_param_key(native: &str) -> Option<&'static str> {
    match native.trim_end_matches('!') {
        "vel" => Some("velocity"),
        "note" => Some("transpose"),
        "dur" => Some("duration"),
        "speed" => Some("speed"),
        "pan" => Some("pan"),
        "chop" => Some("chop"),
        _ => None,
    }
}

pub(in crate::lisp_host) fn process_ratchet_event_read(value: Option<&EValue>, native: &str) -> Result<EValue, String> {
    let key = process_ratchet_event_param_key(native)
        .ok_or_else(|| format!("unknown ratchet event reader '{native}'"))?;
    let Some(EValue::Map(map)) = value else {
        return Err(format!("{native} expects a ratchet event map"));
    };
    Ok(EValue::Number(
        process_ratchet_event_number(map, key)? as f64
    ))
}

pub(in crate::lisp_host) fn process_ratchet_event_write(args: &[EValue], native: &str) -> Result<EValue, String> {
    let key = process_ratchet_event_param_key(native)
        .ok_or_else(|| format!("unknown ratchet event writer '{native}'"))?;
    let Some(EValue::Map(map)) = args.first() else {
        return Err(format!("{native} expects an event map and number"));
    };
    let value = process_number_arg(args.get(1), native)?;
    if !value.is_finite() {
        return Err(format!("{native} expects a finite number"));
    }
    let Some(cell) = map.get(key) else {
        return Err(format!("ratchet event missing field '{key}'"));
    };
    *cell.borrow_mut() = EValue::Number(value);
    Ok(EValue::Number(value))
}

pub(in crate::lisp_host) fn register_process_ratchet_event_natives(runtime: &mut Runtime) {
    for native in ["vel", "note", "dur", "speed", "pan", "chop"] {
        runtime.register_native_with_docs(
            native,
            native,
            "Read a ratchet shape event parameter.",
            move |args, _ctx| {
                if args.len() != 1 {
                    return Err(format!("{native} expects one event argument"));
                }
                process_ratchet_event_read(args.first(), native)
            },
        );
    }
    for native in ["vel!", "note!", "dur!", "speed!", "pan!", "chop!"] {
        runtime.register_native_with_docs(
            native,
            native,
            "Mutate a ratchet shape event parameter.",
            move |args, _ctx| {
                if args.len() != 2 {
                    return Err(format!("{native} expects an event and number"));
                }
                process_ratchet_event_write(&args, native)
            },
        );
    }
    runtime.register_native_with_docs(
        "nudge!",
        "(nudge! event beats)",
        "Offset a ratchet shape event by a number of beats.",
        move |args, _ctx| {
            if args.len() != 2 {
                return Err("nudge! expects an event and beat offset".to_string());
            }
            let Some(EValue::Map(map)) = args.first() else {
                return Err("nudge! expects an event map and beat offset".to_string());
            };
            let amount = process_number_arg(args.get(1), "nudge!")?;
            if !amount.is_finite() {
                return Err("nudge! expects a finite beat offset".to_string());
            }
            let current = process_ratchet_event_number(map, "offset-beats")? as f64;
            let Some(cell) = map.get("offset-beats") else {
                return Err("ratchet event missing field 'offset-beats'".to_string());
            };
            let next = current + amount;
            *cell.borrow_mut() = EValue::Number(next);
            Ok(EValue::Number(next))
        },
    );
}

pub(in crate::lisp_host) fn push_process_target_write(
    process_eval: &SharedProcessEvalContext,
    op: crate::process::ProcessTargetOp,
    port: Option<String>,
    value: f32,
) -> Result<(), String> {
    let mut guard = process_eval
        .lock()
        .map_err(|_| "failed to lock process eval context".to_string())?;
    let Some(ctx) = guard.as_mut() else {
        return Err("target write called outside process execution".to_string());
    };
    ensure_process_run_scope(ctx, "target write")?;
    if !ctx.conductor_play_tracks.is_empty() && ctx.step_context.is_none() {
        return Err(
            "conductors play through emit; direct target writes are not supported".to_string(),
        );
    }
    let port = match port {
        Some(port) => port,
        None => match ctx.ports.as_slice() {
            [only] => only.name.clone(),
            [] => return Err("process target write requires :target or :targets".to_string()),
            _ => {
                return Err(
                    "process has multiple target ports; target write requires an explicit port"
                        .to_string(),
                );
            }
        },
    };
    let Some(port_def) = ctx.ports.iter().find(|entry| entry.name == port).cloned() else {
        return Err(format!("unknown process target port '{port}'"));
    };
    let write = crate::process::ProcessTargetWrite {
        port: port_def.name,
        target: port_def.target,
        op,
        value,
    };
    ctx.target_writes.push(write.clone());
    ctx.commands
        .push(crate::process::ProcessRunCommand::TargetWrite(write));
    Ok(())
}

pub(in crate::lisp_host) fn parse_process_def(name: &str, args: &[EValue]) -> Result<crate::process::ProcessDef, String> {
    let mut inlets = Vec::new();
    let mut outlets = Vec::new();
    let mut state = Vec::new();
    let mut every = None;
    let mut seed_policy = crate::process::ProcessSeedPolicy::default();
    let mut ports: Option<Vec<crate::process::ProcessPortDef>> = None;
    let mut doc = None;
    let mut run_value = None;
    let mut listen_value = None;
    let mut handlers: HashMap<String, EValue> = HashMap::new();
    let mut idx = 0;
    while idx < args.len() {
        let key = process_symbol_name(&args[idx])?.to_ascii_lowercase();
        idx += 1;
        let Some(value) = args.get(idx) else {
            return Err(format!("def-process missing value for :{key}"));
        };
        match key.as_str() {
            "in" => inlets = parse_process_inlets(value)?,
            "out" => outlets = parse_process_outlets(value)?,
            "state" => state = parse_process_state(value)?,
            "every" => every = Some(parse_process_time_expr(value)?),
            "seed" => seed_policy = parse_process_seed_policy(value)?,
            "target" => {
                if ports.is_some() {
                    return Err("def-process cannot specify both :target and :targets".to_string());
                }
                ports = Some(vec![parse_process_default_target(value)?]);
            }
            "targets" => {
                if ports.is_some() {
                    return Err("def-process cannot specify both :target and :targets".to_string());
                }
                ports = Some(parse_process_ports(value)?);
            }
            "run" => run_value = Some(value.clone()),
            "listen" => listen_value = Some(value.clone()),
            other if other.starts_with("on-") => {
                handlers.insert(other.trim_start_matches("on-").to_string(), value.clone());
            }
            "doc" => {
                if let EValue::String(value) = value {
                    doc = Some(value.clone());
                }
            }
            "phase" | "init" => {}
            other => return Err(format!("def-process unknown key :{other}")),
        }
        idx += 1;
    }
    let state_names = state
        .iter()
        .map(|entry| entry.name.clone())
        .collect::<Vec<_>>();
    let inlet_names = inlets
        .iter()
        .map(|entry| entry.name.clone())
        .collect::<Vec<_>>();
    let run_source = run_value
        .as_ref()
        .map(|value| wrap_process_body_source(&state_names, &inlet_names, value))
        .transpose()?;
    let listens = listen_value
        .as_ref()
        .map(|value| parse_process_listens(value, &handlers, &state_names, &inlet_names))
        .transpose()?
        .unwrap_or_default();
    Ok(crate::process::ProcessDef {
        id: crate::process::stable_process_id(name),
        name: name.to_string(),
        source_path: None,
        doc,
        inlets,
        outlets,
        state,
        every,
        seed_policy,
        ports: ports.unwrap_or_default(),
        accumulator: None,
        run_source,
        listens,
    })
}

pub(in crate::lisp_host) fn value_list(value: &EValue) -> Option<Vec<EValue>> {
    match value {
        EValue::List(items) => Some(items.iter().map(|item| item.borrow().clone()).collect()),
        _ => None,
    }
}

pub(in crate::lisp_host) fn parse_process_inlets(value: &EValue) -> Result<Vec<crate::process::ProcessInletDef>, String> {
    let entries = value_list(value).ok_or_else(|| ":in expects a list".to_string())?;
    entries
        .iter()
        .map(|entry| {
            let items =
                value_list(entry).ok_or_else(|| "inlet declaration must be a list".to_string())?;
            let name = process_symbol_name(
                items
                    .first()
                    .ok_or_else(|| "inlet declaration missing name".to_string())?,
            )?;
            let (kind, min, max) = parse_process_inlet_kind_and_range(&items)?;
            let default = keyword_value(&items, "default").unwrap_or(EValue::Number(0.0));
            let lane = keyword_value(&items, "lane")
                .map(|value| process_truthy(&value))
                .unwrap_or(false);
            let doc = keyword_value(&items, "doc").and_then(|value| match value {
                EValue::String(value) => Some(value),
                _ => None,
            });
            Ok(crate::process::ProcessInletDef {
                name,
                kind,
                min,
                max,
                default,
                lane,
                doc,
            })
        })
        .collect()
}

pub(in crate::lisp_host) fn process_truthy(value: &EValue) -> bool {
    !matches!(value, EValue::Nil | EValue::Bool(false))
}

pub(in crate::lisp_host) fn parse_process_inlet_kind_and_range(
    items: &[EValue],
) -> Result<(crate::process::ProcessInletKind, Option<f32>, Option<f32>), String> {
    let Some(kind_value) = items.get(1) else {
        return Ok((crate::process::ProcessInletKind::Any, None, None));
    };
    let Ok(kind_name) = process_symbol_name(kind_value) else {
        return Ok((crate::process::ProcessInletKind::Any, None, None));
    };
    if matches!(
        kind_name.as_str(),
        "default" | "lane" | "doc" | "min" | "max"
    ) {
        return Ok((crate::process::ProcessInletKind::Any, None, None));
    }
    let kind = match kind_name.as_str() {
        "float" => crate::process::ProcessInletKind::Float,
        "int" | "integer" => crate::process::ProcessInletKind::Int,
        "gate" | "bool" | "boolean" => crate::process::ProcessInletKind::Gate,
        "track" => crate::process::ProcessInletKind::Track,
        "field" => crate::process::ProcessInletKind::Field,
        "any" => crate::process::ProcessInletKind::Any,
        other => return Err(format!("unknown process inlet kind :{other}")),
    };
    let positional_min = items.get(2).and_then(|value| match value {
        EValue::Number(value) => Some(*value as f32),
        _ => None,
    });
    let positional_max = items.get(3).and_then(|value| match value {
        EValue::Number(value) => Some(*value as f32),
        _ => None,
    });
    let min = keyword_value(items, "min")
        .and_then(|value| match value {
            EValue::Number(value) => Some(value as f32),
            _ => None,
        })
        .or(positional_min);
    let max = keyword_value(items, "max")
        .and_then(|value| match value {
            EValue::Number(value) => Some(value as f32),
            _ => None,
        })
        .or(positional_max);
    Ok((kind, min, max))
}

pub(in crate::lisp_host) fn parse_process_seed_policy(value: &EValue) -> Result<crate::process::ProcessSeedPolicy, String> {
    let name = process_symbol_name(value)?.to_ascii_lowercase();
    match name.as_str() {
        "locked" => Ok(crate::process::ProcessSeedPolicy::Locked),
        "per-cycle" | "per_cycle" => Ok(crate::process::ProcessSeedPolicy::PerCycle),
        other => Err(format!("unknown process seed policy :{other}")),
    }
}

pub(in crate::lisp_host) fn parse_process_default_target(value: &EValue) -> Result<crate::process::ProcessPortDef, String> {
    if value_list(value).is_some() {
        return Ok(crate::process::ProcessPortDef::default_with_target(
            parse_process_target_hint(value)?,
        ));
    }
    let name = process_symbol_name(value)?.to_ascii_lowercase();
    if name == "mappable" {
        return Ok(crate::process::ProcessPortDef::default_mappable(None, None));
    }
    Err(":target expects a target hint list or :mappable".to_string())
}

pub(in crate::lisp_host) fn parse_process_accumulator_target(
    value: &EValue,
) -> Result<crate::process::ProcessPortDef, String> {
    parse_process_default_target(value)
}

pub(in crate::lisp_host) fn parse_process_target_kind(value: &EValue) -> Result<crate::process::ProcessTargetKind, String> {
    let name = process_symbol_name(value)?.to_ascii_lowercase();
    match name.as_str() {
        "step-param" | "step_param" | "step" => Ok(crate::process::ProcessTargetKind::StepParam),
        "device-param" | "device_param" | "device" => {
            Ok(crate::process::ProcessTargetKind::DeviceParam)
        }
        "instrument-param" | "instrument_param" | "instrument" => {
            Ok(crate::process::ProcessTargetKind::InstrumentParam)
        }
        "effect-param" | "effect_param" | "effect" | "fx-param" | "fx_param" => {
            Ok(crate::process::ProcessTargetKind::EffectParam)
        }
        "midi-fx-param" | "midi_fx_param" | "midi-fx" | "midi_fx" => {
            Ok(crate::process::ProcessTargetKind::MidiFxParam)
        }
        "process-inlet" | "process_inlet" | "inlet" => {
            Ok(crate::process::ProcessTargetKind::ProcessInlet)
        }
        "rack-slot-param" | "rack_slot_param" | "rack-slot" | "rack_slot" => {
            Ok(crate::process::ProcessTargetKind::RackSlotParam)
        }
        "rack-slot-instrument-param"
        | "rack_slot_instrument_param"
        | "rack-instrument-param"
        | "rack_instrument_param" => Ok(crate::process::ProcessTargetKind::RackSlotInstrumentParam),
        "rack-macro-param" | "rack_macro_param" | "rack-macro" | "rack_macro" => {
            Ok(crate::process::ProcessTargetKind::RackMacroParam)
        }
        other => Err(format!("unknown process target kind :{other}")),
    }
}

pub(in crate::lisp_host) fn parse_process_port_def(items: &[EValue]) -> Result<crate::process::ProcessPortDef, String> {
    let name = process_symbol_name(
        items
            .first()
            .ok_or_else(|| "target port declaration missing name".to_string())?,
    )?;
    if name == crate::process::DEFAULT_PROCESS_PORT {
        return Err(format!(
            "'{name}' is reserved for internal default target ports"
        ));
    }
    let tail = &items[1..];
    match tail {
        [target] if value_list(target).is_some() => Ok(
            crate::process::ProcessPortDef::with_target(name, parse_process_target_hint(target)?),
        ),
        [marker] => {
            let marker = process_symbol_name(marker)?.to_ascii_lowercase();
            match marker.as_str() {
                "mappable" => Ok(crate::process::ProcessPortDef::mappable(name, None, None)),
                "process-inlet" | "process_inlet" => {
                    Ok(crate::process::ProcessPortDef::process_inlet(name))
                }
                _ => Err(
                    "target port expects a target hint, :mappable, or :process-inlet".to_string(),
                ),
            }
        }
        [marker, value] => {
            let marker = process_symbol_name(marker)?.to_ascii_lowercase();
            if marker != "mappable" {
                return Err("target port expects :mappable before target kind or hint".to_string());
            }
            if value_list(value).is_some() {
                Ok(crate::process::ProcessPortDef::mappable(
                    name,
                    None,
                    Some(parse_process_target_hint(value)?),
                ))
            } else {
                let target_kind = parse_process_target_kind(value)?;
                if target_kind == crate::process::ProcessTargetKind::ProcessInlet {
                    return Err(
                        "process-inlet ports use (name :process-inlet) and connect!, not :mappable"
                            .to_string(),
                    );
                }
                Ok(crate::process::ProcessPortDef::mappable(
                    name,
                    Some(target_kind),
                    None,
                ))
            }
        }
        [] => Err(
            "target port declaration requires a target hint, :mappable, or :process-inlet"
                .to_string(),
        ),
        _ => Err("target port declaration has too many fields".to_string()),
    }
}

pub(in crate::lisp_host) fn parse_process_ports(value: &EValue) -> Result<Vec<crate::process::ProcessPortDef>, String> {
    let entries = value_list(value).ok_or_else(|| ":targets expects a list".to_string())?;
    let mut ports = Vec::new();
    for entry in entries {
        let items = value_list(&entry)
            .ok_or_else(|| "target port declaration must be a list".to_string())?;
        let port = parse_process_port_def(&items)?;
        if ports
            .iter()
            .any(|existing: &crate::process::ProcessPortDef| existing.name == port.name)
        {
            return Err(format!("duplicate target port '{}'", port.name));
        }
        ports.push(port);
    }
    Ok(ports)
}

pub(in crate::lisp_host) fn parse_process_target_hint(value: &EValue) -> Result<crate::process::ProcessTargetHint, String> {
    let items = value_list(value).ok_or_else(|| "process target must be a list".to_string())?;
    let head = process_symbol_name(
        items
            .first()
            .ok_or_else(|| "process target missing head".to_string())?,
    )?
    .to_ascii_lowercase();
    match head.as_str() {
        "step-param" => {
            let param = process_symbol_name(
                items
                    .get(1)
                    .ok_or_else(|| "(step-param :name) expects a param".to_string())?,
            )?;
            Ok(crate::process::ProcessTargetHint::StepParam { param })
        }
        "param-tag" => {
            let tag = process_symbol_name(
                items
                    .get(1)
                    .ok_or_else(|| "(param-tag :tag) expects a tag".to_string())?,
            )?;
            Ok(crate::process::ProcessTargetHint::ParamTag { tag })
        }
        "instrument-param" => {
            let param = process_symbol_name(
                items
                    .get(1)
                    .ok_or_else(|| "(instrument-param :name) expects a param".to_string())?,
            )?;
            Ok(crate::process::ProcessTargetHint::InstrumentParam { param })
        }
        "effect-param" => {
            let effect =
                process_symbol_name(items.get(1).ok_or_else(|| {
                    "(effect-param :effect :param) expects an effect".to_string()
                })?)?;
            let param = process_symbol_name(
                items
                    .get(2)
                    .ok_or_else(|| "(effect-param :effect :param) expects a param".to_string())?,
            )?;
            Ok(crate::process::ProcessTargetHint::EffectParam { effect, param })
        }
        "fx-param" | "midi-fx-param" | "midi-fx-target" => {
            let fx =
                process_symbol_name(items.get(1).ok_or_else(|| {
                    "(midi-fx-target :fx :param) expects an fx name".to_string()
                })?)?;
            let param = process_symbol_name(
                items
                    .get(2)
                    .ok_or_else(|| "(midi-fx-target :fx :param) expects a param".to_string())?,
            )?;
            Ok(crate::process::ProcessTargetHint::MidiFxParam { fx, param })
        }
        "rack-macro" => {
            let key =
                process_symbol_name(items.get(1).ok_or_else(|| {
                    "(rack-macro :macro_1) expects a macro identifier".to_string()
                })?)?;
            let normalized = key
                .trim_start_matches(':')
                .replace('-', "_")
                .to_ascii_lowercase();
            let number = normalized
                .strip_prefix("macro_")
                .and_then(|value| value.parse::<usize>().ok())
                .filter(|value| (1..=crate::sequencer::RACK_MACRO_COUNT).contains(value))
                .ok_or_else(|| format!("unknown rack macro :{key}"))?;
            Ok(crate::process::ProcessTargetHint::RackMacroParam {
                macro_id: (number - 1) as u8,
            })
        }
        other => Err(format!("unsupported process target {other}")),
    }
}

pub(in crate::lisp_host) fn parse_process_connection_target(value: &EValue) -> Result<crate::process::ParamTarget, String> {
    let items =
        value_list(value).ok_or_else(|| "process port target must be a list".to_string())?;
    let head = process_symbol_name(
        items
            .first()
            .ok_or_else(|| "process port target missing head".to_string())?,
    )?
    .to_ascii_lowercase();
    match head.as_str() {
        "process-inlet" => {
            if items.len() != 3 {
                return Err("(process-inlet :class :inlet) expects two arguments".to_string());
            }
            Ok(crate::process::ParamTarget::ProcessInlet {
                process: process_symbol_name(&items[1])?,
                inlet: process_symbol_name(&items[2])?,
                instance_id: None,
            })
        }
        PROCESS_INLET_INSTANCE_TARGET_TAG => {
            if items.len() != 4 {
                return Err("(inlet process :inlet) target has invalid arity".to_string());
            }
            let EValue::Number(raw_id) = items[1] else {
                return Err("(inlet process :inlet) target has invalid process id".to_string());
            };
            if !raw_id.is_finite() || raw_id < 0.0 || raw_id.fract() != 0.0 {
                return Err(
                    "(inlet process :inlet) target id must be a non-negative integer".to_string(),
                );
            }
            Ok(crate::process::ParamTarget::ProcessInlet {
                process: process_symbol_name(&items[2])?,
                inlet: process_symbol_name(&items[3])?,
                instance_id: Some(crate::process::ProcessInstanceId(raw_id as u64)),
            })
        }
        other => Err(format!(
            "connect! target must be a process inlet, got {other}"
        )),
    }
}

pub(in crate::lisp_host) fn parse_process_accumulator_amount_inlet(
    value: &EValue,
) -> Result<crate::process::ProcessInletDef, String> {
    let items =
        value_list(value).ok_or_else(|| ":amount expects an inlet declaration".to_string())?;
    let name = process_symbol_name(
        items
            .first()
            .ok_or_else(|| ":amount declaration missing name".to_string())?,
    )?;
    let (kind, min, max) = parse_process_inlet_kind_and_range(&items)?;
    let default = keyword_value(&items, "default").unwrap_or(EValue::Number(0.0));
    let doc = keyword_value(&items, "doc").and_then(|value| match value {
        EValue::String(value) => Some(value),
        _ => None,
    });
    Ok(crate::process::ProcessInletDef {
        name,
        kind,
        min,
        max,
        default,
        lane: true,
        doc,
    })
}

pub(in crate::lisp_host) fn parse_process_accumulator_range(value: &EValue) -> Result<(f32, f32), String> {
    let items = value_list(value).ok_or_else(|| ":range expects (lo hi)".to_string())?;
    let Some(EValue::Number(lo)) = items.first() else {
        return Err(":range expects numeric lo".to_string());
    };
    let Some(EValue::Number(hi)) = items.get(1) else {
        return Err(":range expects numeric hi".to_string());
    };
    if hi <= lo {
        return Err(":range high must be greater than low".to_string());
    }
    Ok((*lo as f32, *hi as f32))
}

pub(in crate::lisp_host) fn parse_process_accumulator_mode(
    value: &EValue,
) -> Result<crate::process::ProcessAccumulatorMode, String> {
    let name = process_symbol_name(value)?.to_ascii_lowercase();
    match name.as_str() {
        "wrap" => Ok(crate::process::ProcessAccumulatorMode::Wrap),
        "clip" => Ok(crate::process::ProcessAccumulatorMode::Clip),
        "bounce" => Ok(crate::process::ProcessAccumulatorMode::Bounce),
        other => Err(format!("unknown def-accumulator mode :{other}")),
    }
}

pub(in crate::lisp_host) fn parse_process_outlets(value: &EValue) -> Result<Vec<crate::process::ProcessOutletDef>, String> {
    let entries = value_list(value).ok_or_else(|| ":out expects a list".to_string())?;
    entries
        .iter()
        .map(|entry| {
            let items =
                value_list(entry).ok_or_else(|| "outlet declaration must be a list".to_string())?;
            let name = process_symbol_name(
                items
                    .first()
                    .ok_or_else(|| "outlet declaration missing name".to_string())?,
            )?;
            Ok(crate::process::ProcessOutletDef { name })
        })
        .collect()
}

pub(in crate::lisp_host) fn parse_process_state(value: &EValue) -> Result<Vec<crate::process::ProcessStateDef>, String> {
    let entries = value_list(value).ok_or_else(|| ":state expects a list".to_string())?;
    entries
        .iter()
        .map(|entry| {
            let items =
                value_list(entry).ok_or_else(|| "state declaration must be a list".to_string())?;
            let name = process_symbol_name(
                items
                    .first()
                    .ok_or_else(|| "state declaration missing name".to_string())?,
            )?;
            let initial = items.get(1).cloned().unwrap_or(EValue::Number(0.0));
            Ok(crate::process::ProcessStateDef { name, initial })
        })
        .collect()
}

pub(in crate::lisp_host) fn parse_process_listens(
    value: &EValue,
    handlers: &HashMap<String, EValue>,
    state_names: &[String],
    inlet_names: &[String],
) -> Result<Vec<crate::process::ProcessListenDef>, String> {
    let entries = value_list(value).ok_or_else(|| ":listen expects a list".to_string())?;
    entries
        .iter()
        .map(|entry| {
            let items =
                value_list(entry).ok_or_else(|| "listen declaration must be a list".to_string())?;
            let name = process_symbol_name(
                items
                    .first()
                    .ok_or_else(|| "listen declaration missing name".to_string())?,
            )?;
            let source_value = items
                .get(1)
                .ok_or_else(|| "listen declaration missing event source".to_string())?;
            let source = parse_process_event_source(source_value)?;
            let handler = handlers
                .get(&name)
                .ok_or_else(|| format!("listen :{name} missing :on-{name} handler"))?;
            Ok(crate::process::ProcessListenDef {
                name,
                source,
                handler_source: wrap_process_handler_source(state_names, inlet_names, handler)?,
            })
        })
        .collect()
}

pub(in crate::lisp_host) fn parse_process_event_source(
    value: &EValue,
) -> Result<crate::process::ProcessEventSource, String> {
    if matches!(
        value,
        EValue::String(_) | EValue::Symbol(_) | EValue::Keyword(_)
    ) {
        return Ok(crate::process::ProcessEventSource::Channel(
            process_symbol_name(value)?,
        ));
    }
    let items = value_list(value).ok_or_else(|| "event source must be a list".to_string())?;
    let head = process_symbol_name(
        items
            .first()
            .ok_or_else(|| "event source missing head".to_string())?,
    )?;
    match head.as_str() {
        "track-fires" => {
            let Some(EValue::Number(track)) = items.get(1) else {
                return Err("track-fires expects a track number".to_string());
            };
            Ok(crate::process::ProcessEventSource::TrackFires(
                *track as usize,
            ))
        }
        "seq-fires" => {
            let name = items
                .get(1)
                .map(process_symbol_name)
                .transpose()?
                .unwrap_or_default();
            Ok(crate::process::ProcessEventSource::SeqFires(name))
        }
        "chan" | "channel" => {
            let name = items
                .get(1)
                .map(process_symbol_name)
                .transpose()?
                .unwrap_or_default();
            Ok(crate::process::ProcessEventSource::Channel(name))
        }
        other => Err(format!("unknown process event source {other}")),
    }
}

pub(in crate::lisp_host) fn keyword_value(items: &[EValue], key: &str) -> Option<EValue> {
    let mut idx = 0;
    while idx + 1 < items.len() {
        if process_symbol_name(&items[idx])
            .ok()
            .is_some_and(|name| name.eq_ignore_ascii_case(key))
        {
            return Some(items[idx + 1].clone());
        }
        idx += 1;
    }
    None
}

pub(in crate::lisp_host) fn parse_process_time_expr(value: &EValue) -> Result<crate::process::ProcessTimeExpr, String> {
    match value {
        EValue::Number(beats) => Ok(crate::process::ProcessTimeExpr::Beats(*beats)),
        EValue::Keyword(_) | EValue::Symbol(_) | EValue::String(_) => {
            let tb = parse_timebase_arg(std::slice::from_ref(value), 0)?;
            Ok(crate::process::ProcessTimeExpr::Beats(tb.step_beats(
                crate::generator::GENERATOR_RESOLUTION_REF_STEPS,
            )))
        }
        EValue::List(_) => {
            let items =
                value_list(value).ok_or_else(|| "time expression must be a list".to_string())?;
            let head = process_symbol_name(
                items
                    .first()
                    .ok_or_else(|| "time expression missing head".to_string())?,
            )?;
            match head.as_str() {
                "beats" => {
                    let Some(EValue::Number(beats)) = items.get(1) else {
                        return Err("(beats n) expects a number".to_string());
                    };
                    Ok(crate::process::ProcessTimeExpr::Beats(*beats))
                }
                "bars" => {
                    let Some(EValue::Number(bars)) = items.get(1) else {
                        return Err("(bars n) expects a number".to_string());
                    };
                    Ok(crate::process::ProcessTimeExpr::Beats(*bars * 4.0))
                }
                "in" => {
                    let inlet = process_symbol_name(
                        items
                            .get(1)
                            .ok_or_else(|| "(in :name) expects an inlet".to_string())?,
                    )?;
                    Ok(crate::process::ProcessTimeExpr::Inlet(inlet))
                }
                other => Err(format!("unsupported process time expression {other}")),
            }
        }
        _ => Err("unsupported process time expression".to_string()),
    }
}

pub(in crate::lisp_host) fn wrap_process_source_with_bindings(
    state_names: &[String],
    inlet_names: &[String],
    extra_bindings: Vec<(String, String)>,
    body_source: String,
) -> Result<String, String> {
    let mut seen = BTreeSet::new();
    let mut params = Vec::new();
    let mut args = Vec::new();
    for name in state_names {
        if !seen.insert(name.clone()) {
            return Err(format!("duplicate process binding `{name}`"));
        }
        params.push(name.clone());
        args.push(format!("(__process-state-get :{name})"));
    }
    for name in inlet_names {
        if !seen.insert(name.clone()) {
            return Err(format!("duplicate process binding `{name}`"));
        }
        params.push(name.clone());
        args.push(format!("(in :{name})"));
    }
    for (name, expr) in extra_bindings {
        if !seen.insert(name.clone()) {
            return Err(format!("duplicate process binding `{name}`"));
        }
        params.push(name);
        args.push(expr);
    }
    if params.is_empty() {
        return Ok(body_source);
    }
    let stores = state_names
        .iter()
        .map(|name| format!("(__process-state-set! :{name} {name})"))
        .collect::<Vec<_>>();
    let body_with_stores = if stores.is_empty() {
        body_source
    } else {
        format!("(do {body_source} {})", stores.join(" "))
    };
    Ok(format!(
        "((lambda ({}) {}) {})",
        params.join(" "),
        body_with_stores,
        args.join(" ")
    ))
}

pub(in crate::lisp_host) fn wrap_process_body_source(
    state_names: &[String],
    inlet_names: &[String],
    body: &EValue,
) -> Result<String, String> {
    wrap_process_source_with_bindings(
        state_names,
        inlet_names,
        Vec::new(),
        eseqlisp::vm::format_lisp_source(body),
    )
}

pub(in crate::lisp_host) fn wrap_process_handler_source(
    state_names: &[String],
    inlet_names: &[String],
    handler: &EValue,
) -> Result<String, String> {
    let items =
        value_list(handler).ok_or_else(|| "process event handler expects a lambda".to_string())?;
    let head = process_symbol_name(
        items
            .first()
            .ok_or_else(|| "process event handler lambda is empty".to_string())?,
    )?;
    if head != "lambda" {
        return Err("process event handler expects a lambda".to_string());
    }
    let args = value_list(
        items
            .get(1)
            .ok_or_else(|| "process event handler lambda missing args".to_string())?,
    )
    .ok_or_else(|| "process event handler lambda args must be a list".to_string())?;
    if args.len() != 1 {
        return Err("process event handler lambda expects exactly one argument".to_string());
    }
    let event_arg = process_symbol_name(&args[0])?;
    let body = items
        .get(2..)
        .ok_or_else(|| "process event handler lambda missing body".to_string())?;
    let body_source = process_body_source(body)?;
    wrap_process_source_with_bindings(
        state_names,
        inlet_names,
        vec![(event_arg, "(__process-event-value)".to_string())],
        body_source,
    )
}

pub(in crate::lisp_host) fn process_body_source(body: &[EValue]) -> Result<String, String> {
    match body {
        [] => Err("process body cannot be empty".to_string()),
        [single] => Ok(eseqlisp::vm::format_lisp_source(single)),
        forms => Ok(format!(
            "(do {})",
            forms
                .iter()
                .map(eseqlisp::vm::format_lisp_source)
                .collect::<Vec<_>>()
                .join(" ")
        )),
    }
}

pub(in crate::lisp_host) fn construct_process_instance(
    process_authoring: &SharedProcessAuthoring,
    class_name: &str,
    args: Vec<EValue>,
    anonymous: bool,
    running: bool,
    every: Option<crate::process::ProcessTimeExpr>,
    run_source: Option<String>,
    one_shot: bool,
    process_chain_state: Option<Arc<crate::sequencer::SequencerState>>,
    publish: Option<ProcessPublishHook>,
) -> Result<EValue, String> {
    let constructor_args = parse_process_constructor_args(process_authoring, &args)?;
    let mut registry = process_authoring
        .lock()
        .map_err(|_| "failed to lock process registry".to_string())?;
    let def = registry
        .defs
        .iter()
        .find(|def| def.name == class_name)
        .ok_or_else(|| format!("unknown process class '{class_name}'"))?;
    validate_process_port_bindings(def, &constructor_args.bindings)?;
    let handle_id = crate::process::AuthoredHandleId(registry.next_id());
    registry.upsert_instance(crate::process::AuthoredProcessInstance {
        handle_id,
        name: None,
        class_name: class_name.to_string(),
        inlets: constructor_args.inlets,
        bindings: constructor_args.bindings,
        running,
        anonymous,
        one_shot,
        every,
        run_source,
    });
    Ok(process_instance_handle(
        Arc::clone(process_authoring),
        handle_id,
        process_chain_state,
        publish,
    ))
}

pub(in crate::lisp_host) fn construct_anonymous_listener_process(
    process_authoring: &SharedProcessAuthoring,
    kind: &str,
    source: crate::process::ProcessEventSource,
    handler: &EValue,
    publish: Option<ProcessPublishHook>,
) -> Result<EValue, String> {
    let handler_source = wrap_process_handler_source(&[], &[], handler)?;
    let mut registry = process_authoring
        .lock()
        .map_err(|_| "failed to lock process registry".to_string())?;
    let handle_id = crate::process::AuthoredHandleId(registry.next_id());
    let class_name = format!("__anonymous_{kind}_{}", handle_id.0);
    registry.upsert_def(crate::process::ProcessDef {
        id: crate::process::stable_process_id(&class_name),
        name: class_name.clone(),
        source_path: None,
        doc: None,
        inlets: Vec::new(),
        outlets: Vec::new(),
        state: Vec::new(),
        every: None,
        seed_policy: crate::process::ProcessSeedPolicy::default(),
        ports: Vec::new(),
        accumulator: None,
        run_source: None,
        listens: vec![crate::process::ProcessListenDef {
            name: "event".to_string(),
            source,
            handler_source,
        }],
    });
    registry.upsert_instance(crate::process::AuthoredProcessInstance {
        handle_id,
        name: None,
        class_name,
        inlets: HashMap::new(),
        bindings: BTreeMap::new(),
        running: true,
        anonymous: true,
        one_shot: false,
        every: None,
        run_source: None,
    });
    drop(registry);
    Ok(process_instance_handle(
        Arc::clone(process_authoring),
        handle_id,
        None,
        publish,
    ))
}

pub(in crate::lisp_host) struct ProcessConstructorArgs {
    inlets: HashMap<String, crate::process::ProcessInletValue>,
    bindings: BTreeMap<String, Option<crate::process::ParamTarget>>,
}

pub(in crate::lisp_host) fn parse_process_constructor_args(
    process_authoring: &SharedProcessAuthoring,
    args: &[EValue],
) -> Result<ProcessConstructorArgs, String> {
    if args.len() % 2 != 0 {
        return Err("process constructor expects keyword/value inlet pairs".to_string());
    }
    let mut inlets = HashMap::new();
    let mut bindings = BTreeMap::new();
    let mut idx = 0;
    while idx < args.len() {
        let key = process_symbol_name(&args[idx])?;
        if key.eq_ignore_ascii_case("connect") {
            bindings.extend(parse_process_constructor_connections(&args[idx + 1])?);
        } else if key.eq_ignore_ascii_case("map") {
            return Err(
                "process constructor :map was replaced by :connect for process-inlet connections"
                    .to_string(),
            );
        } else {
            let value = parse_inlet_value(process_authoring, &args[idx + 1])?;
            inlets.insert(key, value);
        }
        idx += 2;
    }
    Ok(ProcessConstructorArgs { inlets, bindings })
}

pub(in crate::lisp_host) fn parse_process_constructor_connections(
    value: &EValue,
) -> Result<BTreeMap<String, Option<crate::process::ParamTarget>>, String> {
    let entries = value_list(value)
        .ok_or_else(|| "process constructor :connect expects a list".to_string())?;
    let mut bindings = BTreeMap::new();
    for entry in entries {
        let items = value_list(&entry)
            .ok_or_else(|| "process constructor :connect entries must be lists".to_string())?;
        if items.len() != 2 {
            return Err("process constructor :connect entries expect (port target)".to_string());
        }
        let port = process_symbol_name(&items[0])?;
        let target = parse_process_connection_target(&items[1])?;
        bindings.insert(port, Some(target));
    }
    Ok(bindings)
}

pub(in crate::lisp_host) fn parse_inlet_value(
    process_authoring: &SharedProcessAuthoring,
    value: &EValue,
) -> Result<crate::process::ProcessInletValue, String> {
    match value {
        EValue::HostHandle { kind, id, .. } if kind == "process-outlet" => {
            let registry = process_authoring
                .lock()
                .map_err(|_| "failed to lock process registry".to_string())?;
            let outlet = registry
                .outlet_handles
                .get(id)
                .cloned()
                .ok_or_else(|| "unknown process outlet handle".to_string())?;
            Ok(crate::process::ProcessInletValue::Outlet(outlet))
        }
        EValue::HostHandle { kind, id, .. } if kind == "channel" => {
            let registry = process_authoring
                .lock()
                .map_err(|_| "failed to lock process registry".to_string())?;
            let name = registry
                .channel_handles
                .get(id)
                .cloned()
                .ok_or_else(|| "unknown channel handle".to_string())?;
            Ok(crate::process::ProcessInletValue::Channel(name))
        }
        _ => Ok(crate::process::ProcessInletValue::Literal(value.clone())),
    }
}

pub(in crate::lisp_host) fn process_instance_handle(
    process_authoring: SharedProcessAuthoring,
    handle_id: crate::process::AuthoredHandleId,
    process_chain_state: Option<Arc<crate::sequencer::SequencerState>>,
    publish: Option<ProcessPublishHook>,
) -> EValue {
    EValue::HostHandle {
        kind: "process".to_string(),
        id: handle_id.0,
        callable: Rc::new(move |args, _vm| {
            let read_only_inline_poll = matches!(
                args.as_slice(),
                [EValue::Keyword(command) | EValue::Symbol(command), _]
                    if command.trim_start_matches(':') == "__inline-read"
            );
            let publish_after_call = args.len() == 2 && !read_only_inline_poll;
            match process_handle_call(
                &process_authoring,
                process_chain_state.as_ref(),
                handle_id,
                args,
            ) {
                Ok(value) => {
                    if publish_after_call {
                        publish_process_authoring(&process_authoring, &publish);
                    }
                    value
                }
                Err(error) => {
                    eprintln!("[process] handle error: {error}");
                    EValue::Bool(false)
                }
            }
        }),
    }
}

/// A `defchan` handle. `(handle :set value)` queues a control-thread channel
/// write that the lookahead worker drains at the top of the next chunk
/// (docs/jaki-live-channel-widgets-spec.md 7); `(handle :__inline-read :set)`
/// reports the value an inline widget bound to the channel should display.
pub(in crate::lisp_host) fn process_channel_handle(
    process_authoring: SharedProcessAuthoring,
    process_chain_state: Option<Arc<crate::sequencer::SequencerState>>,
    handle_id: crate::process::AuthoredHandleId,
) -> EValue {
    EValue::HostHandle {
        kind: "channel".to_string(),
        id: handle_id.0,
        callable: Rc::new(move |args, _vm| {
            match process_channel_handle_call(
                &process_authoring,
                process_chain_state.as_ref(),
                handle_id,
                args,
            ) {
                Ok(value) => value,
                Err(error) => {
                    eprintln!("[process] channel handle error: {error}");
                    EValue::Bool(false)
                }
            }
        }),
    }
}

fn channel_name_for_handle(
    registry: &ProcessAuthoringRegistry,
    handle_id: crate::process::AuthoredHandleId,
) -> Result<String, String> {
    registry
        .channels
        .iter()
        .find(|channel| channel.handle_id == handle_id)
        .and_then(|channel| channel.name.clone())
        .or_else(|| registry.channel_handles.get(&handle_id.0).cloned())
        .ok_or_else(|| "unknown channel handle".to_string())
}

fn process_channel_handle_call(
    process_authoring: &SharedProcessAuthoring,
    process_chain_state: Option<&Arc<crate::sequencer::SequencerState>>,
    handle_id: crate::process::AuthoredHandleId,
    args: Vec<EValue>,
) -> Result<EValue, String> {
    let mut registry = process_authoring
        .lock()
        .map_err(|_| "failed to lock process registry".to_string())?;
    let name = channel_name_for_handle(&registry, handle_id)?;

    let inline_read = matches!(
        args.as_slice(),
        [EValue::Keyword(command) | EValue::Symbol(command), _]
            if command.trim_start_matches(':') == "__inline-read"
    );
    if inline_read {
        // Latest write wins, then the authored initial. The scheduler-side
        // value is not readable from here, which is the point: the widget
        // shows the author's hand, not a process's.
        let value = registry
            .channel_write_echo
            .get(&name)
            .map(|literal| literal.to_value())
            .or_else(|| {
                registry
                    .channels
                    .iter()
                    .find(|channel| channel.handle_id == handle_id)
                    .and_then(|channel| channel.initial.clone())
            })
            .unwrap_or(EValue::Nil);
        return Ok(value);
    }

    let value = match args.as_slice() {
        [EValue::Keyword(command) | EValue::Symbol(command), value]
            if command.trim_start_matches(':') == "set" =>
        {
            value
        }
        [value] => value,
        _ => {
            return Err(format!(
                "channel '{name}' expects (chan :set value) or (chan value)"
            ))
        }
    };
    let literal = crate::process::ProcessLiteral::from_value(value)?;
    registry.queue_channel_write(name, literal);
    // Without a state to hand them to there is no scheduler to reach, so the
    // writes stay queued on the registry and a later call delivers them in
    // order.
    if let Some(state) = process_chain_state {
        state.queue_process_channel_writes(registry.take_pending_channel_writes());
    }
    Ok(EValue::Bool(true))
}

pub(in crate::lisp_host) fn process_outlet_handle(
    process_authoring: SharedProcessAuthoring,
    outlet: crate::process::ProcessOutletRef,
) -> Result<EValue, String> {
    let mut registry = process_authoring
        .lock()
        .map_err(|_| "failed to lock process registry".to_string())?;
    let handle_id = registry.next_id();
    registry.outlet_handles.insert(handle_id, outlet);
    Ok(EValue::HostHandle {
        kind: "process-outlet".to_string(),
        id: handle_id,
        callable: Rc::new(move |_args, _vm| EValue::Bool(true)),
    })
}

pub(in crate::lisp_host) enum DurableProcessHandleUpdate {
    Scalar(crate::process::ProcessLiteral),
    Lane(Vec<f32>),
    None,
}

pub(in crate::lisp_host) fn process_instance_lane_backed_inlet(
    registry: &ProcessAuthoringRegistry,
    handle_id: crate::process::AuthoredHandleId,
    inlet: &str,
) -> Result<bool, String> {
    let instance = registry
        .instances
        .iter()
        .find(|entry| entry.handle_id == handle_id)
        .ok_or_else(|| "unknown process handle".to_string())?;
    let def = registry
        .defs
        .iter()
        .find(|def| def.name == instance.class_name)
        .ok_or_else(|| format!("unknown process class '{}'", instance.class_name))?;
    Ok(def
        .inlets
        .iter()
        .any(|entry| entry.name == inlet && entry.lane))
}

pub(in crate::lisp_host) fn process_handle_call(
    process_authoring: &SharedProcessAuthoring,
    process_chain_state: Option<&Arc<crate::sequencer::SequencerState>>,
    handle_id: crate::process::AuthoredHandleId,
    args: Vec<EValue>,
) -> Result<EValue, String> {
    if let [EValue::Keyword(command), key] = args.as_slice() {
        if command != "__inline-read" {
            // Fall through to the public process-handle call forms below.
        } else {
            let inlet = process_symbol_name(key)?;
            if let Some(value) = process_chain_state.and_then(|state| {
                state.process_inlet_value(crate::process::ProcessInstanceId(handle_id.0), &inlet)
            }) {
                return Ok(value.to_value());
            }
            let registry = process_authoring
                .lock()
                .map_err(|_| "failed to lock process registry".to_string())?;
            return Ok(registry
                .instances
                .iter()
                .find(|instance| instance.handle_id == handle_id)
                .and_then(|instance| instance.inlets.get(&inlet))
                .and_then(|value| match value {
                    crate::process::ProcessInletValue::Literal(value) => Some(value.clone()),
                    _ => None,
                })
                .unwrap_or(EValue::Nil));
        }
    }
    match args.as_slice() {
        [key] => {
            let outlet = process_symbol_name(key)?;
            process_outlet_handle(
                Arc::clone(process_authoring),
                crate::process::ProcessOutletRef {
                    process_handle_id: handle_id,
                    outlet,
                },
            )
        }
        [key, value] => {
            let inlet = process_symbol_name(key)?;
            let value = parse_inlet_value(process_authoring, value)?;
            let attachment_count = process_chain_state
                .map(|state| {
                    state.process_instance_attachment_count(crate::process::ProcessInstanceId(
                        handle_id.0,
                    ))
                })
                .unwrap_or(0);
            let durable_update = match &value {
                crate::process::ProcessInletValue::Literal(literal) => {
                    if let Some(values) = process_lane_values(literal) {
                        DurableProcessHandleUpdate::Lane(values?)
                    } else {
                        DurableProcessHandleUpdate::Scalar(
                            crate::process::ProcessLiteral::from_value(literal)?,
                        )
                    }
                }
                _ if attachment_count > 0 => {
                    return Err(
                        "attached process chain inlets must be literals; use process graphs outside `processes` for outlet/channel wiring"
                            .to_string(),
                    );
                }
                _ => DurableProcessHandleUpdate::None,
            };
            let mut registry = process_authoring
                .lock()
                .map_err(|_| "failed to lock process registry".to_string())?;
            if matches!(durable_update, DurableProcessHandleUpdate::Lane(_))
                && !process_instance_lane_backed_inlet(&registry, handle_id, &inlet)?
            {
                return Err(format!("inlet '{inlet}' is not lane-backed (:lane true)"));
            }
            let instance = registry
                .instances
                .iter_mut()
                .find(|entry| entry.handle_id == handle_id)
                .ok_or_else(|| "unknown process handle".to_string())?;
            instance.inlets.insert(inlet.clone(), value);
            drop(registry);
            if let Some(state) = process_chain_state {
                match durable_update {
                    DurableProcessHandleUpdate::Scalar(literal) => {
                        state.set_process_inlet_value(
                            crate::process::ProcessInstanceId(handle_id.0),
                            &inlet,
                            literal,
                        );
                    }
                    DurableProcessHandleUpdate::Lane(values) => {
                        state.set_process_lane_values(
                            crate::process::ProcessInstanceId(handle_id.0),
                            &inlet,
                            values,
                        );
                    }
                    DurableProcessHandleUpdate::None => {}
                }
            }
            Ok(EValue::Bool(true))
        }
        _ => Err("process handle expects :outlet or :inlet value".to_string()),
    }
}

pub(in crate::lisp_host) fn set_process_running(
    process_authoring: &SharedProcessAuthoring,
    value: Option<&EValue>,
    running: bool,
) -> Result<(), String> {
    let Some(EValue::HostHandle { kind, id, .. }) = value else {
        return Err("start/stop expects a process handle".to_string());
    };
    if kind != "process" {
        return Err("start/stop expects a process handle".to_string());
    }
    let mut registry = process_authoring
        .lock()
        .map_err(|_| "failed to lock process registry".to_string())?;
    let instance = registry
        .instances
        .iter_mut()
        .find(|entry| entry.handle_id.0 == *id)
        .ok_or_else(|| "unknown process handle".to_string())?;
    instance.running = running;
    Ok(())
}

pub(in crate::lisp_host) fn parse_channel_name(
    process_authoring: &SharedProcessAuthoring,
    value: &EValue,
) -> Result<String, String> {
    match value {
        EValue::HostHandle { kind, id, .. } if kind == "channel" => {
            let registry = process_authoring
                .lock()
                .map_err(|_| "failed to lock process registry".to_string())?;
            registry
                .channel_handles
                .get(id)
                .cloned()
                .ok_or_else(|| "unknown channel handle".to_string())
        }
        EValue::String(_) | EValue::Symbol(_) | EValue::Keyword(_) => process_symbol_name(value),
        _ => Err("expected channel handle or name".to_string()),
    }
}

pub(in crate::lisp_host) fn parse_process_source_ref(
    process_authoring: &SharedProcessAuthoring,
    value: &EValue,
) -> Result<crate::process::ProcessSourceRef, String> {
    match value {
        EValue::HostHandle { kind, id, .. } if kind == "process-outlet" => {
            let registry = process_authoring
                .lock()
                .map_err(|_| "failed to lock process registry".to_string())?;
            let outlet = registry
                .outlet_handles
                .get(id)
                .cloned()
                .ok_or_else(|| "unknown process outlet handle".to_string())?;
            Ok(crate::process::ProcessSourceRef::Outlet(outlet))
        }
        EValue::HostHandle { kind, id, .. } if kind == "channel" => {
            let registry = process_authoring
                .lock()
                .map_err(|_| "failed to lock process registry".to_string())?;
            let name = registry
                .channel_handles
                .get(id)
                .cloned()
                .ok_or_else(|| "unknown channel handle".to_string())?;
            Ok(crate::process::ProcessSourceRef::Channel(name))
        }
        EValue::String(_) | EValue::Symbol(_) | EValue::Keyword(_) => Ok(
            crate::process::ProcessSourceRef::Channel(process_symbol_name(value)?),
        ),
        EValue::List(items) => {
            let items = items
                .iter()
                .map(|item| item.borrow().clone())
                .collect::<Vec<_>>();
            let head = process_symbol_name(
                items
                    .first()
                    .ok_or_else(|| "source expression missing head".to_string())?,
            )?;
            match head.as_str() {
                "chan" | "channel" => {
                    let name = process_symbol_name(
                        items
                            .get(1)
                            .ok_or_else(|| "channel source expects a name".to_string())?,
                    )?;
                    Ok(crate::process::ProcessSourceRef::Channel(name))
                }
                process_name => {
                    if items.len() != 2 {
                        return Err(
                            "process outlet source expects (process-name :outlet)".to_string()
                        );
                    }
                    let outlet = process_symbol_name(&items[1])?;
                    let registry = process_authoring
                        .lock()
                        .map_err(|_| "failed to lock process registry".to_string())?;
                    let instance = registry
                        .instances
                        .iter()
                        .rev()
                        .find(|entry| entry.name.as_deref() == Some(process_name))
                        .ok_or_else(|| format!("unknown process instance {process_name}"))?;
                    Ok(crate::process::ProcessSourceRef::Outlet(
                        crate::process::ProcessOutletRef {
                            process_handle_id: instance.handle_id,
                            outlet,
                        },
                    ))
                }
            }
        }
        _ => Err("source must be a process outlet or channel".to_string()),
    }
}

pub(in crate::lisp_host) fn parse_process_event_source_ref(
    process_authoring: &SharedProcessAuthoring,
    value: &EValue,
) -> Result<crate::process::ProcessEventSource, String> {
    if let Ok(source) = parse_process_source_ref(process_authoring, value) {
        return Ok(match source {
            crate::process::ProcessSourceRef::Channel(name) => {
                crate::process::ProcessEventSource::Channel(name)
            }
            crate::process::ProcessSourceRef::Outlet(outlet) => {
                crate::process::ProcessEventSource::Outlet(outlet)
            }
            crate::process::ProcessSourceRef::TrackFires(track) => {
                crate::process::ProcessEventSource::TrackFires(track)
            }
            crate::process::ProcessSourceRef::SeqFires(name) => {
                crate::process::ProcessEventSource::SeqFires(name)
            }
        });
    }
    parse_process_event_source(value)
}

pub(in crate::lisp_host) fn parse_process_target_ref(
    process_authoring: &SharedProcessAuthoring,
    value: &EValue,
) -> Result<crate::process::ProcessTargetRef, String> {
    match value {
        EValue::HostHandle { kind, id, .. } if kind == "channel" => {
            let registry = process_authoring
                .lock()
                .map_err(|_| "failed to lock process registry".to_string())?;
            let name = registry
                .channel_handles
                .get(id)
                .cloned()
                .ok_or_else(|| "unknown channel handle".to_string())?;
            Ok(crate::process::ProcessTargetRef::Channel(name))
        }
        EValue::List(_) => {
            Err("explicit inlet target handles are not represented as lists".to_string())
        }
        _ => Err(
            "target must be a channel in v1 or an inlet patch through constructor nesting"
                .to_string(),
        ),
    }
}

pub(in crate::lisp_host) fn build_process_emit_event(args: &[EValue]) -> Result<EmittedAccumulatorEvent, String> {
    let mut idx = 0;
    if args
        .first()
        .is_some_and(|value| !matches!(value, EValue::Keyword(_)))
    {
        idx = 1;
    }
    let mut resolved = crate::generator::default_resolved();
    let mut track = None;
    let mut offset_beats = 0.0_f32;
    while idx < args.len() {
        let key = process_symbol_name(&args[idx])?.to_ascii_lowercase();
        idx += 1;
        let Some(value) = args.get(idx) else {
            return Err(format!("emit missing value for :{key}"));
        };
        match (key.as_str(), value) {
            ("track", EValue::Number(value)) => track = Some((*value).max(0.0) as usize),
            ("after", value) => {
                offset_beats = parse_process_time_expr(value)?.beats(&HashMap::new()) as f32
            }
            ("note" | "transpose", EValue::Number(value)) => resolved.transpose = *value as f32,
            ("vel" | "velocity", EValue::Number(value)) => resolved.velocity = *value as f32,
            ("duration" | "dur", EValue::Number(value)) => resolved.duration = *value as f32,
            ("speed", EValue::Number(value)) => resolved.speed = *value as f32,
            ("pan", EValue::Number(value)) => resolved.pan = *value as f32,
            ("chop", EValue::Number(value)) => resolved.chop = *value as f32,
            _ => {}
        }
        idx += 1;
    }
    Ok(EmittedAccumulatorEvent {
        offset_beats,
        track,
        resolved,
        chord: Vec::new(),
        chord_durations: Vec::new(),
        chord_step_transpose: 0.0,
        effect_params: Vec::new(),
        instrument_params: Vec::new(),
    })
}

pub(in crate::lisp_host) fn process_status_value(registry: &ProcessAuthoringRegistry) -> EValue {
    process_map([
        (
            "defs",
            process_list(registry.defs.iter().map(|def| {
                process_map([
                    ("id", EValue::Number(def.id as f64)),
                    ("name", EValue::String(def.name.clone())),
                    (
                        "inlets",
                        process_string_list(def.inlets.iter().map(|inlet| &inlet.name)),
                    ),
                    (
                        "outlets",
                        process_string_list(def.outlets.iter().map(|outlet| &outlet.name)),
                    ),
                    (
                        "state",
                        process_string_list(def.state.iter().map(|cell| &cell.name)),
                    ),
                    ("run-source", EValue::Bool(def.run_source.is_some())),
                ])
            })),
        ),
        (
            "instances",
            process_list(registry.instances.iter().map(|instance| {
                process_map([
                    ("id", EValue::Number(instance.handle_id.0 as f64)),
                    (
                        "name",
                        instance
                            .name
                            .as_ref()
                            .map(|name| EValue::String(name.clone()))
                            .unwrap_or(EValue::Nil),
                    ),
                    ("class", EValue::String(instance.class_name.clone())),
                    ("running", EValue::Bool(instance.running)),
                    ("anonymous", EValue::Bool(instance.anonymous)),
                    ("one-shot", EValue::Bool(instance.one_shot)),
                    (
                        "inlets",
                        process_list(instance.inlets.iter().map(|(name, value)| {
                            process_map([
                                ("name", EValue::String(name.clone())),
                                ("value", process_inlet_status_value(value)),
                            ])
                        })),
                    ),
                ])
            })),
        ),
        (
            "channels",
            process_list(registry.channels.iter().map(|channel| {
                process_map([
                    ("id", EValue::Number(channel.handle_id.0 as f64)),
                    (
                        "name",
                        channel
                            .name
                            .as_ref()
                            .map(|name| EValue::String(name.clone()))
                            .unwrap_or(EValue::Nil),
                    ),
                    ("message-only", EValue::Bool(channel.message_only)),
                    ("initial", channel.initial.clone().unwrap_or(EValue::Nil)),
                ])
            })),
        ),
        ("patches", EValue::Number(registry.patches.len() as f64)),
        (
            "listeners",
            EValue::Number(
                registry
                    .defs
                    .iter()
                    .map(|def| def.listens.len())
                    .sum::<usize>() as f64,
            ),
        ),
    ])
}

pub(in crate::lisp_host) fn process_inlet_status_value(value: &crate::process::ProcessInletValue) -> EValue {
    match value {
        crate::process::ProcessInletValue::Literal(value) => process_map([
            ("kind", EValue::Keyword("literal".to_string())),
            ("value", value.clone()),
        ]),
        crate::process::ProcessInletValue::Channel(name) => process_map([
            ("kind", EValue::Keyword("channel".to_string())),
            ("name", EValue::String(name.clone())),
        ]),
        crate::process::ProcessInletValue::Outlet(outlet) => process_map([
            ("kind", EValue::Keyword("outlet".to_string())),
            (
                "process-id",
                EValue::Number(outlet.process_handle_id.0 as f64),
            ),
            ("outlet", EValue::String(outlet.outlet.clone())),
        ]),
    }
}

pub(in crate::lisp_host) fn process_string_list<'a>(items: impl IntoIterator<Item = &'a String>) -> EValue {
    process_list(items.into_iter().map(|item| EValue::String(item.clone())))
}

pub(in crate::lisp_host) fn process_list(items: impl IntoIterator<Item = EValue>) -> EValue {
    EValue::List(
        items
            .into_iter()
            .map(|value| Rc::new(RefCell::new(value)))
            .collect(),
    )
}

pub(in crate::lisp_host) fn process_map(items: impl IntoIterator<Item = (&'static str, EValue)>) -> EValue {
    EValue::Map(
        items
            .into_iter()
            .map(|(key, value)| (key.to_string(), Rc::new(RefCell::new(value))))
            .collect(),
    )
}
