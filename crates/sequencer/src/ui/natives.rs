use super::*;

pub(crate) struct RuntimeInit {
    pub(crate) runtime: Runtime,
    pub(crate) accumulator_names: Arc<Mutex<Vec<String>>>,
    pub(crate) midi_fx_names: Arc<Mutex<Vec<String>>>,
    pub(crate) sample_browser: Rc<RefCell<DebouncedSampleBrowser>>,
    pub(crate) piano_roll_clipboard: PianoRollClipboard,
}

pub(super) fn register_factory_path_native(runtime: &mut Runtime) {
    runtime.register_native("seq-factory-path", |args, _ctx| {
        let Some(Value::String(relative)) = args.first() else {
            return Err("seq-factory-path expects one relative path string".to_string());
        };
        let relative = std::path::Path::new(&relative);
        if relative.components().any(|component| !matches!(
            component,
            std::path::Component::Normal(_)
        )) {
            return Err("seq-factory-path requires a normalized relative path".to_string());
        }
        Ok(Value::String(
            sequencer::app_paths::app_paths()
                .factory_root()
                .join(relative)
                .to_string_lossy()
                .into_owned(),
        ))
    });
    runtime.register_native("seq-project-content-path", |args, _ctx| {
        let Some(Value::String(path)) = args.first() else {
            return Err("seq-project-content-path expects one path string".to_string());
        };
        let path = std::path::Path::new(&path);
        let factory_root = sequencer::app_paths::app_paths().factory_root();
        let relative = if path.is_absolute() {
            match path.strip_prefix(&factory_root) {
                Ok(relative) => relative,
                // Paths outside factory content (user scripts, temp files)
                // have no portable content form: pass them through verbatim.
                Err(_) => {
                    return Ok(Value::String(path.to_string_lossy().into_owned()));
                }
            }
        } else {
            path.strip_prefix("content").unwrap_or(path)
        };
        if relative.components().any(|component| !matches!(
            component,
            std::path::Component::Normal(_)
        )) {
            return Err("seq-project-content-path requires a normalized content path".to_string());
        }
        Ok(Value::String(
            std::path::Path::new("content")
                .join(relative)
                .to_string_lossy()
                .into_owned(),
        ))
    });
}

fn value_number_field(value: &Value, field: &str) -> Option<usize> {
    let Value::Map(map) = value else {
        return None;
    };
    map.get(field).and_then(|cell| match &*cell.borrow() {
        Value::Number(n) if *n >= 0.0 => Some(*n as usize),
        _ => None,
    })
}

fn value_string_field(value: &Value, field: &str) -> Option<String> {
    let Value::Map(map) = value else {
        return None;
    };
    map.get(field).and_then(|cell| match &*cell.borrow() {
        Value::String(s) => Some(s.clone()),
        Value::Keyword(s) => Some(s.clone()),
        _ => None,
    })
}

fn slice3_numeric_history_command(op: &str, track: Option<usize>, value: f64) -> HostCommand {
    let mut payload = HashMap::new();
    payload.insert(
        "op".to_string(),
        Rc::new(RefCell::new(Value::Keyword(op.to_string()))),
    );
    if let Some(track) = track {
        payload.insert(
            "track".to_string(),
            Rc::new(RefCell::new(Value::Number(track as f64))),
        );
    }
    payload.insert(
        "value".to_string(),
        Rc::new(RefCell::new(Value::Number(value))),
    );
    HostCommand::Custom {
        name: "slice3-history-action".to_string(),
        payload: Value::Map(payload),
    }
}

fn bus_mixer_history_command(
    op: &str,
    bus: usize,
    bus_id: sequencer::sequencer::BusId,
    value: Option<f64>,
) -> HostCommand {
    let mut payload = HashMap::new();
    payload.insert(
        "op".to_string(),
        Rc::new(RefCell::new(Value::Keyword(op.to_string()))),
    );
    payload.insert(
        "bus".to_string(),
        Rc::new(RefCell::new(Value::Number(bus as f64))),
    );
    payload.insert(
        "bus-id".to_string(),
        Rc::new(RefCell::new(Value::String(bus_id.0.to_string()))),
    );
    if let Some(value) = value {
        payload.insert(
            "value".to_string(),
            Rc::new(RefCell::new(Value::Number(value))),
        );
    }
    HostCommand::Custom {
        name: "bus-mixer-history-action".to_string(),
        payload: Value::Map(payload),
    }
}

fn process_history_command(op: &str, fields: Vec<(&str, Value)>) -> HostCommand {
    let mut payload = HashMap::new();
    payload.insert(
        "op".to_string(),
        Rc::new(RefCell::new(Value::Keyword(op.to_string()))),
    );
    for (name, value) in fields {
        payload.insert(name.to_string(), Rc::new(RefCell::new(value)));
    }
    HostCommand::Custom {
        name: "process-history-action".to_string(),
        payload: Value::Map(payload),
    }
}

fn midi_fx_history_command(op: &str, track: usize, value: Value) -> HostCommand {
    let mut payload = HashMap::new();
    payload.insert(
        "op".to_string(),
        Rc::new(RefCell::new(Value::Keyword(op.to_string()))),
    );
    payload.insert(
        "track".to_string(),
        Rc::new(RefCell::new(Value::Number(track as f64))),
    );
    payload.insert("value".to_string(), Rc::new(RefCell::new(value)));
    HostCommand::Custom {
        name: "midi-fx-history-action".to_string(),
        payload: Value::Map(payload),
    }
}

fn effect_plock_history_command(
    track: usize,
    steps: &[usize],
    slot_idx: usize,
    updates: &[(usize, f32)],
) -> HostCommand {
    let step_values = steps
        .iter()
        .map(|step| Rc::new(RefCell::new(Value::Number(*step as f64))))
        .collect();
    let update_values = updates
        .iter()
        .map(|(param_idx, value)| {
            Rc::new(RefCell::new(Value::Map(HashMap::from([
                (
                    "param-idx".to_string(),
                    Rc::new(RefCell::new(Value::Number(*param_idx as f64))),
                ),
                (
                    "value".to_string(),
                    Rc::new(RefCell::new(Value::Number(*value as f64))),
                ),
            ]))))
        })
        .collect();
    let payload = HashMap::from([
        (
            "track".to_string(),
            Rc::new(RefCell::new(Value::Number(track as f64))),
        ),
        (
            "steps".to_string(),
            Rc::new(RefCell::new(Value::List(step_values))),
        ),
        (
            "slot-idx".to_string(),
            Rc::new(RefCell::new(Value::Number(slot_idx as f64))),
        ),
        (
            "updates".to_string(),
            Rc::new(RefCell::new(Value::List(update_values))),
        ),
        (
            "gesture".to_string(),
            Rc::new(RefCell::new(Value::String("custom-effect-control".to_string()))),
        ),
        (
            "label".to_string(),
            Rc::new(RefCell::new(Value::String("Set custom effect p-lock".to_string()))),
        ),
    ]);
    HostCommand::Custom {
        name: "set-effect-plock-batch".to_string(),
        payload: Value::Map(payload),
    }
}

fn value_symbol_name(value: &Value) -> Option<String> {
    match value {
        Value::String(value) | Value::Symbol(value) | Value::Keyword(value) => {
            Some(value.trim_start_matches(':').to_string())
        }
        _ => None,
    }
}

fn process_slot_port_def(
    state: &SequencerState,
    track: usize,
    instance_id: sequencer::process::ProcessInstanceId,
    port_name: &str,
) -> Option<sequencer::process::ProcessPortDef> {
    let Some(chain) = state.composed_track_process_chain(track) else {
        return None;
    };
    let Some(slot) = chain
        .slots
        .iter()
        .find(|slot| slot.instance_id == instance_id)
    else {
        return None;
    };
    state
        .published_process_authoring()
        .defs
        .iter()
        .find(|def| def.name == slot.class_name)
        .and_then(|def| def.ports.iter().find(|port| port.name == port_name))
        .cloned()
}

fn process_target_param_name(value: &Value) -> Option<String> {
    value_string_field(value, "param").or_else(|| value_string_field(value, "param-name"))
}

fn process_target_effect_name(value: &Value) -> Option<String> {
    value_string_field(value, "effect").or_else(|| value_string_field(value, "fx"))
}

fn descriptor_param_name(
    desc: Option<&sequencer::effects::EffectDescriptor>,
    param_idx: usize,
) -> Option<String> {
    desc.and_then(|desc| desc.params.get(param_idx))
        .map(|param| param.name.clone())
}

fn require_slot_param_index(
    param_idx: usize,
    num_params: usize,
    target: impl FnOnce() -> String,
) -> Result<(), String> {
    if param_idx < num_params {
        Ok(())
    } else {
        Err(format!(
            "{} param index {param_idx} is out of range ({num_params} params)",
            target()
        ))
    }
}

pub(super) fn param_target_from_value(
    state: &SequencerState,
    track: usize,
    value: &Value,
) -> Result<sequencer::process::ParamTarget, String> {
    let kind = value_string_field(value, "kind")
        .ok_or_else(|| "process target must include :kind".to_string())?;
    match kind.trim_start_matches(':') {
        "step-param" | "step" => {
            let param = value_string_field(value, "param")
                .ok_or_else(|| "step-param process target must include :param".to_string())?;
            Ok(sequencer::process::ParamTarget::StepParam { param })
        }
        "instrument" | "instrument-param" => {
            let param_idx = value_number_field(value, "param-idx")
                .ok_or_else(|| "instrument process target must include :param-idx".to_string())?;
            let slot =
                state.pattern.instrument_slots.get(track).ok_or_else(|| {
                    format!("instrument slot for track {} is not loaded", track + 1)
                })?;
            let num_params = slot.num_params.load(Ordering::Relaxed) as usize;
            require_slot_param_index(param_idx, num_params, || {
                format!("instrument track {}", track + 1)
            })?;
            let (_effect_descriptors, instrument_descriptors) = state.scratch_runtime_descriptors();
            let param = process_target_param_name(value)
                .or_else(|| descriptor_param_name(instrument_descriptors.get(track), param_idx))
                .ok_or_else(|| {
                    format!(
                        "instrument process target must include :param for track {} param index {param_idx}",
                        track + 1
                    )
                })?;
            Ok(sequencer::process::ParamTarget::InstrumentParam {
                param,
                param_id: slot.param_node_id(param_idx),
            })
        }
        "effect" | "effect-param" | "audio-fx" | "audio-effect" => {
            let slot_idx = value_number_field(value, "slot-idx")
                .or_else(|| value_number_field(value, "slot"))
                .ok_or_else(|| "effect process target must include :slot-idx".to_string())?;
            let param_idx = value_number_field(value, "param-idx")
                .ok_or_else(|| "effect process target must include :param-idx".to_string())?;
            let slot = state
                .pattern
                .effect_chains
                .get(track)
                .and_then(|chain| chain.get(slot_idx))
                .ok_or_else(|| {
                    format!(
                        "effect slot for track {} slot {} is not loaded",
                        track + 1,
                        slot_idx + 1
                    )
                })?;
            let num_params = slot.num_params.load(Ordering::Relaxed) as usize;
            require_slot_param_index(param_idx, num_params, || {
                format!("effect track {} slot {}", track + 1, slot_idx + 1)
            })?;
            let (effect_descriptors, _instrument_descriptors) = state.scratch_runtime_descriptors();
            let desc = effect_descriptors
                .get(track)
                .and_then(|descs| descs.get(slot_idx));
            let effect = process_target_effect_name(value)
                .or_else(|| desc.map(|desc| desc.name.clone()))
                .ok_or_else(|| {
                    format!(
                        "effect process target must include :effect for track {} slot {}",
                        track + 1,
                        slot_idx + 1
                    )
                })?;
            let param = process_target_param_name(value)
                .or_else(|| descriptor_param_name(desc, param_idx))
                .ok_or_else(|| {
                    format!(
                        "effect process target must include :param for track {} slot {} param index {param_idx}",
                        track + 1,
                        slot_idx + 1
                    )
                })?;
            Ok(sequencer::process::ParamTarget::EffectParam {
                slot: slot_idx,
                effect,
                param,
                param_id: slot.param_node_id(param_idx),
            })
        }
        "midi-fx" | "midi-fx-param" | "midi-effect" => {
            let slot_idx = value_number_field(value, "slot-idx")
                .or_else(|| value_number_field(value, "slot"))
                .ok_or_else(|| "midi-fx process target must include :slot-idx".to_string())?;
            let param_idx = value_number_field(value, "param-idx")
                .ok_or_else(|| "midi-fx process target must include :param-idx".to_string())?;
            let chain_fx_name = state
                .pattern
                .track_params
                .get(track)
                .and_then(|params| params.midi_fx_chain().get(slot_idx).cloned())
                .ok_or_else(|| {
                    format!(
                        "MIDI-FX slot {} is not loaded on track {}",
                        slot_idx + 1,
                        track + 1
                    )
                })?;
            let fx = value_string_field(value, "fx").unwrap_or(chain_fx_name);
            let desc = sequencer::lisp_host::load_midi_fx_descriptor(&fx)
                .ok_or_else(|| format!("MIDI-FX descriptor for {fx} is not loaded"))?;
            require_slot_param_index(param_idx, desc.params.len(), || format!("MIDI-FX {fx}"))?;
            let param = process_target_param_name(value)
                .or_else(|| descriptor_param_name(Some(&desc), param_idx))
                .ok_or_else(|| {
                    format!("midi-fx process target must include :param for {fx} param index {param_idx}")
                })?;
            Ok(sequencer::process::ParamTarget::MidiFxParam {
                slot: slot_idx,
                fx: desc.name,
                param,
            })
        }
        "process-inlet" | "process_inlet" => {
            let process = value_string_field(value, "process")
                .or_else(|| value_string_field(value, "class"))
                .ok_or_else(|| "process-inlet target must include :process".to_string())?;
            let inlet = value_string_field(value, "inlet")
                .ok_or_else(|| "process-inlet target must include :inlet".to_string())?;
            let instance_id = value_number_field(value, "instance-id")
                .or_else(|| value_number_field(value, "instance_id"))
                .map(|id| sequencer::process::ProcessInstanceId(id as u64));
            Ok(sequencer::process::ParamTarget::ProcessInlet {
                process,
                inlet,
                instance_id,
            })
        }
        "rack-slot" | "rack-slot-param" | "rack-instrument" | "rack-slot-instrument-param" => Err(
            "rack process-port bindings are not exposed until rack dispatch supports them"
                .to_string(),
        ),
        "bus-effect" | "bus-fx" => {
            Err("bus FX process-port bindings are not supported".to_string())
        }
        other => Err(format!("unknown process target kind :{other}")),
    }
}

fn nonnegative_usize_arg(name: &str, value: f64) -> Result<usize, String> {
    if !value.is_finite() || value < 0.0 || value.fract() != 0.0 {
        return Err(format!("{name} must be a non-negative integer"));
    }
    Ok(value as usize)
}

fn expanded_step_viewport_from_numbers(
    track: f64,
    track_id: f64,
    page: f64,
    mode: f64,
    cursor_step: f64,
) -> Result<ExpandedStepViewport, String> {
    let max_page = MAX_STEPS.saturating_sub(1) / PAGE_SIZE;
    Ok(ExpandedStepViewport {
        track: nonnegative_usize_arg("track", track)?
            .min(sequencer::sequencer::MAX_TRACKS.saturating_sub(1)),
        track_id: nonnegative_usize_arg("track-id", track_id)?,
        page: nonnegative_usize_arg("page", page)?.min(max_page),
        mode: nonnegative_usize_arg("mode", mode)?,
        cursor_step: nonnegative_usize_arg("cursor-step", cursor_step)?
            .min(MAX_STEPS.saturating_sub(1)),
    })
}

fn value_string_list(value: Option<&Value>) -> Vec<String> {
    let Some(Value::List(items)) = value else {
        return Vec::new();
    };
    items
        .iter()
        .filter_map(|item| match &*item.borrow() {
            Value::String(value) | Value::Keyword(value) => Some(value.trim().to_string()),
            _ => None,
        })
        .filter(|value| !value.is_empty())
        .collect()
}

fn delete_kind_name(value: &Value) -> Option<&str> {
    match value {
        Value::Keyword(kind) | Value::String(kind) => Some(kind.as_str()),
        _ => None,
    }
}

fn trace_ui_enabled() -> bool {
    std::env::var_os("ESEQLISP_TRACE_UI").is_some()
}

fn register_ui_def_accumulator_dispatch(
    runtime: &mut Runtime,
    accumulator_names: Arc<Mutex<Vec<String>>>,
    process_authoring: sequencer::lisp_host::PublishedProcessAuthoringNatives,
    debug_accum: bool,
) {
    runtime.register_vm_native_with_docs(
        "def-accumulator",
        "(def-accumulator name body) | (def-accumulator name :target (step-param :transpose) :amount (...))",
        "Define either a legacy script accumulator preview or a published process accumulator.",
        move |args, vm| {
            let is_legacy_script_form =
                args.len() == 2 && !matches!(args.get(1), Some(Value::Keyword(_)));
            if is_legacy_script_form {
                let label = match args.first() {
                    Some(Value::String(name) | Value::Symbol(name) | Value::Keyword(name)) => name
                        .trim_start_matches(':')
                        .trim_start_matches('@')
                        .to_string(),
                    _ => {
                        eprintln!("[accum-ui] def-accumulator error: expected accumulator name");
                        return Value::Bool(false);
                    }
                };
                let mut names = accumulator_names.lock().unwrap();
                if !names.iter().any(|name| name.eq_ignore_ascii_case(&label)) {
                    names.push(label.clone());
                }
                if debug_accum {
                    eprintln!("[accum-ui] preview register label={label} names={names:?}");
                }
                return Value::String(label);
            }

            match process_authoring.define_process_accumulator(args, vm) {
                Ok(value) => value,
                Err(error) => {
                    eprintln!("[process] def-accumulator error: {error}");
                    Value::Bool(false)
                }
            }
        },
    );
}

fn song_payload(fields: Vec<(&str, Value)>) -> Value {
    let mut payload = HashMap::new();
    for (name, value) in fields {
        payload.insert(name.to_string(), Rc::new(RefCell::new(value)));
    }
    Value::Map(payload)
}

fn song_beat_arg(name: &str, value: Option<&Value>) -> Result<f64, String> {
    match value {
        Some(Value::Number(beat)) if beat.is_finite() && *beat >= 0.0 => Ok(*beat),
        _ => Err(format!("{name} must be a finite, non-negative beat number")),
    }
}

/// A parsed `(track pattern-id)` pair from a song overrides/patterns list.
/// A pattern-id of `nil` or `0` is an explicit-empty override (the track
/// plays nothing for the row).
fn song_override_pair(value: &Value) -> Result<(usize, Option<u64>), String> {
    let Value::List(items) = value else {
        return Err("expected a (track pattern-id) pair".to_string());
    };
    if items.len() != 2 {
        return Err("expected a (track pattern-id) pair".to_string());
    }
    let track = match &*items[0].borrow() {
        Value::Number(n) => *n,
        _ => return Err("override track must be a number".to_string()),
    };
    if !track.is_finite() || track < 0.0 || track.fract() != 0.0 {
        return Err("override track must be a non-negative integer".to_string());
    }
    let pattern = match &*items[1].borrow() {
        Value::Nil => None,
        Value::Number(n) if *n == 0.0 => None,
        Value::Number(n) if n.is_finite() && *n >= 1.0 && n.fract() == 0.0 => {
            Some(*n as u64)
        }
        _ => {
            return Err(
                "override pattern-id must be a positive integer, or 0/nil for \
                 explicit-empty"
                    .to_string(),
            )
        }
    };
    Ok((track as usize, pattern))
}

/// Parse an optional overrides argument: nil/absent, or a list of
/// `(track pattern-id)` pairs.
fn song_overrides_arg(value: Option<&Value>) -> Result<Vec<(usize, Option<u64>)>, String> {
    match value {
        None | Some(Value::Nil) => Ok(Vec::new()),
        Some(Value::List(items)) => items
            .iter()
            .map(|item| song_override_pair(&item.borrow()))
            .collect(),
        _ => Err("overrides must be a list of (track pattern-id) pairs".to_string()),
    }
}

fn song_overrides_value(overrides: &[(usize, Option<u64>)]) -> Value {
    Value::List(
        overrides
            .iter()
            .map(|(track, pattern)| {
                let pattern_value = match pattern {
                    Some(id) => Value::Number(*id as f64),
                    None => Value::Nil,
                };
                Rc::new(RefCell::new(Value::List(vec![
                    Rc::new(RefCell::new(Value::Number(*track as f64))),
                    Rc::new(RefCell::new(pattern_value)),
                ])))
            })
            .collect(),
    )
}

fn song_row_id_arg(name: &str, value: Option<&Value>) -> Result<u64, String> {
    match value {
        Some(Value::Number(id)) if id.is_finite() && *id >= 0.0 && id.fract() == 0.0 => {
            Ok(*id as u64)
        }
        _ => Err(format!("{name} must be a non-negative integer row id")),
    }
}

fn song_track_arg(name: &str, value: Option<&Value>) -> Result<usize, String> {
    match value {
        Some(Value::Number(track))
            if track.is_finite() && *track >= 0.0 && track.fract() == 0.0 =>
        {
            Ok(*track as usize)
        }
        _ => Err(format!("{name} must be a non-negative integer track index")),
    }
}

fn song_scene_arg(name: &str, value: Option<&Value>) -> Result<usize, String> {
    match value {
        Some(Value::Number(scene))
            if scene.is_finite() && *scene >= 0.0 && scene.fract() == 0.0 =>
        {
            Ok(*scene as usize)
        }
        _ => Err(format!("{name} must be a non-negative integer scene index")),
    }
}

/// One parsed `(at <beat> :scene n [:patterns ((track pat)...)])` row of a
/// `def-song` definition (docs/song-mode-spec.md section 6).
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct DefSongRow {
    pub(crate) start_beat: f64,
    pub(crate) scene: usize,
    pub(crate) patterns: Vec<(usize, Option<u64>)>,
}

/// A fully parsed `def-song` definition, validated for shape before any host
/// command is emitted. Model-level validation (scene/pattern existence,
/// ordering, end > last start) happens in `App::song_replace`.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct DefSongSpec {
    pub(crate) name: String,
    pub(crate) rows: Vec<DefSongRow>,
    pub(crate) end_beat: f64,
    pub(crate) loop_enabled: bool,
}

fn def_song_bool(value: &Value) -> Result<bool, String> {
    match value {
        Value::Bool(flag) => Ok(*flag),
        Value::Symbol(s) | Value::Keyword(s) | Value::String(s) => match s.trim_start_matches(':') {
            "true" | "on" | "yes" => Ok(true),
            "false" | "off" | "no" | "nil" => Ok(false),
            other => Err(format!(":loop expects true/false, got {other}")),
        },
        Value::Number(n) => Ok(*n != 0.0),
        Value::Nil => Ok(false),
        _ => Err(":loop expects true/false".to_string()),
    }
}

fn parse_def_song_row(items: &[Rc<RefCell<Value>>]) -> Result<DefSongRow, String> {
    let start_beat = {
        let beat = items.get(1).map(|cell| cell.borrow().clone());
        song_beat_arg("(at <beat> ...)", beat.as_ref())?
    };
    let mut scene: Option<usize> = None;
    let mut patterns: Vec<(usize, Option<u64>)> = Vec::new();
    let mut idx = 2;
    while idx < items.len() {
        let key = match &*items[idx].borrow() {
            Value::Keyword(key) | Value::Symbol(key) | Value::String(key) => {
                key.trim_start_matches(':').to_ascii_lowercase()
            }
            other => {
                return Err(format!(
                    "row at beat {start_beat}: expected a keyword, got {other:?}"
                ))
            }
        };
        let Some(value) = items.get(idx + 1) else {
            return Err(format!("row at beat {start_beat}: missing value for :{key}"));
        };
        let value = value.borrow();
        match key.as_str() {
            "scene" => {
                scene = Some(song_scene_arg(
                    &format!("row at beat {start_beat}: :scene"),
                    Some(&value),
                )?);
            }
            "patterns" => {
                patterns = song_overrides_arg(Some(&value))
                    .map_err(|error| format!("row at beat {start_beat}: :patterns {error}"))?;
            }
            other => {
                return Err(format!(
                    "row at beat {start_beat}: unknown key :{other} (expected :scene or :patterns)"
                ));
            }
        }
        idx += 2;
    }
    let Some(scene) = scene else {
        return Err(format!("row at beat {start_beat}: missing :scene"));
    };
    Ok(DefSongRow {
        start_beat,
        scene,
        patterns,
    })
}

/// Parse the complete `def-song` argument list:
/// `(def-song "name" (at <beat> :scene n [:patterns ((track pat)...)])... :end <beat> [:loop bool])`.
/// All-or-nothing: any malformed element fails the whole definition.
pub(crate) fn parse_def_song_args(args: &[Value]) -> Result<DefSongSpec, String> {
    let name = match args.first() {
        Some(Value::String(name) | Value::Symbol(name)) if !name.trim().is_empty() => {
            name.trim().to_string()
        }
        _ => return Err("expected a song name string as the first argument".to_string()),
    };
    let mut rows = Vec::new();
    let mut end_beat: Option<f64> = None;
    let mut loop_enabled = false;
    let mut idx = 1;
    while idx < args.len() {
        match &args[idx] {
            Value::List(items) => {
                let head = items.first().map(|cell| cell.borrow().clone());
                let is_at_form = matches!(
                    &head,
                    Some(Value::Symbol(s) | Value::String(s) | Value::Keyword(s))
                        if s.trim_start_matches(':').eq_ignore_ascii_case("at")
                );
                if !is_at_form {
                    return Err(
                        "rows must be (at <beat> :scene n [:patterns ((track pat)...)]) forms"
                            .to_string(),
                    );
                }
                rows.push(parse_def_song_row(items)?);
                idx += 1;
            }
            Value::Keyword(key) | Value::Symbol(key) => {
                let key = key.trim_start_matches(':').to_ascii_lowercase();
                let Some(value) = args.get(idx + 1) else {
                    return Err(format!("missing value for :{key}"));
                };
                match key.as_str() {
                    "end" => end_beat = Some(song_beat_arg(":end", Some(value))?),
                    "loop" => loop_enabled = def_song_bool(value)?,
                    other => {
                        return Err(format!("unknown key :{other} (expected :end or :loop)"));
                    }
                }
                idx += 2;
            }
            other => {
                return Err(format!(
                    "unexpected argument {other:?}; expected (at ...) rows, :end, or :loop"
                ));
            }
        }
    }
    if rows.is_empty() {
        return Err("a song needs at least one (at <beat> :scene n) row".to_string());
    }
    let Some(end_beat) = end_beat else {
        return Err("missing :end <beat>".to_string());
    };
    Ok(DefSongSpec {
        name,
        rows,
        end_beat,
        loop_enabled,
    })
}

/// Lower a parsed `def-song` definition to the `arrangement-replace`
/// host-command payload. The payload is still a declarative *row* list; the
/// row → lane inverse compile happens in `App::arr_replace_rows`
/// (docs/arrangement-lane-model-spec.md 8), which needs the project's
/// timebases and so cannot run in the native.
pub(crate) fn def_song_replace_payload(spec: &DefSongSpec) -> Value {
    let rows = Value::List(
        spec.rows
            .iter()
            .map(|row| {
                Rc::new(RefCell::new(song_payload(vec![
                    ("start-beat", Value::Number(row.start_beat)),
                    ("scene", Value::Number(row.scene as f64)),
                    ("overrides", song_overrides_value(&row.patterns)),
                ])))
            })
            .collect(),
    );
    song_payload(vec![
        ("name", Value::String(spec.name.clone())),
        ("rows", rows),
        ("end-beat", Value::Number(spec.end_beat)),
        ("loop", Value::Bool(spec.loop_enabled)),
    ])
}

/// Song-mode natives (docs/song-mode-spec.md 6/12): `seq-song-*` wrappers for
/// every editing primitive plus declarative `def-song`. Each enqueues one
/// custom host command handled by `ui/host_commands/song.rs`; success and
/// failure surface on the status line there.
/// `seq-toggle-play` — Play/Stop route through the song transport state
/// machine (docs/song-mode-spec.md 12/13): the host command handler in
/// host_commands/song.rs applies the transition on App so Use Arrangement
/// and record select session playback, song playback, or capture.
pub(crate) fn register_transport_toggle_play_native(
    runtime: &mut Runtime,
    state: Arc<SequencerState>,
) {
    runtime.register_native("seq-toggle-play", move |_args, ctx| {
        ctx.enqueue_command(HostCommand::Custom {
            name: "song-transport-toggle-play".to_string(),
            payload: Value::Nil,
        });
        Ok(Value::Bool(!state.is_playing()))
    });
}

pub(crate) fn register_song_natives(runtime: &mut Runtime) {
    // Arrangement editing primitives (docs/arrangement-lane-model-spec.md 8).
    // Scene-lane ops address a scene change by its beat; clip ops address a
    // clip by its stable id.
    runtime.register_native_with_docs(
        "seq-arrangement-scene-insert",
        "(seq-arrangement-scene-insert beat scene)",
        "Insert a scene change at an absolute beat. Clips are untouched: a \
         clip is opaque while it spans a beat, so this only changes what the \
         uncovered lanes play.",
        move |args, ctx| {
            let beat = song_beat_arg("seq-arrangement-scene-insert: beat", args.first())?;
            let scene = song_scene_arg("seq-arrangement-scene-insert: scene", args.get(1))?;
            ctx.enqueue_command(HostCommand::Custom {
                name: "arrangement-scene-insert".to_string(),
                payload: song_payload(vec![
                    ("beat", Value::Number(beat)),
                    ("scene", Value::Number(scene as f64)),
                ]),
            });
            Ok(Value::Bool(true))
        },
    );

    runtime.register_native_with_docs(
        "seq-arrangement-scene-remove",
        "(seq-arrangement-scene-remove beat)",
        "Remove the scene change at an absolute beat; the predecessor's scene \
         extends over it. Removing the change at beat 0.0 is rejected, and no \
         clip is ever touched.",
        move |args, ctx| {
            let beat = song_beat_arg("seq-arrangement-scene-remove: beat", args.first())?;
            ctx.enqueue_command(HostCommand::Custom {
                name: "arrangement-scene-remove".to_string(),
                payload: song_payload(vec![("beat", Value::Number(beat))]),
            });
            Ok(Value::Bool(true))
        },
    );

    runtime.register_native_with_docs(
        "seq-arrangement-scene-move",
        "(seq-arrangement-scene-move from-beat to-beat)",
        "Move a scene change to a new absolute beat. Moving the change at \
         beat 0.0, or onto another change's beat, is rejected.",
        move |args, ctx| {
            let from_beat = song_beat_arg("seq-arrangement-scene-move: from-beat", args.first())?;
            let to_beat = song_beat_arg("seq-arrangement-scene-move: to-beat", args.get(1))?;
            ctx.enqueue_command(HostCommand::Custom {
                name: "arrangement-scene-move".to_string(),
                payload: song_payload(vec![
                    ("from-beat", Value::Number(from_beat)),
                    ("to-beat", Value::Number(to_beat)),
                ]),
            });
            Ok(Value::Bool(true))
        },
    );

    runtime.register_native_with_docs(
        "seq-arrangement-scene-set",
        "(seq-arrangement-scene-set beat scene)",
        "Point the scene change at an absolute beat at a different scene.",
        move |args, ctx| {
            let beat = song_beat_arg("seq-arrangement-scene-set: beat", args.first())?;
            let scene = song_scene_arg("seq-arrangement-scene-set: scene", args.get(1))?;
            ctx.enqueue_command(HostCommand::Custom {
                name: "arrangement-scene-set".to_string(),
                payload: song_payload(vec![
                    ("beat", Value::Number(beat)),
                    ("scene", Value::Number(scene as f64)),
                ]),
            });
            Ok(Value::Bool(true))
        },
    );

    runtime.register_native_with_docs(
        "seq-arrangement-clip-create",
        "(seq-arrangement-clip-create track start-beat end-beat [pattern-id] \
          [offset-steps])",
        "Create a clip on a track lane, truncating whatever it lands on \
         (Ableton-style: the incoming clip always wins). pattern-id nil or 0 \
         asks for silence over the span, which is an ABSENCE of clips: the \
         span is cleared instead of filled.",
        move |args, ctx| {
            let track = song_track_arg("seq-arrangement-clip-create: track", args.first())?;
            let start_beat =
                song_beat_arg("seq-arrangement-clip-create: start-beat", args.get(1))?;
            let end_beat = song_beat_arg("seq-arrangement-clip-create: end-beat", args.get(2))?;
            let pattern_id = match args.get(3) {
                None | Some(Value::Nil) => Value::Nil,
                Some(Value::Number(id)) => Value::Number(*id),
                Some(_) => {
                    return Err(
                        "seq-arrangement-clip-create: pattern-id must be a number or nil".into(),
                    )
                }
            };
            let offset_steps = match args.get(4) {
                None | Some(Value::Nil) => 0.0,
                Some(Value::Number(offset)) if offset.is_finite() && *offset >= 0.0 => *offset,
                Some(_) => {
                    return Err(
                        "seq-arrangement-clip-create: offset-steps must be a non-negative number"
                            .into(),
                    )
                }
            };
            ctx.enqueue_command(HostCommand::Custom {
                name: "arrangement-clip-create".to_string(),
                payload: song_payload(vec![
                    ("track", Value::Number(track as f64)),
                    ("start-beat", Value::Number(start_beat)),
                    ("end-beat", Value::Number(end_beat)),
                    ("pattern-id", pattern_id),
                    ("offset-steps", Value::Number(offset_steps)),
                ]),
            });
            Ok(Value::Bool(true))
        },
    );

    runtime.register_native_with_docs(
        "seq-arrangement-empty-take-create",
        "(seq-arrangement-empty-take-create track start-beat end-beat)",
        "Create a silent take-backed clip on an arrangement track, selecting \
         it as the edit focus. The take, clip, and any song-end extension are \
         committed as one undo entry.",
        move |args, ctx| {
            let track =
                song_track_arg("seq-arrangement-empty-take-create: track", args.first())?;
            let start_beat = song_beat_arg(
                "seq-arrangement-empty-take-create: start-beat",
                args.get(1),
            )?;
            let end_beat =
                song_beat_arg("seq-arrangement-empty-take-create: end-beat", args.get(2))?;
            ctx.enqueue_command(HostCommand::Custom {
                name: "arrangement-empty-take-create".to_string(),
                payload: song_payload(vec![
                    ("track", Value::Number(track as f64)),
                    ("start-beat", Value::Number(start_beat)),
                    ("end-beat", Value::Number(end_beat)),
                ]),
            });
            Ok(Value::Bool(true))
        },
    );

    runtime.register_native_with_docs(
        "seq-arrangement-clip-delete",
        "(seq-arrangement-clip-delete clip-id)",
        "Delete a clip by stable id. Nothing plays over its span afterwards: \
         the clip is gone from the timeline.",
        move |args, ctx| {
            let clip_id = song_row_id_arg("seq-arrangement-clip-delete: clip-id", args.first())?;
            ctx.enqueue_command(HostCommand::Custom {
                name: "arrangement-clip-delete".to_string(),
                payload: song_payload(vec![("clip-id", Value::Number(clip_id as f64))]),
            });
            Ok(Value::Bool(true))
        },
    );

    runtime.register_native_with_docs(
        "seq-arrangement-clip-move",
        "(seq-arrangement-clip-move clip-id new-start-beat)",
        "Move a clip rigidly: same length, same phase anchor, later or \
         earlier. It truncates whatever it lands on.",
        move |args, ctx| {
            let clip_id = song_row_id_arg("seq-arrangement-clip-move: clip-id", args.first())?;
            let start_beat = song_beat_arg("seq-arrangement-clip-move: new-start-beat", args.get(1))?;
            ctx.enqueue_command(HostCommand::Custom {
                name: "arrangement-clip-move".to_string(),
                payload: song_payload(vec![
                    ("clip-id", Value::Number(clip_id as f64)),
                    ("start-beat", Value::Number(start_beat)),
                ]),
            });
            Ok(Value::Bool(true))
        },
    );

    runtime.register_native_with_docs(
        "seq-arrangement-clip-resize",
        "(seq-arrangement-clip-resize clip-id start-beat end-beat)",
        "Resize a clip. Trimming the left edge re-stamps the phase anchor so \
         the clip keeps playing what it played there; the right edge is pure \
         occlusion, clamped to a take's playable length.",
        move |args, ctx| {
            let clip_id = song_row_id_arg("seq-arrangement-clip-resize: clip-id", args.first())?;
            let start_beat = song_beat_arg("seq-arrangement-clip-resize: start-beat", args.get(1))?;
            let end_beat = song_beat_arg("seq-arrangement-clip-resize: end-beat", args.get(2))?;
            ctx.enqueue_command(HostCommand::Custom {
                name: "arrangement-clip-resize".to_string(),
                payload: song_payload(vec![
                    ("clip-id", Value::Number(clip_id as f64)),
                    ("start-beat", Value::Number(start_beat)),
                    ("end-beat", Value::Number(end_beat)),
                ]),
            });
            Ok(Value::Bool(true))
        },
    );

    runtime.register_native_with_docs(
        "seq-arrangement-clip-split",
        "(seq-arrangement-clip-split clip-id beat)",
        "Split a clip at an absolute beat into two clips playing the same \
         uninterrupted music.",
        move |args, ctx| {
            let clip_id = song_row_id_arg("seq-arrangement-clip-split: clip-id", args.first())?;
            let beat = song_beat_arg("seq-arrangement-clip-split: beat", args.get(1))?;
            ctx.enqueue_command(HostCommand::Custom {
                name: "arrangement-clip-split".to_string(),
                payload: song_payload(vec![
                    ("clip-id", Value::Number(clip_id as f64)),
                    ("beat", Value::Number(beat)),
                ]),
            });
            Ok(Value::Bool(true))
        },
    );

    runtime.register_native_with_docs(
        "seq-arrangement-clip-set-source",
        "(seq-arrangement-clip-set-source clip-id [pattern-id])",
        "Swap a clip's content in place, keeping its span and identity. The \
         phase anchor resets to the new source's step 0.",
        move |args, ctx| {
            let clip_id =
                song_row_id_arg("seq-arrangement-clip-set-source: clip-id", args.first())?;
            let pattern_id = match args.get(1) {
                None | Some(Value::Nil) => Value::Nil,
                Some(Value::Number(id)) => Value::Number(*id),
                Some(_) => {
                    return Err(
                        "seq-arrangement-clip-set-source: pattern-id must be a number or nil"
                            .into(),
                    )
                }
            };
            ctx.enqueue_command(HostCommand::Custom {
                name: "arrangement-clip-set-source".to_string(),
                payload: song_payload(vec![
                    ("clip-id", Value::Number(clip_id as f64)),
                    ("pattern-id", pattern_id),
                ]),
            });
            Ok(Value::Bool(true))
        },
    );

    runtime.register_native_with_docs(
        "seq-arrangement-clear",
        "(seq-arrangement-clear)",
        "Remove the committed arrangement (and with it the compiled song) \
         entirely.",
        move |_args, ctx| {
            ctx.enqueue_command(HostCommand::Custom {
                name: "arrangement-clear".to_string(),
                payload: Value::Nil,
            });
            Ok(Value::Bool(true))
        },
    );

    runtime.register_native_with_docs(
        "seq-song-set-end",
        "(seq-song-set-end end-beat)",
        "Set the song's explicit end beat (must stay greater than the last \
         row's start beat).",
        move |args, ctx| {
            let end_beat = song_beat_arg("seq-song-set-end: end-beat", args.first())?;
            ctx.enqueue_command(HostCommand::Custom {
                name: "song-set-end".to_string(),
                payload: song_payload(vec![("end-beat", Value::Number(end_beat))]),
            });
            Ok(Value::Bool(true))
        },
    );

    runtime.register_native_with_docs(
        "seq-song-set-loop",
        "(seq-song-set-loop enabled)",
        "Enable or disable song looping.",
        move |args, ctx| {
            let enabled = match args.first() {
                Some(value) => def_song_bool(value)
                    .map_err(|_| "seq-song-set-loop: expected true/false".to_string())?,
                None => return Err("seq-song-set-loop: expected true/false".into()),
            };
            ctx.enqueue_command(HostCommand::Custom {
                name: "song-set-loop".to_string(),
                payload: song_payload(vec![("enabled", Value::Bool(enabled))]),
            });
            Ok(Value::Bool(true))
        },
    );

    runtime.register_native_with_docs(
        "seq-song-replace",
        "(seq-song-replace rows end-beat [loop])",
        "Replace the song wholesale. rows is a list of \
         (start-beat scene [((track pattern-id)...)]) lists; fresh row ids are \
         allocated and the whole replacement is one undo entry.",
        move |args, ctx| {
            let Some(Value::List(rows)) = args.first() else {
                return Err("seq-song-replace: expected a list of rows".into());
            };
            let mut row_values = Vec::with_capacity(rows.len());
            for row in rows {
                let row = row.borrow();
                let Value::List(items) = &*row else {
                    return Err(
                        "seq-song-replace: each row must be (start-beat scene [overrides])".into(),
                    );
                };
                let start = items.first().map(|cell| cell.borrow().clone());
                let start_beat = song_beat_arg("seq-song-replace: row start-beat", start.as_ref())?;
                let scene = items.get(1).map(|cell| cell.borrow().clone());
                let scene = song_scene_arg("seq-song-replace: row scene", scene.as_ref())?;
                let overrides = items.get(2).map(|cell| cell.borrow().clone());
                let overrides = song_overrides_arg(overrides.as_ref())
                    .map_err(|error| format!("seq-song-replace: {error}"))?;
                row_values.push(Rc::new(RefCell::new(song_payload(vec![
                    ("start-beat", Value::Number(start_beat)),
                    ("scene", Value::Number(scene as f64)),
                    ("overrides", song_overrides_value(&overrides)),
                ]))));
            }
            let end_beat = song_beat_arg("seq-song-replace: end-beat", args.get(1))?;
            let loop_enabled = match args.get(2) {
                Some(value) => def_song_bool(value)
                    .map_err(|error| format!("seq-song-replace: {error}"))?,
                None => false,
            };
            ctx.enqueue_command(HostCommand::Custom {
                name: "song-replace".to_string(),
                payload: song_payload(vec![
                    ("rows", Value::List(row_values)),
                    ("end-beat", Value::Number(end_beat)),
                    ("loop", Value::Bool(loop_enabled)),
                ]),
            });
            Ok(Value::Bool(true))
        },
    );

    runtime.register_native_with_docs(
        "seq-song-clear",
        "(seq-song-clear)",
        "Remove the committed song entirely.",
        move |_args, ctx| {
            ctx.enqueue_command(HostCommand::Custom {
                name: "song-clear".to_string(),
                payload: Value::Nil,
            });
            Ok(Value::Bool(true))
        },
    );

    // Transport-facing song commands (spec 12/13): each routes through the
    // song transport state machine in app/song_transport.rs via the host
    // command handler; rejections surface on the status line there.
    runtime.register_native_with_docs(
        "seq-song-play",
        "(seq-song-play)",
        "Play through the song transport state machine: session playback, \
         song playback, or arrangement capture per Use Arrangement + record \
         (docs/song-mode-spec.md section 1). Errors if already playing.",
        move |_args, ctx| {
            ctx.enqueue_command(HostCommand::Custom {
                name: "song-transport-play".to_string(),
                payload: Value::Nil,
            });
            Ok(Value::Bool(true))
        },
    );

    runtime.register_native_with_docs(
        "seq-song-capture-arm",
        "(seq-song-capture-arm armed)",
        "Arm or disarm arrangement capture for the next Play (Use \
         Arrangement + record selects capture). Only changes while stopped.",
        move |args, ctx| {
            let armed = match args.first() {
                Some(value) => def_song_bool(value)
                    .map_err(|_| "seq-song-capture-arm: expected true/false".to_string())?,
                None => return Err("seq-song-capture-arm: expected true/false".into()),
            };
            ctx.enqueue_command(HostCommand::Custom {
                name: "song-capture-arm".to_string(),
                payload: song_payload(vec![("armed", Value::Bool(armed))]),
            });
            Ok(Value::Bool(armed))
        },
    );

    runtime.register_native_with_docs(
        "seq-song-capture-cancel",
        "(seq-song-capture-cancel)",
        "Cancel arrangement capture: discard the take, preserve the committed \
         song, stop the transport. Only valid during arrangement capture.",
        move |_args, ctx| {
            ctx.enqueue_command(HostCommand::Custom {
                name: "song-capture-cancel".to_string(),
                payload: Value::Nil,
            });
            Ok(Value::Bool(true))
        },
    );

    runtime.register_native_with_docs(
        "seq-song-back-to-song",
        "(seq-song-back-to-song)",
        "Back to Song (takes spec 10): clear the manual-override latch so \
         every latched lane snaps back to what the song resolves at the \
         current beat, with anchored phase.",
        move |_args, ctx| {
            ctx.enqueue_command(HostCommand::Custom {
                name: "song-back-to-song".to_string(),
                payload: Value::Nil,
            });
            Ok(Value::Bool(true))
        },
    );

    runtime.register_native_with_docs(
        "seq-song-back-to-song-track",
        "(seq-song-back-to-song-track track)",
        "Per-track Back to Song (takes spec 10 UX): clear one lane's \
         manual-override latch so it snaps back to what the song resolves \
         at the current beat; other latched lanes stay the performer's.",
        move |args, ctx| {
            let Some(Value::Number(track)) = args.first() else {
                return Err("seq-song-back-to-song-track: expected track number".into());
            };
            let mut payload = HashMap::new();
            payload.insert(
                "track".to_string(),
                Rc::new(RefCell::new(Value::Number(*track))),
            );
            ctx.enqueue_command(HostCommand::Custom {
                name: "song-back-to-song-track".to_string(),
                payload: Value::Map(payload),
            });
            Ok(Value::Bool(true))
        },
    );

    runtime.register_native_with_docs(
        "seq-song-select-clip",
        "(seq-song-select-clip track clip-id [start end])",
        "Bind a track's device panel, monitor sound and take punch-in \
         template to the stored clip `clip-id` on `track` (takes spec \
         16.2/16.6). With the clip's beat span, ALSO select that span as a \
         one-track region (region spec 4.1) so the clip body lights up and \
         copy/delete have a target. Call with no arguments (or \
         seq-song-deselect-clip) to fall back to the playing/scene source.",
        move |args, ctx| {
            let (Some(Value::Number(track)), Some(Value::Number(clip_id))) =
                (args.first(), args.get(1))
            else {
                return Err("seq-song-select-clip: expected track and clip-id".into());
            };
            let mut payload = HashMap::new();
            payload.insert(
                "track".to_string(),
                Rc::new(RefCell::new(Value::Number(*track))),
            );
            payload.insert(
                "clip-id".to_string(),
                Rc::new(RefCell::new(Value::Number(*clip_id))),
            );
            if let (Some(Value::Number(start)), Some(Value::Number(end))) =
                (args.get(2), args.get(3))
            {
                payload.insert(
                    "start".to_string(),
                    Rc::new(RefCell::new(Value::Number(*start))),
                );
                payload.insert("end".to_string(), Rc::new(RefCell::new(Value::Number(*end))));
            }
            ctx.enqueue_command(HostCommand::Custom {
                name: "song-select-clip".to_string(),
                payload: Value::Map(payload),
            });
            Ok(Value::Bool(true))
        },
    );

    runtime.register_native_with_docs(
        "seq-song-deselect-clip",
        "(seq-song-deselect-clip)",
        "Clear the timeline clip selection (takes spec 16.6 cause 1): every \
         track falls back to the song-audible source, else its scene pattern.",
        move |_args, ctx| {
            ctx.enqueue_command(HostCommand::Custom {
                name: "song-deselect-clip".to_string(),
                payload: Value::Nil,
            });
            Ok(Value::Bool(true))
        },
    );

    runtime.register_native_with_docs(
        "seq-song-set-region",
        "(seq-song-set-region track-a track-b start end [scene-lane])",
        "Select an arrangement REGION — the inclusive model-track span \
         `track-a`..`track-b` over the half-open beat span `[start, end)` \
         (region spec 4.1). Rust-owned, so it survives view switches; \
         published as SEQ.song-region. A region names no single clip, so it \
         clears the clip selection and releases the sound binding. Pass a \
         truthy `scene-lane` for a marquee swept in the SCENE lane: \
         copy/paste/delete then carry the scene EVENTS inside the rectangle \
         as well as the clips (lane spec 8).",
        move |args, ctx| {
            let (
                Some(Value::Number(track_a)),
                Some(Value::Number(track_b)),
                Some(Value::Number(start)),
                Some(Value::Number(end)),
            ) = (args.first(), args.get(1), args.get(2), args.get(3))
            else {
                return Err(
                    "seq-song-set-region: expected track-a, track-b, start and end".into(),
                );
            };
            let mut payload = HashMap::new();
            for (key, value) in [
                ("track-a", *track_a),
                ("track-b", *track_b),
                ("start", *start),
                ("end", *end),
            ] {
                payload.insert(key.to_string(), Rc::new(RefCell::new(Value::Number(value))));
            }
            let scene_lane = matches!(args.get(4), Some(Value::Bool(true)));
            payload.insert(
                "scene-lane".to_string(),
                Rc::new(RefCell::new(Value::Bool(scene_lane))),
            );
            ctx.enqueue_command(HostCommand::Custom {
                name: "song-set-region".to_string(),
                payload: Value::Map(payload),
            });
            Ok(Value::Bool(true))
        },
    );

    runtime.register_native_with_docs(
        "seq-song-clear-region",
        "(seq-song-clear-region)",
        "Clear the arrangement region selection (region spec 4.1): \
         SEQ.song-region goes nil and every lane drops its region highlight.",
        move |_args, ctx| {
            ctx.enqueue_command(HostCommand::Custom {
                name: "song-clear-region".to_string(),
                payload: Value::Nil,
            });
            Ok(Value::Bool(true))
        },
    );

    runtime.register_native_with_docs(
        "seq-song-set-arr-cursor",
        "(seq-song-set-arr-cursor time [track])",
        "Mirror the arrangement edit cursor into Rust (region spec 5.3). The \
         Lisp view owns the click gesture; the Cmd-V paste seam lives in Rust \
         and pastes at this beat. `track` is the model track index, -1 for \
         the scene lane.",
        move |args, ctx| {
            let Some(Value::Number(time)) = args.first() else {
                return Err("seq-song-set-arr-cursor: expected a time".into());
            };
            let track = match args.get(1) {
                Some(Value::Number(track)) => *track,
                _ => -1.0,
            };
            let mut payload = HashMap::new();
            payload.insert(
                "time".to_string(),
                Rc::new(RefCell::new(Value::Number(*time))),
            );
            payload.insert(
                "track".to_string(),
                Rc::new(RefCell::new(Value::Number(track))),
            );
            ctx.enqueue_command(HostCommand::Custom {
                name: "song-set-arr-cursor".to_string(),
                payload: Value::Map(payload),
            });
            Ok(Value::Bool(true))
        },
    );

    runtime.register_native_with_docs(
        "seq-song-region-copy",
        "(seq-song-region-copy)",
        "Copy the selected arrangement region to the arrangement clipboard \
         (region spec 5.1): the whole time x track rectangle, gaps included. \
         Read-only — no undo entry. A clip selection is a one-clip region, so \
         this copies a selected clip too.",
        move |_args, ctx| {
            ctx.enqueue_command(HostCommand::Custom {
                name: "song-region-copy".to_string(),
                payload: Value::Nil,
            });
            Ok(Value::Bool(true))
        },
    );

    runtime.register_native_with_docs(
        "seq-song-region-paste",
        "(seq-song-region-paste [time])",
        "Paste the arrangement clipboard at `time` — or at the mirrored edit \
         cursor when omitted — floored to the copied rectangle's grid (region \
         spec 5.2). Same-track, time-shift only. Pattern clips paste as \
         references; take clips paste as fresh copies. ONE undo entry.",
        move |args, ctx| {
            let payload = match args.first() {
                Some(Value::Number(time)) => {
                    let mut map = HashMap::new();
                    map.insert(
                        "time".to_string(),
                        Rc::new(RefCell::new(Value::Number(*time))),
                    );
                    Value::Map(map)
                }
                _ => Value::Nil,
            };
            ctx.enqueue_command(HostCommand::Custom {
                name: "song-region-paste".to_string(),
                payload,
            });
            Ok(Value::Bool(true))
        },
    );

    runtime.register_native_with_docs(
        "seq-song-region-delete",
        "(seq-song-region-delete)",
        "Silence the selected arrangement region on every track it covers \
         (region spec 5.2): explicit-empty overrides, ONE undo entry.",
        move |_args, ctx| {
            ctx.enqueue_command(HostCommand::Custom {
                name: "song-region-delete".to_string(),
                payload: Value::Nil,
            });
            Ok(Value::Bool(true))
        },
    );

    runtime.register_native_with_docs(
        "seq-song-region-duplicate",
        "(seq-song-region-duplicate)",
        "Duplicate the selected region immediately after itself, pushing the \
         rest of the song right by its length (Ableton's Duplicate Time). \
         Take clips duplicate as fresh copies; the selection follows the new \
         copy so repeated calls chain. ONE undo entry.",
        move |_args, ctx| {
            ctx.enqueue_command(HostCommand::Custom {
                name: "song-region-duplicate".to_string(),
                payload: Value::Nil,
            });
            Ok(Value::Bool(true))
        },
    );

    runtime.register_native_with_docs(
        "seq-sound-push-to-pattern",
        "(seq-sound-push-to-pattern track)",
        "Push to pattern (takes spec 16.5): copy the track's bound sound \
         into the CURRENT scene's effective pattern. One undo entry; other \
         scenes are untouched.",
        move |args, ctx| {
            let Some(Value::Number(track)) = args.first() else {
                return Err("seq-sound-push-to-pattern: expected track number".into());
            };
            let mut payload = HashMap::new();
            payload.insert(
                "track".to_string(),
                Rc::new(RefCell::new(Value::Number(*track))),
            );
            ctx.enqueue_command(HostCommand::Custom {
                name: "sound-push-to-pattern".to_string(),
                payload: Value::Map(payload),
            });
            Ok(Value::Bool(true))
        },
    );

    runtime.register_native_with_docs(
        "seq-sound-apply-to-all-takes",
        "(seq-sound-apply-to-all-takes track)",
        "Apply to all takes on track (takes spec 16.5): copy the track's \
         bound sound into every take (every chunk). One undo entry.",
        move |args, ctx| {
            let Some(Value::Number(track)) = args.first() else {
                return Err("seq-sound-apply-to-all-takes: expected track number".into());
            };
            let mut payload = HashMap::new();
            payload.insert(
                "track".to_string(),
                Rc::new(RefCell::new(Value::Number(*track))),
            );
            ctx.enqueue_command(HostCommand::Custom {
                name: "sound-apply-to-all-takes".to_string(),
                payload: Value::Map(payload),
            });
            Ok(Value::Bool(true))
        },
    );

    // Sound palette (takes spec 17.6/18.3). The overlay natives address the
    // open palette's target implicitly (the host command falls back to
    // `App::sound_palette_open`, then the track's binding), so the lisp side
    // only ever names the entity being grabbed.
    fn number_entry(value: f64) -> Rc<RefCell<Value>> {
        Rc::new(RefCell::new(Value::Number(value)))
    }
    fn palette_payload(args: &[Value]) -> Result<HashMap<String, Rc<RefCell<Value>>>, String> {
        let Some(Value::Number(track)) = args.first() else {
            return Err("expected track number".into());
        };
        let mut payload = HashMap::new();
        payload.insert("track".to_string(), number_entry(*track));
        Ok(payload)
    }
    runtime.register_native_with_docs(
        "seq-sound-palette-open",
        "(seq-sound-palette-open track [target-kind target-id])",
        "Open the sound palette overlay for a track (takes spec 17.6). \
         target-kind is \"take\", \"pattern\", or \"cell\"; omitted, the \
         track's bound source is the target.",
        move |args, ctx| {
            let mut payload = palette_payload(&args)
                .map_err(|error| format!("seq-sound-palette-open: {error}"))?;
            if let Some(Value::String(kind)) = args.get(1) {
                payload.insert(
                    "target-kind".to_string(),
                    Rc::new(RefCell::new(Value::String(kind.clone()))),
                );
                if let Some(Value::Number(id)) = args.get(2) {
                    payload.insert("target-id".to_string(), number_entry(*id));
                }
            }
            ctx.enqueue_command(HostCommand::Custom {
                name: "sound-palette-open".to_string(),
                payload: Value::Map(payload),
            });
            Ok(Value::Bool(true))
        },
    );
    runtime.register_native_with_docs(
        "seq-sound-palette-close",
        "(seq-sound-palette-close)",
        "Close the sound palette overlay.",
        move |_args, ctx| {
            ctx.enqueue_command(HostCommand::Custom {
                name: "sound-palette-close".to_string(),
                payload: Value::Map(HashMap::new()),
            });
            Ok(Value::Bool(true))
        },
    );
    runtime.register_native_with_docs(
        "seq-sound-apply",
        "(seq-sound-apply track patch-id)",
        "Palette Apply (takes spec 17.6): re-link the open target's patch \
         ref to the named Patch entity. Reference semantics - future edits \
         to that patch follow. One undo entry; key locks ride the patch.",
        move |args, ctx| {
            let mut payload = palette_payload(&args)
                .map_err(|error| format!("seq-sound-apply: {error}"))?;
            let Some(Value::Number(patch)) = args.get(1) else {
                return Err("seq-sound-apply: expected patch id".into());
            };
            payload.insert("patch".to_string(), number_entry(*patch));
            ctx.enqueue_command(HostCommand::Custom {
                name: "sound-apply".to_string(),
                payload: Value::Map(payload),
            });
            Ok(Value::Bool(true))
        },
    );
    runtime.register_native_with_docs(
        "seq-sound-apply-with-mix",
        "(seq-sound-apply-with-mix track patch-id mix-id)",
        "Palette Apply with mix (takes spec 17.6): re-link BOTH refs of the \
         open target to the named entity pair. One undo entry.",
        move |args, ctx| {
            let mut payload = palette_payload(&args)
                .map_err(|error| format!("seq-sound-apply-with-mix: {error}"))?;
            let (Some(Value::Number(patch)), Some(Value::Number(mix))) =
                (args.get(1), args.get(2))
            else {
                return Err("seq-sound-apply-with-mix: expected patch and mix ids".into());
            };
            payload.insert("patch".to_string(), number_entry(*patch));
            payload.insert("mix".to_string(), number_entry(*mix));
            ctx.enqueue_command(HostCommand::Custom {
                name: "sound-apply-with-mix".to_string(),
                payload: Value::Map(payload),
            });
            Ok(Value::Bool(true))
        },
    );
    runtime.register_native_with_docs(
        "seq-sound-fork",
        "(seq-sound-fork track)",
        "Palette Fork / own parameters (takes spec 17.3): clone the open \
         target's Patch+Mix and repoint the target at the clones. One undo \
         entry.",
        move |args, ctx| {
            let payload =
                palette_payload(&args).map_err(|error| format!("seq-sound-fork: {error}"))?;
            ctx.enqueue_command(HostCommand::Custom {
                name: "sound-fork".to_string(),
                payload: Value::Map(payload),
            });
            Ok(Value::Bool(true))
        },
    );
    runtime.register_native_with_docs(
        "seq-sound-rename",
        "(seq-sound-rename track kind entity-id name)",
        "Rename a pooled sound entity (takes spec 17.11). kind is \"patch\" \
         or \"mix\". Overlay-only gesture; one undo entry.",
        move |args, ctx| {
            let (
                Some(Value::Number(track)),
                Some(Value::String(kind)),
                Some(Value::Number(entity)),
                Some(Value::String(name)),
            ) = (args.first(), args.get(1), args.get(2), args.get(3))
            else {
                return Err(
                    "seq-sound-rename: expected (track kind entity-id name)".into()
                );
            };
            let mut payload = HashMap::new();
            payload.insert("track".to_string(), number_entry(*track));
            payload.insert(
                "kind".to_string(),
                Rc::new(RefCell::new(Value::String(kind.clone()))),
            );
            payload.insert("entity".to_string(), number_entry(*entity));
            payload.insert(
                "name".to_string(),
                Rc::new(RefCell::new(Value::String(name.clone()))),
            );
            ctx.enqueue_command(HostCommand::Custom {
                name: "sound-rename".to_string(),
                payload: Value::Map(payload),
            });
            Ok(Value::Bool(true))
        },
    );
    runtime.register_native_with_docs(
        "seq-sound-cleanup-unused",
        "(seq-sound-cleanup-unused track)",
        "Clean up unused sound entities on a track (takes spec 17.4): the \
         interactive version of the save-time prune. Frees palette colors.",
        move |args, ctx| {
            let payload = palette_payload(&args)
                .map_err(|error| format!("seq-sound-cleanup-unused: {error}"))?;
            ctx.enqueue_command(HostCommand::Custom {
                name: "sound-cleanup-unused".to_string(),
                payload: Value::Map(payload),
            });
            Ok(Value::Bool(true))
        },
    );

    runtime.register_native_with_docs(
        "seq-song-status",
        "(seq-song-status)",
        "Report the song transport status on the status line: mode, row \
         count, end beat, and loop state.",
        move |_args, ctx| {
            ctx.enqueue_command(HostCommand::Custom {
                name: "song-status".to_string(),
                payload: Value::Nil,
            });
            Ok(Value::Bool(true))
        },
    );

    // Declarative authoring (spec section 6). The compiler auto-quotes every
    // def-song argument, so the `(at ...)` rows arrive here as list data.
    // The complete definition is parsed and shape-validated before the
    // single arrangement-replace command is emitted; a failed definition
    // emits nothing and leaves the committed arrangement unchanged.
    runtime.register_native_with_docs(
        "def-song",
        "(def-song \"name\" (at 0 :scene 0) (at 32 :scene 1 :patterns ((1 3))) :end 64 [:loop true])",
        "Define the project song declaratively from absolute beat positions. \
         Lowers to the arrangement: scene changes become scene events, \
         per-track overrides become clips, one undo entry. A failed \
         definition leaves the previous song unchanged.",
        move |args, ctx| {
            let spec = parse_def_song_args(&args).map_err(|error| format!("def-song: {error}"))?;
            ctx.enqueue_command(HostCommand::Custom {
                name: "arrangement-replace".to_string(),
                payload: def_song_replace_payload(&spec),
            });
            Ok(Value::String(spec.name))
        },
    );
}

fn toggle_master_recording_capture(
    master_recording: &AtomicBool,
    master_recorder: &sequencer::recorder::MasterRecorder,
) -> Result<(bool, String), String> {
    toggle_master_recording_capture_in(
        master_recording,
        master_recorder,
        &sequencer::app_paths::app_paths().recordings_dir(),
    )
}

fn toggle_master_recording_capture_in(
    master_recording: &AtomicBool,
    master_recorder: &sequencer::recorder::MasterRecorder,
    recordings_dir: &std::path::Path,
) -> Result<(bool, String), String> {
    if master_recording.load(Ordering::Acquire) {
        let take = match master_recorder.stop() {
            Ok(take) => take,
            Err(error) => {
                master_recording.store(false, Ordering::Release);
                return Err(format!("Failed to stop master recording: {error}"));
            }
        };
        master_recording.store(false, Ordering::Release);

        let path = recordings_dir.join(sequencer::recorder::default_recording_name());
        sequencer::recorder::save_recording_wav(&path, &take)
            .map_err(|error| format!("Failed to save master recording: {error}"))?;
        Ok((
            false,
            format!("Saved master recording to {}", path.display()),
        ))
    } else {
        match master_recorder.start() {
            Ok(()) => {
                master_recording.store(true, Ordering::Release);
                Ok((true, "Master WAV recording started".to_string()))
            }
            Err(error) => {
                master_recording.store(false, Ordering::Release);
                Err(format!("Failed to start master recording: {error}"))
            }
        }
    }
}

fn parse_fx_delete_chain(value: &Value) -> Option<FxDeleteChain> {
    match value_string_field(value, "chain")?.as_str() {
        "audio" | "fx" => Some(FxDeleteChain::Audio),
        "midi" | "midi-fx" => Some(FxDeleteChain::Midi),
        "bus" | "bus-fx" => Some(FxDeleteChain::Bus),
        _ => None,
    }
}

fn parse_delete_target(kind: &Value, payload: &Value) -> Result<ActiveDeleteTarget, String> {
    match delete_kind_name(kind) {
        Some("mixer-track") | Some("track") => {
            let track = value_number_field(payload, "track")
                .ok_or_else(|| "mixer-track delete target expects :track".to_string())?;
            Ok(ActiveDeleteTarget::MixerTrack { track })
        }
        Some("mixer-tracks") | Some("tracks") => {
            let Value::Map(fields) = payload else {
                return Err("mixer-tracks delete target expects a :tracks list".to_string());
            };
            let tracks = fields
                .get("tracks")
                .ok_or_else(|| "mixer-tracks delete target expects :tracks".to_string())?;
            let tracks = tracks.borrow();
            let Value::List(tracks) = &*tracks else {
                return Err("mixer-tracks delete target expects :tracks to be a list".to_string());
            };
            let mut parsed = Vec::with_capacity(tracks.len());
            for track in tracks {
                let track = track.borrow();
                let Value::Number(track) = &*track else {
                    return Err("mixer-tracks delete target indices must be numbers".to_string());
                };
                if !track.is_finite() || *track < 0.0 || track.fract() != 0.0 {
                    return Err(
                        "mixer-tracks delete target indices must be non-negative integers"
                            .to_string(),
                    );
                }
                parsed.push(*track as usize);
            }
            parsed.sort_unstable();
            parsed.dedup();
            if parsed.len() < 2 {
                return Err("mixer-tracks delete target expects at least two tracks".to_string());
            }
            Ok(ActiveDeleteTarget::MixerTracks { tracks: parsed })
        }
        Some("mixer-group") | Some("group") => {
            let group_id = value_number_field(payload, "group-id")
                .ok_or_else(|| "mixer-group delete target expects :group-id".to_string())?;
            Ok(ActiveDeleteTarget::MixerGroup { group_id: group_id as u64 })
        }
        Some("track-pattern") | Some("pattern") => {
            let track = value_number_field(payload, "track")
                .ok_or_else(|| "track-pattern delete target expects :track".to_string())?;
            let pattern_id = value_number_field(payload, "pattern-id")
                .or_else(|| value_number_field(payload, "pattern_id"))
                .ok_or_else(|| "track-pattern delete target expects :pattern-id".to_string())?;
            if pattern_id == 0 {
                return Err("track-pattern delete target pattern id must be positive".to_string());
            }
            Ok(ActiveDeleteTarget::TrackPattern {
                track,
                pattern_id: PatternId(pattern_id as u64),
            })
        }
        Some("mod-route") | Some("route") | Some("cable") => {
            let source = value_number_field(payload, "source")
                .ok_or_else(|| "mod-route delete target expects :source".to_string())?;
            let dest = value_number_field(payload, "dest")
                .ok_or_else(|| "mod-route delete target expects :dest".to_string())?;
            let destination = match value_string_field(payload, "dest-kind").as_deref() {
                Some("bus") => sequencer::sequencer::ModDestination::Bus(
                    sequencer::sequencer::BusId(dest as u64),
                ),
                _ => sequencer::sequencer::ModDestination::Track(dest),
            };
            let input = value_number_field(payload, "input")
                .ok_or_else(|| "mod-route delete target expects :input".to_string())?;
            Ok(ActiveDeleteTarget::ModRoute {
                source,
                destination,
                input,
            })
        }
        Some("fx-effect") | Some("effect") => {
            if value_string_field(payload, "chain").as_deref() == Some("rack") {
                let track = value_number_field(payload, "track")
                    .ok_or_else(|| "rack fx-effect delete target expects :track".to_string())?;
                let rack_slot = value_number_field(payload, "rack-slot")
                    .ok_or_else(|| "rack fx-effect delete target expects :rack-slot".to_string())?;
                let effect_slot = value_number_field(payload, "effect-slot").ok_or_else(|| {
                    "rack fx-effect delete target expects :effect-slot".to_string()
                })?;
                return Ok(ActiveDeleteTarget::RackEffect {
                    track,
                    rack_slot,
                    effect_slot,
                });
            }
            let chain = parse_fx_delete_chain(payload)
                .ok_or_else(|| "fx-effect delete target expects :chain".to_string())?;
            let slot = value_number_field(payload, "slot")
                .ok_or_else(|| "fx-effect delete target expects :slot".to_string())?;
            let bus = value_number_field(payload, "bus");
            if chain == FxDeleteChain::Bus && bus.is_none() {
                return Err("bus fx-effect delete target expects :bus".to_string());
            }
            Ok(ActiveDeleteTarget::FxEffect { chain, bus, slot })
        }
        Some("rack-slot") | Some("rack-layer") => {
            let track = value_number_field(payload, "track")
                .ok_or_else(|| "rack-slot delete target expects :track".to_string())?;
            let slot = value_number_field(payload, "slot")
                .ok_or_else(|| "rack-slot delete target expects :slot".to_string())?;
            Ok(ActiveDeleteTarget::RackSlot { track, slot })
        }
        Some(other) => Err(format!("unknown delete target kind :{other}")),
        None => Err("delete target kind must be a keyword or string".to_string()),
    }
}

fn effect_param_target(
    slot_state: &sequencer::effects::EffectSlotState,
    param_idx: usize,
) -> Option<(u64, u64)> {
    let idx = slot_state.resolve_node_idx(param_idx);
    if idx == u32::MAX as u64 {
        return None;
    }
    if idx as u32 >= sequencer::instruments::voice_modulator::MOD_PARAM_BASE {
        let modulator_node_id = slot_state.modulator_node_id.load(Ordering::Relaxed);
        (modulator_node_id != 0).then_some((
            modulator_node_id as u64,
            idx - sequencer::instruments::voice_modulator::MOD_PARAM_BASE as u64,
        ))
    } else {
        let node_id = slot_state.node_id.load(Ordering::Relaxed);
        (node_id != 0).then_some((node_id as u64, idx))
    }
}

fn active_delete_target_kind(target: Option<&ActiveDeleteTarget>) -> Value {
    match target {
        Some(ActiveDeleteTarget::MixerTrack { .. }) => Value::String("mixer-track".to_string()),
        Some(ActiveDeleteTarget::MixerTracks { .. }) => Value::String("mixer-tracks".to_string()),
        Some(ActiveDeleteTarget::MixerGroup { .. }) => Value::String("mixer-group".to_string()),
        Some(ActiveDeleteTarget::TrackPattern { .. }) => Value::String("track-pattern".to_string()),
        Some(ActiveDeleteTarget::ModRoute { .. }) => Value::String("mod-route".to_string()),
        Some(ActiveDeleteTarget::FxEffect { .. }) => Value::String("fx-effect".to_string()),
        Some(ActiveDeleteTarget::RackEffect { .. }) => Value::String("fx-effect".to_string()),
        Some(ActiveDeleteTarget::RackSlot { .. }) => Value::String("rack-slot".to_string()),
        None => Value::Bool(false),
    }
}

/// Arming/clearing a delete target only moves the delete-target read
/// surfaces (`SEQ.delete-target-version` + the mixer/rack binding fields),
/// which the reactive tick republishes off the version counter alone — a
/// `ui_epoch` bump here would buy nothing but a whole-project resync per
/// clip-launch click (~7ms at 20-clip pool scale).
fn bump_delete_target_version(active_delete_target_version: &Arc<AtomicUsize>) {
    active_delete_target_version.fetch_add(1, Ordering::Relaxed);
}

#[cfg(test)]
mod delete_target_tests {
    use super::*;

    fn map_value(entries: impl IntoIterator<Item = (&'static str, Value)>) -> Value {
        Value::Map(
            entries
                .into_iter()
                .map(|(key, value)| (key.to_string(), Rc::new(RefCell::new(value))))
                .collect(),
        )
    }

    #[test]
    fn delete_target_parser_distinguishes_mixer_track_mod_route_and_fx_effects() {
        assert_eq!(
            parse_delete_target(
                &Value::Keyword("mixer-track".to_string()),
                &map_value([("track", Value::Number(2.0))]),
            )
            .expect("mixer target"),
            ActiveDeleteTarget::MixerTrack { track: 2 }
        );

        assert_eq!(
            parse_delete_target(
                &Value::Keyword("mixer-tracks".to_string()),
                &map_value([(
                    "tracks",
                    Value::List(vec![
                        Rc::new(RefCell::new(Value::Number(4.0))),
                        Rc::new(RefCell::new(Value::Number(2.0))),
                        Rc::new(RefCell::new(Value::Number(4.0))),
                    ]),
                )]),
            )
            .expect("multiple mixer target"),
            ActiveDeleteTarget::MixerTracks { tracks: vec![2, 4] }
        );
        assert_eq!(
            active_delete_target_kind(Some(&ActiveDeleteTarget::MixerTracks {
                tracks: vec![2, 4],
            })),
            Value::String("mixer-tracks".to_string())
        );

        assert_eq!(
            parse_delete_target(
                &Value::Keyword("mixer-group".to_string()),
                &map_value([("group-id", Value::Number(17.0))]),
            )
            .expect("mixer group target"),
            ActiveDeleteTarget::MixerGroup { group_id: 17 }
        );

        assert_eq!(
            parse_delete_target(
                &Value::Keyword("mod-route".to_string()),
                &map_value([
                    ("source", Value::Number(0.0)),
                    ("dest", Value::Number(3.0)),
                    ("input", Value::Number(1.0)),
                ]),
            )
            .expect("mod route target"),
            ActiveDeleteTarget::ModRoute {
                source: 0,
                destination: sequencer::sequencer::ModDestination::Track(3),
                input: 1,
            }
        );

        assert_eq!(
            parse_delete_target(
                &Value::Keyword("track-pattern".to_string()),
                &map_value([
                    ("track", Value::Number(1.0)),
                    ("pattern-id", Value::Number(4.0)),
                ]),
            )
            .expect("track pattern target"),
            ActiveDeleteTarget::TrackPattern {
                track: 1,
                pattern_id: PatternId(4),
            }
        );

        assert_eq!(
            parse_delete_target(
                &Value::Keyword("fx-effect".to_string()),
                &map_value([
                    ("chain", Value::String("bus".to_string())),
                    ("bus", Value::Number(1.0)),
                    ("slot", Value::Number(4.0)),
                ]),
            )
            .expect("bus fx target"),
            ActiveDeleteTarget::FxEffect {
                chain: FxDeleteChain::Bus,
                bus: Some(1),
                slot: 4,
            }
        );

        assert_eq!(
            parse_delete_target(
                &Value::Keyword("rack-slot".to_string()),
                &map_value([("track", Value::Number(2.0)), ("slot", Value::Number(3.0))]),
            )
            .expect("rack slot target"),
            ActiveDeleteTarget::RackSlot { track: 2, slot: 3 }
        );

        assert_eq!(
            parse_delete_target(
                &Value::Keyword("fx-effect".to_string()),
                &map_value([
                    ("chain", Value::String("rack".to_string())),
                    ("track", Value::Number(2.0)),
                    ("rack-slot", Value::Number(3.0)),
                    ("effect-slot", Value::Number(1.0)),
                ]),
            )
            .expect("rack fx target"),
            ActiveDeleteTarget::RackEffect {
                track: 2,
                rack_slot: 3,
                effect_slot: 1,
            }
        );
    }

    #[test]
    fn selected_tracks_arm_and_clear_multiple_track_delete_target() {
        let target = Arc::new(Mutex::new(Some(ActiveDeleteTarget::MixerTrack { track: 0 })));
        let version = Arc::new(AtomicUsize::new(0));
        let selected = HashSet::from([3, 1]);

        arm_selected_tracks_delete_target(&selected, &target, &version);
        assert_eq!(
            *target.lock().unwrap(),
            Some(ActiveDeleteTarget::MixerTracks { tracks: vec![1, 3] })
        );
        assert_eq!(version.load(Ordering::Relaxed), 1);

        arm_selected_tracks_delete_target(&HashSet::from([3]), &target, &version);
        assert_eq!(*target.lock().unwrap(), None);
        assert_eq!(version.load(Ordering::Relaxed), 2);
    }

    #[test]
    fn delete_target_parser_rejects_incomplete_bus_fx_target() {
        let err = parse_delete_target(
            &Value::Keyword("fx-effect".to_string()),
            &map_value([
                ("chain", Value::String("bus".to_string())),
                ("slot", Value::Number(0.0)),
            ]),
        )
        .expect_err("bus fx target without bus should fail");
        assert!(err.contains(":bus"), "unexpected error: {err}");
    }
}

#[cfg(test)]
mod process_binding_target_tests {
    use super::*;

    fn map_value(entries: impl IntoIterator<Item = (&'static str, Value)>) -> Value {
        Value::Map(
            entries
                .into_iter()
                .map(|(key, value)| (key.to_string(), Rc::new(RefCell::new(value))))
                .collect(),
        )
    }

    #[test]
    fn process_binding_target_prefers_clicked_instrument_param_name_over_descriptor_cache() {
        let state =
            SequencerState::new(1, vec![sequencer::sequencer::default_empty_effect_chain()]);
        let sampler_desc = sequencer::effects::EffectDescriptor::builtin_sampler();
        let speed_idx = sampler_desc
            .params
            .iter()
            .position(|param| param.name == "speed")
            .expect("sampler speed param");
        state.pattern.instrument_slots[0].apply_descriptor(&sampler_desc, 12);
        state.set_scratch_runtime_descriptors(
            vec![sequencer::effects::EffectDescriptor::default_full_chain()],
            vec![sequencer::effects::EffectDescriptor::builtin_filter()],
        );

        let target = param_target_from_value(
            &state,
            0,
            &map_value([
                ("kind", Value::String("instrument".to_string())),
                ("param-idx", Value::Number(speed_idx as f64)),
                ("param", Value::String("speed".to_string())),
            ]),
        )
        .expect("instrument target");

        match target {
            sequencer::process::ParamTarget::InstrumentParam { param, param_id } => {
                assert_eq!(param, "speed");
                assert_eq!(
                    param_id,
                    state.pattern.instrument_slots[0].param_node_id(speed_idx)
                );
            }
            other => panic!("unexpected target: {other:?}"),
        }
    }
}

/// Toggles a track's record arm, keeping the drum-rack arm exclusive with it:
/// arming a member track hands the live keyboard back to chromatic play, so
/// the rack it belongs to disarms (docs/drum-rack-v2-spec.md, "Arming & live
/// play"). Tracks outside the armed rack leave pad play alone. Returns the new
/// arm state, or `None` when the track index is out of range.
pub(crate) fn toggle_track_record_arm(
    record_armed: &mut [bool],
    armed_rack: &mut Option<u64>,
    groups: &[sequencer::project::ProjectTrackGroup],
    track: usize,
) -> Option<bool> {
    let flag = record_armed.get_mut(track)?;
    *flag = !*flag;
    let armed = *flag;
    if armed {
        if let Some(rack_id) = *armed_rack {
            let is_member = groups
                .iter()
                .any(|group| group.id == rack_id && group.members.contains(&track));
            if is_member {
                *armed_rack = None;
            }
        }
    }
    Some(armed)
}

/// Toggles a drum rack's pad arm. Only one rack answers the keyboard at a
/// time, and arming one disarms its own member tracks so a member never
/// responds to a key both as a pad and chromatically. Returns the new state.
pub(crate) fn toggle_rack_pad_arm(
    armed_rack: &mut Option<u64>,
    record_armed: &mut [bool],
    members: &[usize],
    group_id: u64,
) -> bool {
    let armed = *armed_rack != Some(group_id);
    *armed_rack = armed.then_some(group_id);
    if armed {
        for member in members {
            if let Some(flag) = record_armed.get_mut(*member) {
                *flag = false;
            }
        }
    }
    armed
}

/// Whether `armed_rack` still names a live drum rack. Group ids are recycled
/// (`max(id) + 1` in `app::edit`), so a stale arm is worse than a dangling
/// reference: the next group created inherits the dead id and comes up
/// pad-armed, stealing the keyboard from the sequencer's bare-key shortcuts.
/// Ungrouping or converting a rack away is the same staleness — the id lives
/// on, but no longer as a rack.
pub(crate) fn armed_rack_still_live(
    groups: &[sequencer::project::ProjectTrackGroup],
    armed_rack: Option<u64>,
) -> bool {
    let Some(rack_id) = armed_rack else {
        return true;
    };
    groups
        .iter()
        .any(|group| group.id == rack_id && group.is_rack())
}

/// Whether the armed delete target still names something that exists. Only
/// the group-addressed variant can be invalidated by a group edit; the
/// track-addressed ones are reindexed (and cleared) on their own paths.
pub(crate) fn delete_target_still_live(
    groups: &[sequencer::project::ProjectTrackGroup],
    target: Option<&ActiveDeleteTarget>,
) -> bool {
    match target {
        Some(ActiveDeleteTarget::MixerGroup { group_id }) => {
            groups.iter().any(|group| group.id == *group_id)
        }
        _ => true,
    }
}

/// Drops shared group references that the just-applied topology edit killed.
/// Called from the two group-topology sync choke points
/// (`sync_after_track_topology_delete`, `sync_after_rack_structure_change`),
/// so delete/ungroup/convert/audition all get the same invalidation without
/// each handler remembering to clear.
pub(crate) fn prune_stale_group_references(
    armed_rack: &Arc<Mutex<Option<u64>>>,
    active_delete_target: &Arc<Mutex<Option<ActiveDeleteTarget>>>,
    active_delete_target_version: &Arc<AtomicUsize>,
    groups: &[sequencer::project::ProjectTrackGroup],
) {
    {
        let mut guard = armed_rack.lock().unwrap();
        if !armed_rack_still_live(groups, *guard) {
            *guard = None;
        }
    }
    let mut guard = active_delete_target.lock().unwrap();
    if !delete_target_still_live(groups, guard.as_ref()) {
        *guard = None;
        bump_delete_target_version(active_delete_target_version);
    }
}

fn arm_selected_tracks_delete_target(
    selected: &HashSet<usize>,
    active_delete_target: &Arc<Mutex<Option<ActiveDeleteTarget>>>,
    active_delete_target_version: &Arc<AtomicUsize>,
) {
    let mut tracks: Vec<usize> = selected.iter().copied().collect();
    tracks.sort_unstable();
    let target = (tracks.len() >= 2).then_some(ActiveDeleteTarget::MixerTracks { tracks });
    let mut guard = active_delete_target.lock().unwrap();
    if *guard != target {
        *guard = target;
        bump_delete_target_version(active_delete_target_version);
    }
}

fn register_track_selection_natives(
    runtime: &mut Runtime,
    state: Arc<SequencerState>,
    current_track: Arc<AtomicUsize>,
    selected_tracks: Arc<Mutex<HashSet<usize>>>,
    active_delete_target: Arc<Mutex<Option<ActiveDeleteTarget>>>,
    active_delete_target_version: Arc<AtomicUsize>,
) {
    fn track_arg(value: Option<&Value>, name: &str, native: &str) -> Result<usize, String> {
        let Some(Value::Number(value)) = value else {
            return Err(format!("{native}: expected {name} track number"));
        };
        if !value.is_finite() || *value < 0.0 || value.fract() != 0.0 {
            return Err(format!("{native}: invalid {name} track {value}"));
        }
        Ok(*value as usize)
    }

    let range_state = Arc::clone(&state);
    let range_current_track = Arc::clone(&current_track);
    let range_selected_tracks = Arc::clone(&selected_tracks);
    let range_delete_target = Arc::clone(&active_delete_target);
    let range_delete_target_version = Arc::clone(&active_delete_target_version);
    runtime.register_native("seq-select-track-range", move |args, _ctx| {
        let anchor = track_arg(args.first(), "anchor", "seq-select-track-range")?;
        let target = track_arg(args.get(1), "target", "seq-select-track-range")?;
        let track_count = range_state.active_track_count();
        if anchor >= track_count {
            return Err(format!("seq-select-track-range: anchor track {anchor} out of range").into());
        }
        if target >= track_count {
            return Err(format!("seq-select-track-range: target track {target} out of range").into());
        }

        let (start, end) = if anchor <= target {
            (anchor, target)
        } else {
            (target, anchor)
        };
        {
            let mut selected = range_selected_tracks.lock().unwrap();
            selected.clear();
            selected.extend(start..=end);
            arm_selected_tracks_delete_target(
                &selected,
                &range_delete_target,
                &range_delete_target_version,
            );
        }
        range_current_track.store(target, Ordering::Relaxed);
        Ok(Value::Number(target as f64))
    });

    runtime.register_native("seq-select-tracks", move |args, _ctx| {
        let Some(Value::List(values)) = args.first() else {
            return Err("seq-select-tracks: expected a track list".to_string());
        };
        if values.is_empty() {
            return Err("seq-select-tracks: track list cannot be empty".to_string());
        }
        let track_count = state.active_track_count();
        let mut replacement = HashSet::with_capacity(values.len());
        for value in values {
            let value = value.borrow();
            let track = track_arg(Some(&value), "selected", "seq-select-tracks")?;
            if track >= track_count {
                return Err(format!("seq-select-tracks: track {track} out of range"));
            }
            if !replacement.insert(track) {
                return Err(format!("seq-select-tracks: duplicate track {track}"));
            }
        }
        let target = track_arg(args.get(1), "target", "seq-select-tracks")?;
        if !replacement.contains(&target) {
            return Err(format!("seq-select-tracks: target track {target} is not selected"));
        }

        {
            let mut selected = selected_tracks.lock().unwrap();
            *selected = replacement;
            arm_selected_tracks_delete_target(
                &selected,
                &active_delete_target,
                &active_delete_target_version,
            );
        }
        current_track.store(target, Ordering::Relaxed);
        Ok(Value::Number(target as f64))
    });
}

pub(crate) fn init_runtime(
    app: &app::App,
    state: Arc<SequencerState>,
    track_names: &[String],
    track_pan_ids: Arc<Mutex<Vec<i32>>>,
    track_collapsed: Arc<Mutex<Vec<bool>>>,
    buses: Arc<Mutex<Vec<app::BusChannelState>>>,
    bus_node_ids: Arc<Mutex<Vec<app::BusNodeIds>>>,
    current_track: Arc<AtomicUsize>,
    selected_tracks: Arc<Mutex<HashSet<usize>>>,
    track_groups: Arc<Mutex<Vec<sequencer::project::ProjectTrackGroup>>>,
    selected_steps: Arc<Mutex<HashSet<usize>>>,
    piano_roll_selection: Arc<Mutex<HashSet<u64>>>,
    piano_roll_move_state: Arc<Mutex<Option<PianoRollMoveState>>>,
    piano_roll_focus: SharedPianoRollFocus,
    recording: Arc<AtomicBool>,
    master_recording: Arc<AtomicBool>,
    master_recorder: Arc<sequencer::recorder::MasterRecorder>,
    record_armed: Arc<Mutex<Vec<bool>>>,
    armed_rack: Arc<Mutex<Option<u64>>>,
    ui_epoch: Arc<AtomicUsize>,
    fx_epoch: Arc<AtomicUsize>,
    ui_invalidations: Arc<UiInvalidationQueue>,
    expanded_step_projection: Arc<ExpandedStepProjectionRegistry>,
    selected_neural_neurons: sequencer::lisp_host::SharedSelectedNeuralNeurons,
    active_delete_target: Arc<Mutex<Option<ActiveDeleteTarget>>>,
    active_delete_target_version: Arc<AtomicUsize>,
    auto_follow_override_until: Arc<Mutex<Option<Instant>>>,
    lg_raw: *mut sequencer::audiograph::LiveGraph,
) -> RuntimeInit {
    let mut runtime = Runtime::new();
    register_factory_path_native(&mut runtime);
    sequencer::lisp_host::register_neural_authoring_natives_with_selection(
        &mut runtime,
        Arc::clone(&state),
        Arc::clone(&selected_neural_neurons),
    );
    sequencer::lisp_host::register_graph_authoring_natives(&mut runtime, Arc::clone(&state));
    let process_authoring_natives =
        sequencer::lisp_host::register_published_process_authoring_natives(
            &mut runtime,
            Arc::clone(&state),
            Arc::clone(&ui_epoch),
        );
    let process_library = sequencer::lisp_host::load_process_library_source();
    if !process_library.trim().is_empty() {
        let _ = runtime.eval_str(&process_library);
    }
    // The *scratch* buffer evaluates on this runtime (eval_source_transactional),
    // so the bare `jak` macro + jaki module must be resident here for jaki
    // sequencers to author. Loaded unconditionally: this runtime is built once
    // at startup, unlike the per-source scheduler runtime with its usage gate.
    let jaki_library = sequencer::lisp_host::load_jaki_library_source();
    if !jaki_library.trim().is_empty() {
        if let Err(error) = runtime.eval_str(&jaki_library) {
            eprintln!("metal_seq: jaki library eval failed on UI runtime: {error:?}");
        }
    }
    let debug_accum = std::env::var_os("TINYSEQ_DEBUG_ACCUM").is_some();

    let track_count = track_names.len();
    let effect_descriptors = app.graph.effect_descriptors.clone();
    state.set_scratch_runtime_descriptors(
        app.graph.effect_descriptors.clone(),
        app.graph.instrument_descriptors.clone(),
    );
    let accumulator_names = Arc::new(Mutex::new(build_accumulator_names(&app)));
    let midi_fx_names = Arc::new(Mutex::new(Vec::<String>::new()));

    // Register SEQ reactive namespace
    runtime.register_reactive(
        "SEQ",
        {
            let mut fields = vec![
                ("macros", build_macros_value(app)),
                ("playing", Value::Bool(false)),
                ("bpm", Value::Number(120.0)),
                ("scene-launch-quantize", Value::String("off".to_string())),
                ("record-quantize", Value::String("1/16".to_string())),
                ("metronome", Value::Bool(false)),
                // Roll mode (docs/rolling-core-spec.md 8): toggle + rate label.
                ("roll-mode", Value::Bool(false)),
                ("roll-rate", Value::String("16".to_string())),
                ("sequence-rolling", Value::Bool(false)),
                // Per-track false or (start_beats len_beats), published by
                // the scheduler for the sequence-roll bracket/playhead UI.
                ("roll-window", Value::List(vec![])),
                ("queued-scene", Value::Number(-1.0)),
                // Per-track pattern id (-1 = none) with a pending quantized
                // clip launch — drives the mixer grid's queued-cell blink.
                ("queued-track-clips", Value::List(vec![])),
                // Song mode observability (docs/song-mode-spec.md 12).
                ("song-exists", Value::Bool(false)),
                ("song-mode", Value::String("stopped".to_string())),
                ("song-recording-kind", Value::String("".to_string())),
                ("song-manual-latch", Value::Bool(false)),
                ("song-track-latched", Value::List(vec![])),
                ("song-scene-latched", Value::Bool(false)),
                ("song-current-row", Value::Number(-1.0)),
                ("song-current-row-id", Value::Number(-1.0)),
                ("song-row-count", Value::Number(0.0)),
                ("song-cursor-beats", Value::Number(0.0)),
                ("song-position-beats", Value::Number(0.0)),
                ("song-end-beat", Value::Number(0.0)),
                ("song-loop-enabled", Value::Bool(false)),
                ("song-capture-failed", Value::Bool(false)),
                ("song-capture-error", Value::Nil),
                ("song-edit-error", Value::Nil),
                ("song-track-governed", Value::List(vec![])),
                ("song-bound-clip", Value::Nil),
                // Region selection (region spec 4.1): nil, or
                // (track-a track-b start end scene-lane?).
                ("song-region", Value::Nil),
                // Arrangement read surfaces (lane spec 12): the stored clips
                // and the derived scene-event spans. There is no third
                // surface — a lane gap is silence, so it renders as nothing.
                ("song-lanes", Value::List(vec![])),
                ("scene-spans", Value::List(vec![])),
                ("song-lane-events", Value::List(vec![])),
                // Sound palette read surfaces (takes spec 17.6/18.3): the
                // open overlay's entries (Nil = closed) and the per-clip
                // sound divergence/color join for the timeline dots.
                ("sound-palette", Value::Nil),
                ("song-clip-sounds", Value::List(vec![])),
                // Provisional arrangement-capture content
                // (docs/realtime-arrangement-feedback-spec.md 3.2): nil
                // unless a capture is running. Inert — no ids, so no gesture
                // can address it.
                ("song-pending", Value::Nil),
                ("scene-names", Value::List(vec![])),
                ("num-steps", Value::Number(PAGE_SIZE as f64)),
                ("num-tracks", Value::Number(track_count as f64)),
                ("current-track", Value::Number(0.0)),
                ("selected-tracks", Value::List(vec![])),
                ("groups", Value::List(vec![])),
                ("group-collapsed", Value::List(vec![])),
                // Group id of the pad-armed drum rack; -1 = none.
                ("armed-rack-id", Value::Number(-1.0)),
                ("delete-target-version", Value::Number(0.0)),
                ("selected-mod-routes", Value::List(vec![])),
                (
                    "current-pattern",
                    Value::Number(state.current_scene_index() as f64),
                ),
                ("num-patterns", Value::Number(state.scene_count() as f64)),
                ("neural-networks", build_neural_networks_value(&state)),
                (
                    "selected-neural-neurons",
                    sequencer::lisp_host::selected_neural_neurons_to_value(
                        &selected_neural_neurons.lock().unwrap(),
                    ),
                ),
                (
                    "neural-energy-matrix",
                    build_neural_energy_matrix_value(&state),
                ),
                (
                    "neural-trigger-matrix",
                    build_neural_trigger_matrix_value(&state),
                ),
                (
                    "neural-dampening-matrix",
                    build_neural_dampening_matrix_value(&state),
                ),
                (
                    "graph-visualizations",
                    build_graph_visualizations_value(&state),
                ),
                ("track-events", build_track_output_events_value(&state)),
                (
                    "track-event-current-beat",
                    build_track_output_current_beat_value(&state),
                ),
                ("auto-follow", Value::Bool(true)),
                ("playhead", Value::Number(0.0)),
                ("transport-playhead", Value::Number(0.0)),
                ("sampler-playhead", Value::Number(0.0)),
                ("browser-preview-playing", Value::Bool(false)),
                ("browser-preview-playhead", Value::Number(0.0)),
                ("track-ids", build_track_ids(&app)),
                ("track-instrument-types", build_track_instrument_types(&app)),
                (
                    "track-mod-output-available",
                    build_track_mod_output_available(&app),
                ),
                (
                    "track-instrument-run-modes",
                    build_track_instrument_run_modes(&app),
                ),
                ("track-names", build_track_names(&track_names)),
                ("track-collapsed", build_track_collapsed(app)),
                (
                    "track-pattern-cells",
                    build_track_pattern_cells_value(&state, track_count),
                ),
                (
                    "track-num-steps",
                    build_all_track_num_steps_value(&state, app),
                ),
                (
                    "track-duration-spans",
                    build_all_track_duration_spans_value(&state, app),
                ),
                (
                    "track-step-plock-kinds",
                    build_all_track_step_plock_kinds(&state, app),
                ),
                (
                    "track-step-variant-r",
                    build_all_track_step_variant_color_channel(&state, app, 0),
                ),
                (
                    "track-step-variant-g",
                    build_all_track_step_variant_color_channel(&state, app, 1),
                ),
                (
                    "track-step-variant-b",
                    build_all_track_step_variant_color_channel(&state, app, 2),
                ),
                (
                    "steps",
                    if track_count == 0 {
                        Value::List(vec![])
                    } else {
                        build_steps_value(&state, 0)
                    },
                ),
                ("piano-roll-lanes", build_piano_roll_lanes_value()),
                (
                    "piano-roll-items",
                    if track_count == 0 {
                        Value::List(vec![])
                    } else {
                        build_piano_roll_items_value(
                            &PianoRollLanes::live(&state, 0),
                            &piano_roll_selection,
                        )
                    },
                ),
                (
                    "piano-roll-selection",
                    build_piano_roll_selection_value(&piano_roll_selection),
                ),
                ("focus-num-steps", Value::Number(16.0)),
                ("focus-label", Value::String(String::new())),
                ("focus-live", Value::Bool(true)),
                ("focus-kind", Value::Keyword("live".to_string())),
                ("focus-clip-kind", Value::Keyword("none".to_string())),
                ("focus-window-marker", Value::Number(-1.0)),
                ("focus-window-span", Value::Nil),
                ("focus-window-repeat", Value::Number(0.0)),
                ("focus-clip-start", Value::Nil),
                ("focus-clip-end", Value::Nil),
                ("focus-clip-offset", Value::Nil),
                ("piano-roll-playhead", Value::Number(-1.0)),
                (
                    "velocities",
                    if track_count == 0 {
                        Value::List(vec![])
                    } else {
                        build_param_list(&state, 0, StepParam::Velocity)
                    },
                ),
                (
                    "durations",
                    if track_count == 0 {
                        Value::List(vec![])
                    } else {
                        build_param_list(&state, 0, StepParam::Duration)
                    },
                ),
                (
                    "transposes",
                    if track_count == 0 {
                        Value::List(vec![])
                    } else {
                        build_param_list(&state, 0, StepParam::Transpose)
                    },
                ),
                (
                    "auxas",
                    if track_count == 0 {
                        Value::List(vec![])
                    } else {
                        build_param_list(&state, 0, StepParam::AuxA)
                    },
                ),
                (
                    "pans",
                    if track_count == 0 {
                        Value::List(vec![])
                    } else {
                        build_param_list(&state, 0, StepParam::Pan)
                    },
                ),
                (
                    "syncs",
                    if track_count == 0 {
                        Value::List(vec![])
                    } else {
                        build_param_list(&state, 0, StepParam::Sync)
                    },
                ),
                (
                    "delays",
                    if track_count == 0 {
                        Value::List(vec![])
                    } else {
                        build_param_list(&state, 0, StepParam::Delay)
                    },
                ),
                (
                    "process-lanes",
                    if track_count == 0 {
                        Value::List(vec![])
                    } else {
                        build_process_lanes_value(&state, 0)
                    },
                ),
                (
                    "track-process-lanes",
                    build_all_track_process_lanes_value(&state, track_count),
                ),
                (
                    "process-slots",
                    if track_count == 0 {
                        Value::List(vec![])
                    } else {
                        build_process_slots_value(&state, 0)
                    },
                ),
                (
                    "track-process-slots",
                    build_all_track_process_slots_value(&state, track_count),
                ),
                ("process-library", build_process_library_value(&state)),
                ("sync-labels", build_sync_labels()),
                ("track-volumes", build_track_volumes(&state)),
                (
                    "track-pans",
                    build_all_track_param_lists_value(&state, &app, StepParam::Pan),
                ),
                (
                    "track-delays",
                    build_all_track_param_lists_value(&state, &app, StepParam::Delay),
                ),
                ("track-mixer-pans", build_track_pans(&state)),
                ("track-outputs", build_track_outputs(&app, &state)),
                ("track-bus-sends", build_all_track_bus_sends(&app, &state)),
                ("mod-routes", build_mod_routes(&state)),
                ("track-mutes", build_track_mutes(&state)),
                ("track-solos", build_track_solos(&state)),
                ("track-muted-by-solo", build_track_muted_by_solo(&state)),
                (
                    "bus-ids",
                    Value::List(
                        app.buses
                            .iter()
                            .map(|bus| Rc::new(RefCell::new(Value::Number(bus.id.0 as f64))))
                            .collect(),
                    ),
                ),
                (
                    "bus-names",
                    build_track_names(
                        &app.buses
                            .iter()
                            .map(|bus| bus.name.clone())
                            .collect::<Vec<_>>(),
                    ),
                ),
                (
                    "bus-volumes",
                    Value::List(
                        app.buses
                            .iter()
                            .map(|bus| Rc::new(RefCell::new(Value::Number(bus.volume as f64))))
                            .collect(),
                    ),
                ),
                (
                    "bus-mutes",
                    Value::List(
                        app.buses
                            .iter()
                            .map(|bus| Rc::new(RefCell::new(Value::Bool(bus.mute))))
                            .collect(),
                    ),
                ),
                (
                    "bus-solos",
                    Value::List(
                        app.buses
                            .iter()
                            .map(|bus| Rc::new(RefCell::new(Value::Bool(bus.solo))))
                            .collect(),
                    ),
                ),
                ("bus-effects", build_bus_effects_value(&app)),
                (
                    "effects",
                    if track_count == 0 {
                        Value::List(vec![])
                    } else {
                        build_effects_value(&state, 0, &effect_descriptors, &selected_steps)
                    },
                ),
                (
                    "midi-effects",
                    if track_count == 0 {
                        Value::List(vec![])
                    } else {
                        build_midi_effects_value(&state, 0, &selected_steps)
                    },
                ),
                (
                    "instrument-panel",
                    if track_count == 0 {
                        Value::List(vec![])
                    } else {
                        build_instrument_panel_value(&app, 0, &selected_steps)
                    },
                ),
                ("instrument-active-notes", Value::List(vec![])),
                (
                    "track-active-notes",
                    build_track_active_notes_value(&state, track_count),
                ),
                ("track-params", build_track_params(&state, 0)),
                (
                    "tp-attack",
                    Value::Number(state.pattern.track_params[0].get_attack_ms() as f64),
                ),
                (
                    "tp-release",
                    Value::Number(state.pattern.track_params[0].get_release_ms() as f64),
                ),
                (
                    "tp-swing",
                    Value::Number(state.pattern.track_params[0].get_swing() as f64),
                ),
                (
                    "tp-send",
                    Value::Number(state.pattern.track_params[0].get_send() as f64),
                ),
                ("tp-output", {
                    let tp = &state.pattern.track_params[0];
                    let label = match tp.output() {
                        sequencer::sequencer::TrackOutput::Mix => "main".to_string(),
                        sequencer::sequencer::TrackOutput::None => "sends only".to_string(),
                        sequencer::sequencer::TrackOutput::Bus(id) => app
                            .buses
                            .iter()
                            .find(|bus| bus.id == id)
                            .map(|bus| bus.name.clone())
                            .unwrap_or_else(|| "main".to_string()),
                    };
                    Value::String(label)
                }),
                (
                    "track-output-options",
                    Value::List(
                        std::iter::once("main".to_string())
                            .chain(std::iter::once("sends only".to_string()))
                            .chain(
                                app.buses
                                    .iter()
                                    .filter(|bus| bus.id != sequencer::sequencer::BusId::MIX)
                                    .map(|bus| bus.name.clone()),
                            )
                            .map(|label| Rc::new(RefCell::new(Value::String(label))))
                            .collect(),
                    ),
                ),
                ("tp-bus-sends", {
                    use std::collections::HashMap;
                    let tp = &state.pattern.track_params[0];
                    let sends = tp.sends();
                    Value::List(
                        app.buses
                            .iter()
                            .enumerate()
                            .filter(|(_, bus)| bus.id != sequencer::sequencer::BusId::MIX)
                            .map(|(bus_idx, bus)| {
                                let amount = sends
                                    .iter()
                                    .find(|send| send.destination == bus.id)
                                    .map(|send| send.amount)
                                    .unwrap_or(0.0);
                                let mut map = HashMap::new();
                                map.insert(
                                    "bus-idx".to_string(),
                                    Rc::new(RefCell::new(Value::Number(bus_idx as f64))),
                                );
                                map.insert(
                                    "name".to_string(),
                                    Rc::new(RefCell::new(Value::String(bus.name.clone()))),
                                );
                                map.insert(
                                    "amount".to_string(),
                                    Rc::new(RefCell::new(Value::Number(amount as f64))),
                                );
                                Rc::new(RefCell::new(Value::Map(map)))
                            })
                            .collect(),
                    )
                }),
                (
                    "tp-num-steps",
                    Value::Number(state.pattern.track_params[0].get_num_steps() as f64),
                ),
                (
                    "tp-gate",
                    Value::Bool(state.pattern.track_params[0].is_gate_on()),
                ),
                (
                    "tp-poly",
                    Value::Bool(state.pattern.track_params[0].is_polyphonic()),
                ),
                (
                    "tp-timebase",
                    Value::String(
                        state.pattern.track_params[0]
                            .get_timebase()
                            .label()
                            .to_string(),
                    ),
                ),
                (
                    "tp-swing-resolution",
                    Value::String(
                        state.pattern.track_params[0]
                            .get_swing_resolution()
                            .label()
                            .to_string(),
                    ),
                ),
                (
                    "tp-fts",
                    Value::String(
                        FTS_SCALE_NAMES
                            .get(state.pattern.track_params[0].get_fts_scale())
                            .copied()
                            .unwrap_or("Off")
                            .to_string(),
                    ),
                ),
                (
                    "tp-mute-group",
                    Value::String(mute_group_label(
                        state.pattern.track_params[0].get_mute_group(),
                    )),
                ),
                (
                    "tp-accumulator",
                    Value::String(selected_accumulator_name(&app, 0)),
                ),
                (
                    "tp-accum-limit",
                    Value::Number(state.pattern.track_params[0].get_accum_limit() as f64),
                ),
                (
                    "tp-accum-mode",
                    Value::String(
                        accum_mode_label(state.pattern.track_params[0].get_accum_mode())
                            .to_string(),
                    ),
                ),
                ("accumulator-options", build_accumulator_options(&app)),
                ("fts-options", build_fts_options()),
                ("mute-group-options", build_mute_group_options()),
                ("accum-mode-options", build_accum_mode_options()),
                (
                    "available-builtin-effects",
                    build_available_builtin_effects(),
                ),
                ("available-effects", build_available_effects()),
                ("available-midi-effects", build_available_midi_effects()),
                ("selected-steps", build_selection_value(&selected_steps)),
                (
                    "step-has-plocks",
                    if track_count == 0 {
                        Value::List(vec![])
                    } else {
                        build_step_has_plocks(&state, 0, &effect_descriptors)
                    },
                ),
                (
                    "step-plock-kinds",
                    if track_count == 0 {
                        Value::List(vec![])
                    } else {
                        build_step_plock_kinds(&state, 0)
                    },
                ),
                (
                    "step-variant-r",
                    if track_count == 0 {
                        Value::List(vec![])
                    } else {
                        build_step_variant_color_channel(&state, 0, 0)
                    },
                ),
                (
                    "step-variant-g",
                    if track_count == 0 {
                        Value::List(vec![])
                    } else {
                        build_step_variant_color_channel(&state, 0, 1)
                    },
                ),
                (
                    "step-variant-b",
                    if track_count == 0 {
                        Value::List(vec![])
                    } else {
                        build_step_variant_color_channel(&state, 0, 2)
                    },
                ),
                (
                    "track-plocks",
                    if track_count == 0 {
                        Value::List(vec![])
                    } else {
                        build_track_plocks_value(&app, &state, 0, &selected_steps)
                    },
                ),
                (
                    "track-plock-variants",
                    if track_count == 0 {
                        Value::List(vec![])
                    } else {
                        build_track_plock_variants_value(&state, 0, &selected_steps)
                    },
                ),
                ("compiling", Value::Bool(false)),
                ("recording", Value::Bool(false)),
                ("master-recording", Value::Bool(false)),
                ("cpu-load-pct", Value::Number(0.0)),
                (
                    "output-latency-ms",
                    Value::Number((state.pdc_latency_seconds() * 1000.0) as f64),
                ),
                ("master-peak-l", Value::Number(0.0)),
                ("master-peak-r", Value::Number(0.0)),
                (
                    "record-armed",
                    build_record_armed_value(&record_armed.lock().unwrap()),
                ),
                ("eseq.seq-core-state/playhead-page", Value::Number(0.0)),
                ("sidebar-kind", Value::String("sampler".to_string())),
                ("sidebar-instrument-name", Value::String(String::new())),
                (
                    "sidebar-instrument-display-name",
                    Value::String(String::new()),
                ),
                ("sidebar-loaded-preset", Value::String(String::new())),
                ("sidebar-selected-sample", Value::String(String::new())),
                ("sidebar-track-index", Value::Number(0.0)),
                ("sidebar-presets", Value::List(vec![])),
                ("sidebar-preset-tree", Value::List(vec![])),
                (
                    "project-instrument-engines",
                    build_string_list(&project_instrument_engine_names(app)),
                ),
                ("sound-presets", build_sound_presets_value()),
                ("kit-presets", build_kit_presets_value()),
                ("current-project-name", Value::String(String::new())),
                // Editor mode state (for inline instrument/effect creation/editing)
                ("editor-active", Value::Bool(false)),
                ("editor-canceling", Value::Bool(false)),
                ("editor-error", Value::String(String::new())),
                ("editor-mode", Value::String(String::new())),
                ("editor-buffer-name", Value::String(String::new())),
                ("editor-active-macro-name", Value::String(String::new())),
                ("editor-active-macro-action", Value::String(String::new())),
                ("learn-target-path", Value::String(String::new())),
                ("learn-target-name", Value::String(String::new())),
                ("learn-phase", Value::String("pick".to_string())),
                ("learn-plan-params", Value::List(vec![])),
                ("learn-method", Value::String("Local fit + basin check".to_string())),
                ("learn-epochs", Value::Number(300.0)),
                ("learn-cma-generations", Value::Number(12.0)),
                ("learn-cma-population", Value::Number(0.0)),
                ("learn-cma-sigma", Value::Number(0.2)),
                ("learn-cma-seed", Value::Number(1.0)),
                ("learn-cma-forward-batch", Value::Number(0.0)),
                ("learn-local-epochs", Value::Number(0.0)),
                ("learn-cma-continue", Value::Number(8.0)),
                ("learn-cma-refine-epochs", Value::Number(5.0)),
                ("learn-cma-refine-mode", Value::String("Batched".to_string())),
                ("learn-cma-final-epochs", Value::Number(300.0)),
                ("learn-pitch-hz", Value::Number(0.0)),
                ("learn-gate-frames", Value::Number(0.0)),
                ("learn-stage", Value::String(String::new())),
                ("learn-current-epoch", Value::Number(0.0)),
                ("learn-total-epochs", Value::Number(0.0)),
                ("learn-loss", Value::Number(0.0)),
                ("learn-losses", Value::List(vec![])),
                ("learn-optimization-losses", Value::List(vec![])),
                ("learn-epoch-params", Value::List(vec![])),
                ("learn-checkpoint-wav", Value::String(String::new())),
                ("learn-improvement-pct", Value::Number(0.0)),
                ("learn-abs-distance", Value::Number(0.0)),
                ("learn-basin-check", Value::String(String::new())),
                ("learn-result-deltas", Value::List(vec![])),
                ("learn-final-wav", Value::String(String::new())),
                ("learn-error", Value::String(String::new())),
                ("editor-patch-macros", Value::List(vec![])),
                ("editor-library-macros", Value::List(vec![])),
                ("editor-open-macro", Value::String(String::new())),
                (
                    "editor-instrument-run-mode",
                    Value::String("instrument".to_string()),
                ),
            ];
            for idx in 0..track_count {
                fields.push((
                    Box::leak(track_selected_field(idx).into_boxed_str()),
                    Value::Bool(idx == 0),
                ));
                fields.push((
                    Box::leak(mixer_track_delete_target_field(idx).into_boxed_str()),
                    Value::Bool(false),
                ));
                for cell in state.track_pattern_cells(idx) {
                    let pattern_id = cell.pattern_id.0;
                    fields.push((
                        Box::leak(
                            track_pattern_cell_active_field(idx, pattern_id).into_boxed_str(),
                        ),
                        Value::Bool(cell.active_effective),
                    ));
                    fields.push((
                        Box::leak(
                            track_pattern_cell_assigned_field(idx, pattern_id).into_boxed_str(),
                        ),
                        Value::Bool(cell.assigned_to_current_scene),
                    ));
                    fields.push((
                        Box::leak(
                            track_pattern_cell_override_field(idx, pattern_id).into_boxed_str(),
                        ),
                        Value::Bool(cell.overridden),
                    ));
                    fields.push((
                        Box::leak(
                            track_pattern_cell_selected_field(idx, pattern_id).into_boxed_str(),
                        ),
                        Value::Bool(false),
                    ));
                }
                fields.push((
                    Box::leak(format!("track-peak-{idx}").into_boxed_str()),
                    Value::Number(0.0),
                ));
                fields.push((
                    Box::leak(format!("modulator-phase-{idx}").into_boxed_str()),
                    Value::Number(0.0),
                ));
                fields.push((
                    Box::leak(format!("modulator-level-{idx}").into_boxed_str()),
                    Value::Number(1.0),
                ));
            }
            for idx in 0..app.buses.len() {
                fields.push((
                    Box::leak(format!("bus-peak-{idx}").into_boxed_str()),
                    Value::Number(0.0),
                ));
            }
            for track in 0..track_count {
                for (bus_idx, bus) in app.buses.iter().enumerate() {
                    if bus.id == sequencer::sequencer::BusId::MIX {
                        continue;
                    }
                    fields.push((
                        Box::leak(track_bus_send_field(track, bus_idx).into_boxed_str()),
                        Value::Number(
                            track_bus_send_amount(&app, &state, track, bus_idx).unwrap_or(0.0)
                                as f64,
                        ),
                    ));
                }
            }
            if track_count > 0 {
                for (bus_idx, bus) in app.buses.iter().enumerate() {
                    if bus.id == sequencer::sequencer::BusId::MIX {
                        continue;
                    }
                    fields.push((
                        Box::leak(current_track_bus_send_field(bus_idx).into_boxed_str()),
                        Value::Number(
                            track_bus_send_amount(&app, &state, 0, bus_idx).unwrap_or(0.0) as f64,
                        ),
                    ));
                }
            }
            for idx in 0..MAX_STEPS {
                fields.push((
                    Box::leak(format!("playhead-active-{idx}").into_boxed_str()),
                    Value::Bool(idx == 0),
                ));
            }
            for track in 0..track_count {
                fields.push((
                    Box::leak(track_playhead_page_field(track).into_boxed_str()),
                    Value::Number((track_active_playhead_step(&state, track) / PAGE_SIZE) as f64),
                ));
                for step in 0..MAX_STEPS {
                    fields.push((
                        Box::leak(track_step_active_field(track, step).into_boxed_str()),
                        Value::Bool(state.pattern.patterns[track].is_active(step)),
                    ));
                    fields.push((
                        Box::leak(track_step_duration_field(track, step).into_boxed_str()),
                        Value::Bool(track_step_duration_covered(&state, track, step)),
                    ));
                    fields.push((
                        Box::leak(track_step_plocked_field(track, step).into_boxed_str()),
                        Value::Bool(false),
                    ));
                    fields.push((
                        Box::leak(track_step_selected_field(track, step).into_boxed_str()),
                        Value::Bool(false),
                    ));
                    fields.push((
                        Box::leak(track_playhead_active_field(track, step).into_boxed_str()),
                        Value::Bool(step == track_active_playhead_step(&state, track)),
                    ));
                }
            }
            fields
        },
        false,
    );
    runtime.register_reactive("SEQV", vec![], true);
    runtime.register_reactive("AGENT", vec![("generation", Value::Number(0.0))], false);
    if track_count > 0 {
        sync_fx_param_binding_fields(&mut runtime, app, &state, 0, &selected_steps);
    }

    // ── Native functions ──

    let delete_target = active_delete_target.clone();
    let delete_target_version = active_delete_target_version.clone();
    
    runtime.register_native("seq-set-delete-target", move |args, _ctx| {
        let (Some(kind), Some(payload)) = (args.first(), args.get(1)) else {
            return Err("seq-set-delete-target: expected (kind payload)".into());
        };
        let target = parse_delete_target(kind, payload)?;
        let mut guard = delete_target.lock().unwrap();
        if guard.as_ref() != Some(&target) {
            *guard = Some(target);
            bump_delete_target_version(&delete_target_version);
        }
        Ok(Value::Bool(true))
    });

    let delete_target = active_delete_target.clone();
    let delete_target_version = active_delete_target_version.clone();
    
    runtime.register_native("seq-clear-delete-target", move |_args, _ctx| {
        let mut guard = delete_target.lock().unwrap();
        if guard.take().is_some() {
            bump_delete_target_version(&delete_target_version);
        }
        Ok(Value::Bool(true))
    });

    let delete_target = active_delete_target.clone();
    runtime.register_native("seq-delete-target?", move |args, _ctx| {
        let (Some(kind), Some(payload)) = (args.first(), args.get(1)) else {
            return Err("seq-delete-target?: expected (kind payload)".into());
        };
        let target = parse_delete_target(kind, payload)?;
        Ok(Value::Bool(
            delete_target.lock().unwrap().as_ref() == Some(&target),
        ))
    });

    let delete_target = active_delete_target.clone();
    runtime.register_native("seq-active-delete-target-kind", move |_args, _ctx| {
        let guard = delete_target.lock().unwrap();
        Ok(active_delete_target_kind(guard.as_ref()))
    });

    let projection = expanded_step_projection.clone();
    let ui_inv = ui_invalidations.clone();
    runtime.register_native("seqv-sync-expanded-step-slots", move |args, _ctx| {
        let (
            Some(Value::Number(track)),
            Some(Value::Number(track_id)),
            Some(Value::Number(page)),
            Some(Value::Number(mode)),
            Some(Value::Number(cursor_step)),
        ) = (
            args.first(),
            args.get(1),
            args.get(2),
            args.get(3),
            args.get(4),
        )
        else {
            return Err(
                "seqv-sync-expanded-step-slots: expected (track track-id page mode cursor-step)"
                    .into(),
            );
        };
        let viewport =
            expanded_step_viewport_from_numbers(*track, *track_id, *page, *mode, *cursor_step)?;
        if projection.set_viewport(viewport) {
            ui_inv.push(UiInvalidation::ExpandedStepViewport {
                track: viewport.track,
                track_id: viewport.track_id,
            });
        }
        Ok(Value::Bool(true))
    });

    let projection = expanded_step_projection.clone();
    let ui_inv = ui_invalidations.clone();
    runtime.register_native("seqv-clear-expanded-step-slots", move |args, _ctx| {
        let Some(Value::Number(track_id)) = args.first() else {
            return Err("seqv-clear-expanded-step-slots: expected track-id".into());
        };
        if *track_id < 0.0 {
            return Err("seqv-clear-expanded-step-slots: track-id must be non-negative".into());
        }
        let track_id = *track_id as usize;
        if let Some(viewport) = projection.viewport(track_id) {
            if projection.remove_viewport(track_id) {
                ui_inv.push(UiInvalidation::ExpandedStepViewport {
                    track: viewport.track,
                    track_id,
                });
            }
        }
        Ok(Value::Bool(true))
    });

    let st = state.clone();
    let ct = current_track.clone();
    let groups = track_groups.clone();
    let delete_target = active_delete_target.clone();
    let delete_target_version = active_delete_target_version.clone();
    
    runtime.register_native("seq-delete-active-target", move |_args, ctx| {
        let target = delete_target.lock().unwrap().clone();
        let Some(target) = target else {
            return Ok(Value::Bool(false));
        };

        let current_buffer = ctx.current_buffer_name();
        match target {
            ActiveDeleteTarget::MixerTrack { track } => {
                if current_buffer != "*mixer*" {
                    return Ok(Value::Bool(false));
                }
                let track_count = st.active_track_count();
                if track_count <= 1 {
                    ctx.set_status("Cannot delete the last remaining track");
                    return Ok(Value::Bool(false));
                }
                if track >= track_count {
                    ctx.set_status(format!("Cannot delete missing track {}", track + 1));
                    let mut guard = delete_target.lock().unwrap();
                    if guard.take().is_some() {
                        bump_delete_target_version(&delete_target_version);
                    }
                    return Ok(Value::Bool(false));
                }
                let mut map = std::collections::HashMap::new();
                map.insert(
                    "track".to_string(),
                    Rc::new(RefCell::new(Value::Number(track as f64))),
                );
                ctx.enqueue_command(HostCommand::Custom {
                    name: "delete-track".to_string(),
                    payload: Value::Map(map),
                });
            }
            ActiveDeleteTarget::MixerTracks { tracks } => {
                if current_buffer != "*mixer*" {
                    return Ok(Value::Bool(false));
                }
                let track_count = st.active_track_count();
                let armed_track_count = tracks.len();
                let valid_tracks: Vec<usize> = tracks
                    .into_iter()
                    .filter(|track| *track < track_count)
                    .collect();
                if valid_tracks.len() < 2 {
                    ctx.set_status("Cannot delete tracks because the selection is stale");
                    let mut guard = delete_target.lock().unwrap();
                    if guard.take().is_some() {
                        bump_delete_target_version(&delete_target_version);
                    }
                    return Ok(Value::Bool(false));
                }
                if valid_tracks.len() == track_count {
                    ctx.set_status("Cannot delete all remaining tracks");
                    return Ok(Value::Bool(false));
                }
                if valid_tracks.len() != armed_track_count {
                    *delete_target.lock().unwrap() = Some(ActiveDeleteTarget::MixerTracks {
                        tracks: valid_tracks.clone(),
                    });
                    bump_delete_target_version(&delete_target_version);
                }
                let tracks = Value::List(
                    valid_tracks
                        .into_iter()
                        .map(|track| Rc::new(RefCell::new(Value::Number(track as f64))))
                        .collect(),
                );
                let mut map = std::collections::HashMap::new();
                map.insert("tracks".to_string(), Rc::new(RefCell::new(tracks)));
                ctx.enqueue_command(HostCommand::Custom {
                    name: "delete-tracks".to_string(),
                    payload: Value::Map(map),
                });
            }
            ActiveDeleteTarget::MixerGroup { group_id } => {
                if current_buffer != "*mixer*" {
                    return Ok(Value::Bool(false));
                }
                if !groups.lock().unwrap().iter().any(|group| group.id == group_id) {
                    ctx.set_status("Cannot delete missing track group");
                    let mut guard = delete_target.lock().unwrap();
                    if guard.take().is_some() {
                        bump_delete_target_version(&delete_target_version);
                    }
                    return Ok(Value::Bool(false));
                }
                let mut map = std::collections::HashMap::new();
                map.insert(
                    "group-id".to_string(),
                    Rc::new(RefCell::new(Value::Number(group_id as f64))),
                );
                ctx.enqueue_command(HostCommand::Custom {
                    name: "delete-track-group".to_string(),
                    payload: Value::Map(map),
                });
            }
            ActiveDeleteTarget::TrackPattern { track, pattern_id } => {
                if current_buffer != "*mixer*" {
                    return Ok(Value::Bool(false));
                }
                let valid = track < st.active_track_count()
                    && st
                        .track_pattern_cells(track)
                        .iter()
                        .any(|cell| cell.pattern_id == pattern_id);
                if !valid {
                    ctx.set_status("Cannot delete missing track pattern");
                    let mut guard = delete_target.lock().unwrap();
                    if guard.take().is_some() {
                        bump_delete_target_version(&delete_target_version);
                    }
                    return Ok(Value::Bool(false));
                }
                let mut map = std::collections::HashMap::new();
                map.insert(
                    "track".to_string(),
                    Rc::new(RefCell::new(Value::Number(track as f64))),
                );
                map.insert(
                    "pattern-id".to_string(),
                    Rc::new(RefCell::new(Value::Number(pattern_id.0 as f64))),
                );
                ctx.enqueue_command(HostCommand::Custom {
                    name: "delete-track-pattern".to_string(),
                    payload: Value::Map(map),
                });
            }
            ActiveDeleteTarget::ModRoute {
                source,
                destination,
                input,
            } => {
                if current_buffer != "*mixer*" {
                    return Ok(Value::Bool(false));
                }
                let route_exists = st.current_mod_connections().iter().any(|route| {
                    route.source_track == source
                        && route.destination == destination
                        && route.dest_input == input
                });
                if !route_exists {
                    let mut guard = delete_target.lock().unwrap();
                    if guard.take().is_some() {
                        bump_delete_target_version(&delete_target_version);
                    }
                    return Ok(Value::Bool(false));
                }
                let mut map = std::collections::HashMap::new();
                map.insert(
                    "source".to_string(),
                    Rc::new(RefCell::new(Value::Number(source as f64))),
                );
                map.insert(
                    "dest-kind".to_string(),
                    Rc::new(RefCell::new(match destination {
                        sequencer::sequencer::ModDestination::Track(_) => {
                            Value::String("track".to_string())
                        }
                        sequencer::sequencer::ModDestination::Bus(_) => {
                            Value::String("bus".to_string())
                        }
                    })),
                );
                map.insert(
                    "dest".to_string(),
                    Rc::new(RefCell::new(Value::Number(match destination {
                        sequencer::sequencer::ModDestination::Track(track) => track as f64,
                        sequencer::sequencer::ModDestination::Bus(bus) => bus.0 as f64,
                    }))),
                );
                map.insert(
                    "input".to_string(),
                    Rc::new(RefCell::new(Value::Number(input as f64))),
                );
                ctx.enqueue_command(HostCommand::Custom {
                    name: "delete-mod-route".to_string(),
                    payload: Value::Map(map),
                });
            }
            ActiveDeleteTarget::FxEffect { chain, bus, slot } => {
                if current_buffer != "*fx*" {
                    return Ok(Value::Bool(false));
                }
                let (name, payload) = match chain {
                    FxDeleteChain::Audio => {
                        let track = ct.load(Ordering::Relaxed);
                        let valid = track < st.active_track_count()
                            && slot < st.pattern.effect_chains[track].len();
                        if !valid {
                            ctx.set_status("Cannot delete missing audio effect");
                            let mut guard = delete_target.lock().unwrap();
                            if guard.take().is_some() {
                                bump_delete_target_version(&delete_target_version);
                            }
                            return Ok(Value::Bool(false));
                        }
                        let mut map = std::collections::HashMap::new();
                        map.insert(
                            "slot".to_string(),
                            Rc::new(RefCell::new(Value::Number(slot as f64))),
                        );
                        ("delete-effect".to_string(), Value::Map(map))
                    }
                    FxDeleteChain::Midi => {
                        let track = ct.load(Ordering::Relaxed);
                        let valid = track < st.active_track_count()
                            && st
                                .pattern
                                .track_params
                                .get(track)
                                .is_some_and(|params| slot < params.midi_fx_chain().len());
                        if !valid {
                            ctx.set_status("Cannot delete missing MIDI effect");
                            let mut guard = delete_target.lock().unwrap();
                            if guard.take().is_some() {
                                bump_delete_target_version(&delete_target_version);
                            }
                            return Ok(Value::Bool(false));
                        }
                        let mut map = std::collections::HashMap::new();
                        map.insert(
                            "slot".to_string(),
                            Rc::new(RefCell::new(Value::Number(slot as f64))),
                        );
                        ("delete-midi-fx".to_string(), Value::Map(map))
                    }
                    FxDeleteChain::Bus => {
                        let Some(bus) = bus else {
                            return Err("bus fx delete target is missing bus index".into());
                        };
                        let mut map = std::collections::HashMap::new();
                        map.insert(
                            "bus".to_string(),
                            Rc::new(RefCell::new(Value::Number(bus as f64))),
                        );
                        map.insert(
                            "slot".to_string(),
                            Rc::new(RefCell::new(Value::Number(slot as f64))),
                        );
                        ("delete-bus-effect".to_string(), Value::Map(map))
                    }
                };
                ctx.enqueue_command(HostCommand::Custom { name, payload });
            }
            ActiveDeleteTarget::RackEffect {
                track,
                rack_slot,
                effect_slot,
            } => {
                if current_buffer != "*fx*" {
                    return Ok(Value::Bool(false));
                }
                let valid = st
                    .pattern
                    .rack_tracks
                    .lock()
                    .unwrap()
                    .get(track)
                    .and_then(Option::as_ref)
                    .and_then(|rack| rack.slots.get(rack_slot))
                    .and_then(|slot| slot.effect_slots.get(effect_slot))
                    .is_some_and(|effect| effect.node_id != 0);
                if !valid {
                    ctx.set_status("Cannot delete missing rack-slot effect");
                    let mut guard = delete_target.lock().unwrap();
                    if guard.take().is_some() {
                        bump_delete_target_version(&delete_target_version);
                    }
                    return Ok(Value::Bool(false));
                }
                let mut map = std::collections::HashMap::new();
                map.insert(
                    "track".to_string(),
                    Rc::new(RefCell::new(Value::Number(track as f64))),
                );
                map.insert(
                    "rack-slot".to_string(),
                    Rc::new(RefCell::new(Value::Number(rack_slot as f64))),
                );
                map.insert(
                    "effect-slot".to_string(),
                    Rc::new(RefCell::new(Value::Number(effect_slot as f64))),
                );
                ctx.enqueue_command(HostCommand::Custom {
                    name: "delete-rack-slot-effect".to_string(),
                    payload: Value::Map(map),
                });
            }
            ActiveDeleteTarget::RackSlot { track, slot } => {
                if current_buffer != "*fx*" {
                    return Ok(Value::Bool(false));
                }
                let valid = track < st.active_track_count()
                    && st
                        .pattern
                        .rack_tracks
                        .lock()
                        .unwrap()
                        .get(track)
                        .and_then(|rack| rack.as_ref())
                        .is_some_and(|rack| slot < rack.slots.len());
                if !valid {
                    ctx.set_status("Cannot delete missing rack layer");
                    let mut guard = delete_target.lock().unwrap();
                    if guard.take().is_some() {
                        bump_delete_target_version(&delete_target_version);
                    }
                    return Ok(Value::Bool(false));
                }
                let mut map = std::collections::HashMap::new();
                map.insert(
                    "track".to_string(),
                    Rc::new(RefCell::new(Value::Number(track as f64))),
                );
                map.insert(
                    "slot".to_string(),
                    Rc::new(RefCell::new(Value::Number(slot as f64))),
                );
                ctx.enqueue_command(HostCommand::Custom {
                    name: "delete-rack-slot".to_string(),
                    payload: Value::Map(map),
                });
            }
        }

        let mut guard = delete_target.lock().unwrap();
        if guard.take().is_some() {
            bump_delete_target_version(&delete_target_version);
        }
        Ok(Value::Bool(true))
    });

    let st = state.clone();
    let delete_target = active_delete_target.clone();
    let delete_target_version = active_delete_target_version.clone();
    
    runtime.register_native("seq-clone-active-track-pattern", move |_args, ctx| {
        let target = delete_target.lock().unwrap().clone();
        let Some(ActiveDeleteTarget::TrackPattern { track, pattern_id }) = target else {
            ctx.set_status("Select a track pattern to clone");
            return Ok(Value::Bool(false));
        };
        if ctx.current_buffer_name() != "*mixer*" {
            return Ok(Value::Bool(false));
        }
        let valid = track < st.active_track_count()
            && st
                .track_pattern_cells(track)
                .iter()
                .any(|cell| cell.pattern_id == pattern_id);
        if !valid {
            ctx.set_status("Cannot clone missing track pattern");
            let mut guard = delete_target.lock().unwrap();
            if guard.take().is_some() {
                bump_delete_target_version(&delete_target_version);
            }
            return Ok(Value::Bool(false));
        }
        let mut map = std::collections::HashMap::new();
        map.insert(
            "track".to_string(),
            Rc::new(RefCell::new(Value::Number(track as f64))),
        );
        map.insert(
            "pattern-id".to_string(),
            Rc::new(RefCell::new(Value::Number(pattern_id.0 as f64))),
        );
        ctx.enqueue_command(HostCommand::Custom {
            name: "clone-track-pattern".to_string(),
            payload: Value::Map(map),
        });
        Ok(Value::Bool(true))
    });

    // now-ms — wall-clock milliseconds for gesture timing (hold-to-select)
    runtime.register_native("now-ms", |_args, _ctx| {
        Ok(Value::Number(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs_f64() * 1000.0)
                .unwrap_or(0.0),
        ))
    });

    // seq-toggle-step — toggle step on current track
    let st = state.clone();
    let ct = current_track.clone();
    runtime.register_native("seq-toggle-step", move |args, ctx| {
        let Some(Value::Number(step)) = args.first() else {
            return Err("seq-toggle-step: expected step number".into());
        };
        let step = *step as usize;
        if step >= MAX_STEPS {
            return Err(format!("seq-toggle-step: step {step} out of range").into());
        }
        let track = ct.load(Ordering::Relaxed);
        let next_active = !st.pattern.patterns[track].is_active(step);
        let mut payload = HashMap::new();
        payload.insert(
            "track".to_string(),
            Rc::new(RefCell::new(Value::Number(track as f64))),
        );
        payload.insert(
            "step".to_string(),
            Rc::new(RefCell::new(Value::Number(step as f64))),
        );
        ctx.enqueue_command(HostCommand::Custom {
            name: "toggle-step".to_string(),
            payload: Value::Map(payload),
        });
        Ok(Value::Bool(next_active))
    });

    // seq-toggle-track-step — toggle a step on a specific track (no track switch)
    let st = state.clone();
    runtime.register_native("seq-toggle-track-step", move |args, ctx| {
        let (Some(Value::Number(track)), Some(Value::Number(step))) = (args.first(), args.get(1))
        else {
            return Err("seq-toggle-track-step: expected (track step)".into());
        };
        let track = *track as usize;
        let step = *step as usize;
        if track >= st.active_track_count() {
            return Err(format!("seq-toggle-track-step: track {track} out of range").into());
        }
        if step >= MAX_STEPS {
            return Err(format!("seq-toggle-track-step: step {step} out of range").into());
        }
        let next_active = !st.pattern.patterns[track].is_active(step);
        let mut payload = HashMap::new();
        payload.insert(
            "track".to_string(),
            Rc::new(RefCell::new(Value::Number(track as f64))),
        );
        payload.insert(
            "step".to_string(),
            Rc::new(RefCell::new(Value::Number(step as f64))),
        );
        ctx.enqueue_command(HostCommand::Custom {
            name: "toggle-step".to_string(),
            payload: Value::Map(payload),
        });
        Ok(Value::Bool(next_active))
    });


    let st = state.clone();
    runtime.register_native("seq-track-step-active?", move |args, _ctx| {
        let (Some(Value::Number(track)), Some(Value::Number(step))) = (args.first(), args.get(1))
        else {
            return Err("seq-track-step-active?: expected (track step)".into());
        };
        let track = *track as usize;
        let step = *step as usize;
        if track >= st.active_track_count() || step >= MAX_STEPS {
            return Ok(Value::Bool(false));
        }
        Ok(Value::Bool(st.pattern.patterns[track].is_active(step)))
    });


    // seq-set-step-param — set param on current track
    let ct = current_track.clone();
    let sel = selected_steps.clone();
    let fx_ep = fx_epoch.clone();
    runtime.register_native("seq-set-step-param", move |args, ctx| {
        let (Some(Value::Number(step)), Some(Value::Keyword(param_name)), Some(Value::Number(val))) =
            (args.first(), args.get(1), args.get(2))
        else {
            return Err("seq-set-step-param: expected (step :param value)".into());
        };
        let step = *step as usize;
        if step >= MAX_STEPS {
            return Err(format!("seq-set-step-param: step {step} out of range").into());
        }
        let param = match param_name.as_str() {
            "velocity" | "vel" => StepParam::Velocity,
            "duration" | "dur" => StepParam::Duration,
            "aux-a" | "aux_a" | "auxa" | "axa" => StepParam::AuxA,
            "transpose" => StepParam::Transpose,
            "pan" => StepParam::Pan,
            "sync" | "syn" => StepParam::Sync,
            "delay" | "dly" => StepParam::Delay,
            "speed" => StepParam::Speed,
            other => return Err(format!("seq-set-step-param: unknown param :{other}").into()),
        };
        let track = ct.load(Ordering::Relaxed);
        let val = (*val as f32).clamp(param.min(), param.max());
        {
            let mut set = sel.lock().unwrap();
            if !set.is_empty() && !set.contains(&step) {
                set.clear();
                fx_ep.fetch_add(1, Ordering::Relaxed);
            }
        }
        let mut payload = HashMap::new();
        payload.insert("track".to_string(), Rc::new(RefCell::new(Value::Number(track as f64))));
        payload.insert("param".to_string(), Rc::new(RefCell::new(Value::Keyword(param_name.clone()))));
        payload.insert("value".to_string(), Rc::new(RefCell::new(Value::Number(val as f64))));
        payload.insert(
            "steps".to_string(),
            Rc::new(RefCell::new(Value::List(vec![Rc::new(RefCell::new(Value::Number(step as f64)))]))),
        );
        ctx.enqueue_command(HostCommand::Custom {
            name: "set-step-param-history".to_string(),
            payload: Value::Map(payload),
        });
        Ok(Value::Number(val as f64))
    });

    // seq-print-step-param — live step-param printing (bead eseq-jc9): same
    // signature as seq-set-step-param, but while playing+recording the value
    // latches into print mode and lands on the playhead's passing trigger
    // steps instead of the cursor step. The step argument is the cursor
    // fallback if the play/record gate raced off before dispatch. Never
    // clears the selection — printing targets the playhead, not the cursor.
    let ct = current_track.clone();
    runtime.register_native("seq-print-step-param", move |args, ctx| {
        let (Some(Value::Number(step)), Some(Value::Keyword(param_name)), Some(Value::Number(val))) =
            (args.first(), args.get(1), args.get(2))
        else {
            return Err("seq-print-step-param: expected (step :param value)".into());
        };
        let step = *step as usize;
        if step >= MAX_STEPS {
            return Err(format!("seq-print-step-param: step {step} out of range").into());
        }
        let param = match param_name.as_str() {
            "velocity" | "vel" => StepParam::Velocity,
            "duration" | "dur" => StepParam::Duration,
            "transpose" => StepParam::Transpose,
            other => {
                return Err(format!("seq-print-step-param: unknown param :{other}").into())
            }
        };
        let track = ct.load(Ordering::Relaxed);
        let val = (*val as f32).clamp(param.min(), param.max());
        let mut payload = HashMap::new();
        payload.insert("track".to_string(), Rc::new(RefCell::new(Value::Number(track as f64))));
        payload.insert("param".to_string(), Rc::new(RefCell::new(Value::Keyword(param_name.clone()))));
        payload.insert("value".to_string(), Rc::new(RefCell::new(Value::Number(val as f64))));
        payload.insert(
            "steps".to_string(),
            Rc::new(RefCell::new(Value::List(vec![Rc::new(RefCell::new(Value::Number(step as f64)))]))),
        );
        ctx.enqueue_command(HostCommand::Custom {
            name: "print-step-param".to_string(),
            payload: Value::Map(payload),
        });
        Ok(Value::Number(val as f64))
    });

    // seq-print-step-param-release — hold-to-print gesture end: the picker's
    // mouse-up releases that param's print latch immediately (the last held
    // param releasing ends print mode). Harmless when nothing is latched.
    runtime.register_native("seq-print-step-param-release", move |args, ctx| {
        let Some(Value::Keyword(param_name)) = args.first() else {
            return Err("seq-print-step-param-release: expected (:param)".into());
        };
        let mut payload = HashMap::new();
        payload.insert(
            "param".to_string(),
            Rc::new(RefCell::new(Value::Keyword(param_name.clone()))),
        );
        ctx.enqueue_command(HostCommand::Custom {
            name: "print-step-param-release".to_string(),
            payload: Value::Map(payload),
        });
        Ok(Value::Nil)
    });

    runtime.register_native("seq-set-process-lane-step", move |args, ctx| {
        let (
            Some(Value::Number(track)),
            Some(Value::Number(instance_id)),
            Some(inlet),
            Some(Value::Number(step)),
            Some(Value::Number(value)),
        ) = (
            args.first(),
            args.get(1),
            args.get(2),
            args.get(3),
            args.get(4),
        )
        else {
            return Err(
                "seq-set-process-lane-step: expected (track instance-id inlet step value)".into(),
            );
        };
        let track = *track as usize;
        let instance_id = *instance_id as u64;
        let inlet = value_symbol_name(inlet)
            .ok_or_else(|| "seq-set-process-lane-step: inlet must be a name".to_string())?;
        let step = *step as usize;
        let value = *value as f32;
        ctx.enqueue_command(process_history_command("set-lane-step", vec![
            ("track", Value::Number(track as f64)),
            ("instance-id", Value::Number(instance_id as f64)),
            ("inlet", Value::String(inlet)),
            ("step", Value::Number(step as f64)),
            ("value", Value::Number(value as f64)),
        ]));
        Ok(Value::Number(value as f64))
    });

    runtime.register_native("seq-clear-project-lane-override", move |args, ctx| {
        if args.len() != 3 {
            return Err(
                "seq-clear-project-lane-override: expected (track instance-id inlet)".into(),
            );
        }
        let Value::Number(track) = args[0] else {
            return Err("seq-clear-project-lane-override: track must be a number".into());
        };
        let Value::Number(instance_id) = args[1] else {
            return Err("seq-clear-project-lane-override: instance-id must be a number".into());
        };
        let inlet = value_symbol_name(&args[2])
            .ok_or_else(|| "seq-clear-project-lane-override: inlet must be a name".to_string())?;
        ctx.enqueue_command(process_history_command("clear-project-lane-override", vec![
            ("track", Value::Number(track)),
            ("instance-id", Value::Number(instance_id)),
            ("inlet", Value::String(inlet)),
        ]));
        Ok(Value::Bool(true))
    });

    runtime.register_native("seq-set-process-inlet", move |args, ctx| {
        let (
            Some(Value::Number(track)),
            Some(Value::Number(instance_id)),
            Some(inlet),
            Some(value),
        ) = (args.first(), args.get(1), args.get(2), args.get(3))
        else {
            return Err("seq-set-process-inlet: expected (track instance-id inlet value)".into());
        };
        let track = *track as usize;
        let instance_id = *instance_id as u64;
        let inlet = value_symbol_name(inlet)
            .ok_or_else(|| "seq-set-process-inlet: inlet must be a name".to_string())?;
        let _literal = match value {
            Value::Number(value) => sequencer::process::ProcessLiteral::Number(*value),
            Value::Bool(value) => sequencer::process::ProcessLiteral::Bool(*value),
            Value::String(value) => sequencer::process::ProcessLiteral::String(value.clone()),
            Value::Keyword(value) => sequencer::process::ProcessLiteral::Keyword(value.clone()),
            Value::Symbol(value) => sequencer::process::ProcessLiteral::Symbol(value.clone()),
            Value::Nil => sequencer::process::ProcessLiteral::Nil,
            other => {
                return Err(format!(
                    "seq-set-process-inlet: unsupported literal {}",
                    eseqlisp::vm::format_lisp_value(other)
                )
                .into());
            }
        };
        ctx.enqueue_command(process_history_command("set-inlet", vec![
            ("track", Value::Number(track as f64)),
            ("instance-id", Value::Number(instance_id as f64)),
            ("inlet", Value::String(inlet)),
            ("value", value.clone()),
        ]));
        Ok(Value::Bool(true))
    });

    runtime.register_native("seq-set-process-slot-enabled", move |args, ctx| {
        let (
            Some(Value::Number(track)),
            Some(Value::Number(instance_id)),
            Some(Value::Bool(enabled)),
        ) = (args.first(), args.get(1), args.get(2))
        else {
            return Err(
                "seq-set-process-slot-enabled: expected (track instance-id enabled)".into(),
            );
        };
        let track = *track as usize;
        ctx.enqueue_command(process_history_command("set-enabled", vec![
            ("track", Value::Number(track as f64)),
            ("instance-id", Value::Number(*instance_id)),
            ("enabled", Value::Bool(*enabled)),
        ]));
        Ok(Value::Bool(*enabled))
    });

    runtime.register_native("seq-move-process-slot-before", move |args, ctx| {
        let (Some(Value::Number(track)), Some(Value::Number(instance_id)), Some(target)) =
            (args.first(), args.get(1), args.get(2))
        else {
            return Err(
                "seq-move-process-slot-before: expected (track instance-id before-instance-id-or-nil)"
                    .into(),
            );
        };
        let track = *track as usize;
        let before = match target {
            Value::Nil => None,
            Value::Number(value) => Some(sequencer::process::ProcessInstanceId(*value as u64)),
            _ => {
                return Err(
                    "seq-move-process-slot-before: target must be an instance id or nil".into(),
                )
            }
        };
        ctx.enqueue_command(process_history_command("move-slot", vec![
            ("track", Value::Number(track as f64)),
            ("instance-id", Value::Number(*instance_id)),
            ("before-instance-id", before
                .map(|id| Value::Number(id.0 as f64))
                .unwrap_or(Value::Nil)),
        ]));
        Ok(Value::Bool(true))
    });

    runtime.register_native("seq-remove-process-slot", move |args, ctx| {
        let (Some(Value::Number(track)), Some(Value::Number(instance_id))) =
            (args.first(), args.get(1))
        else {
            return Err("seq-remove-process-slot: expected (track instance-id)".into());
        };
        let track = *track as usize;
        ctx.enqueue_command(process_history_command("remove-slot", vec![
            ("track", Value::Number(track as f64)),
            ("instance-id", Value::Number(*instance_id)),
        ]));
        Ok(Value::Bool(true))
    });

    let st = state.clone();
    runtime.register_native("seq-bind-process-port", move |args, ctx| {
        let (
            Some(Value::Number(track)),
            Some(Value::Number(instance_id)),
            Some(port),
            Some(target),
        ) = (args.first(), args.get(1), args.get(2), args.get(3))
        else {
            return Err(
                "seq-bind-process-port: expected (track instance-id port target-map)".into(),
            );
        };
        let track = *track as usize;
        let instance_id = sequencer::process::ProcessInstanceId(*instance_id as u64);
        let port = value_symbol_name(port)
            .ok_or_else(|| "seq-bind-process-port: port must be a name".to_string())?;
        let Some(port_def) = process_slot_port_def(&st, track, instance_id, &port) else {
            return Err(format!(
                "seq-bind-process-port: no matching process port {port:?} on track {}",
                track + 1
            )
            .into());
        };
        if !port_def.is_mappable() {
            return Err(format!(
                "seq-bind-process-port: process port {port:?} is not mappable"
            )
            .into());
        }
        let target_value = target.clone();
        let target = param_target_from_value(&st, track, target)
            .map_err(|error| format!("seq-bind-process-port: {error}"))?;
        if !port_def.allows_parameter_mapping_target(&target) {
            let target_kind = port_def
                .effective_target_kind()
                .map(|kind| kind.as_str())
                .unwrap_or("any compatible target");
            return Err(format!(
                "seq-bind-process-port: target is incompatible with port {port:?} kind {target_kind}"
            )
            .into());
        }
        if st.process_trace_enabled() {
            eprintln!(
                "[process-trace] bind track={} instance={} port={} target={:?}",
                track + 1,
                instance_id.0,
                port,
                target
            );
        }
        ctx.enqueue_command(process_history_command("bind-port", vec![
            ("track", Value::Number(track as f64)),
            ("instance-id", Value::Number(instance_id.0 as f64)),
            ("port", Value::String(port)),
            ("target", target_value),
        ]));
        Ok(Value::Bool(true))
    });

    runtime.register_native("seq-clear-process-port-binding", move |args, ctx| {
        let (Some(Value::Number(track)), Some(Value::Number(instance_id)), Some(port)) =
            (args.first(), args.get(1), args.get(2))
        else {
            return Err("seq-clear-process-port-binding: expected (track instance-id port)".into());
        };
        let track = *track as usize;
        let instance_id = sequencer::process::ProcessInstanceId(*instance_id as u64);
        let port = value_symbol_name(port)
            .ok_or_else(|| "seq-clear-process-port-binding: port must be a name".to_string())?;
        ctx.enqueue_command(process_history_command("clear-port-binding", vec![
            ("track", Value::Number(track as f64)),
            ("instance-id", Value::Number(instance_id.0 as f64)),
            ("port", Value::String(port)),
        ]));
        Ok(Value::Bool(true))
    });

    let st = state.clone();
    runtime.register_native("seq-set-process-trace", move |args, _ctx| {
        let enabled = match args.first() {
            Some(Value::Bool(value)) => *value,
            Some(Value::Number(value)) => *value != 0.0,
            Some(Value::Keyword(value))
            | Some(Value::Symbol(value))
            | Some(Value::String(value)) => {
                matches!(
                    value.trim_start_matches(':').to_ascii_lowercase().as_str(),
                    "true" | "on" | "yes" | "1"
                )
            }
            None => return Ok(Value::Bool(st.process_trace_enabled())),
            Some(other) => {
                return Err(format!(
                    "seq-set-process-trace: expected bool-ish value, got {}",
                    eseqlisp::vm::format_lisp_value(other)
                )
                .into());
            }
        };
        st.set_process_trace_enabled(enabled);
        eprintln!("[process-trace] enabled={enabled}");
        Ok(Value::Bool(enabled))
    });

    // Arrangement-timeline edit translator
    // (docs/arrangement-timeline-ui-spec.md 9): lowers one finished
    // scene-lane gesture into song editing-primitive host commands. View
    // actions are handled in Lisp; primitive validation/undo and rejection
    // reporting live in ui/host_commands/song.rs.
    let st = state.clone();
    runtime.register_native("seq-arrangement-action", move |args, ctx| {
        let Some(action) = args.first() else {
            return Err("seq-arrangement-action: expected action map".into());
        };
        let song = st.committed_song();
        let arrangement = st.committed_arrangement();
        let commands = crate::arrangement_actions::arrangement_action_song_commands(
            action,
            song.as_ref(),
            arrangement.as_ref(),
        )
        .map_err(|error| format!("seq-arrangement-action: {error}"))?;
        if commands.is_empty() {
            return Ok(Value::String("arrangement".to_string()));
        }
        let count = commands.len();
        for (name, payload) in commands {
            ctx.enqueue_command(HostCommand::Custom {
                name: name.to_string(),
                payload,
            });
        }
        let status = if count == 1 {
            "arrangement edit queued".to_string()
        } else {
            format!("arrangement: {count} edits queued")
        };
        ctx.set_status(status.clone());
        Ok(Value::String(status))
    });

    let st = state.clone();
    let ct = current_track.clone();
    let piano_sel = piano_roll_selection.clone();
    let piano_move = piano_roll_move_state.clone();
    let piano_clipboard = new_piano_roll_clipboard();
    let native_piano_clipboard = piano_clipboard.clone();
    let piano_focus = piano_roll_focus.clone();
    let ui_inv = ui_invalidations.clone();
    runtime.register_native("seq-piano-roll-action", move |args, ctx| {
        let Some(action) = args.first() else {
            return Err("seq-piano-roll-action: expected action map".into());
        };
        let track = ct.load(Ordering::Relaxed);
        if let Some(command) = piano_roll_gesture_command(action) {
            let mut payload = HashMap::new();
            payload.insert(
                "track".to_string(),
                Rc::new(RefCell::new(Value::Number(track as f64))),
            );
            payload.insert(
                "action".to_string(),
                Rc::new(RefCell::new(action.clone())),
            );
            let (name, status) = match command {
                PianoRollGestureCommand::Update(_) => {
                    ("piano-roll-gesture-update", "piano roll gesture preview")
                }
                PianoRollGestureCommand::Finish(_) => {
                    ("piano-roll-gesture-finish", "piano roll gesture finished")
                }
            };
            ctx.enqueue_command(HostCommand::Custom {
                name: name.to_string(),
                payload: Value::Map(payload),
            });
            ctx.set_status(status);
            return Ok(Value::String(status.to_string()));
        }
        // The shared focus cell is the reactive tick's projection of the
        // App-resolved edit target (clip-edit-target spec 3); the host
        // command layer re-resolves authoritatively before mutating.
        let focus = *piano_focus.lock().unwrap();
        let lanes = PianoRollLanes::new(&st, track, focus);
        if piano_roll_history_plan(&lanes, action, &native_piano_clipboard)?.is_some() {
            let mut payload = HashMap::new();
            payload.insert(
                "track".to_string(),
                Rc::new(RefCell::new(Value::Number(track as f64))),
            );
            payload.insert(
                "action".to_string(),
                Rc::new(RefCell::new(action.clone())),
            );
            ctx.enqueue_command(HostCommand::Custom {
                name: "piano-roll-history-action".to_string(),
                payload: Value::Map(payload),
            });
            let status = "piano roll edit queued".to_string();
            ctx.set_status(status.clone());
            return Ok(Value::String(status));
        }
        let mutates_pattern = piano_roll_action_mutates_pattern(action);
        if mutates_pattern {
            return Err(
                "seq-piano-roll-action: mutating action has no history transaction plan".into(),
            );
        }
        let status = apply_piano_roll_action_with_clipboard(
            &lanes,
            &piano_sel,
            &piano_move,
            &native_piano_clipboard,
            action,
        )?;
        ui_inv.push(UiInvalidation::PianoRoll {
            track,
            change: if mutates_pattern {
                PianoRollInvalidation::Items
            } else {
                PianoRollInvalidation::Selection
            },
        });
        ctx.set_status(status.clone());
        Ok(Value::String(status))
    });

    // seq-set-track — switch current track (single-select: resets the multi-select set)
    let st = state.clone();
    let ct = current_track.clone();
    let sel_tracks = selected_tracks.clone();
    let sel = selected_steps.clone();
    let piano_sel = piano_roll_selection.clone();
    let ui_ep = ui_epoch.clone();
    let fx_ep = fx_epoch.clone();
    let ui_inv = ui_invalidations.clone();
    let delete_target = active_delete_target.clone();
    let delete_target_version = active_delete_target_version.clone();
    runtime.register_native("seq-set-track", move |args, _ctx| {
        let Some(Value::Number(track)) = args.first() else {
            return Err("seq-set-track: expected track number".into());
        };
        let track = *track as usize;
        if track >= st.active_track_count() {
            return Err(format!("seq-set-track: track {track} out of range").into());
        }
        {
            let mut set = sel_tracks.lock().unwrap();
            set.clear();
            set.insert(track);
        }
        let previous = ct.load(Ordering::Relaxed);
        ct.store(track, Ordering::Relaxed);
        if previous != track {
            sel.lock().unwrap().clear();
            piano_sel.lock().unwrap().clear();
            let mut guard = delete_target.lock().unwrap();
            if matches!(
                guard.as_ref(),
                Some(ActiveDeleteTarget::TrackPattern { .. })
            ) {
                guard.take();
                bump_delete_target_version(&delete_target_version);
            }
            ui_inv.push(UiInvalidation::CurrentTrack {
                previous,
                current: track,
            });
            let next_fx_epoch = fx_ep.fetch_add(1, Ordering::Relaxed) + 1;
            if trace_ui_enabled() {
                eprintln!(
                    "[ui-trace][native] seq-set-track previous={} next={} fx_epoch={}",
                    previous, track, next_fx_epoch
                );
            }
        } else if trace_ui_enabled() {
            eprintln!(
                "[ui-trace][native] seq-set-track unchanged track={} ui_epoch={}",
                track,
                ui_ep.load(Ordering::Relaxed)
            );
        }
        Ok(Value::Number(track as f64))
    });

    // Atomic replacement primitives for contiguous model-index ranges and for
    // an explicit ordered UI range. The latter lets grouped surfaces select the
    // tracks they actually draw without teaching Rust their visual topology.
    register_track_selection_natives(
        &mut runtime,
        state.clone(),
        current_track.clone(),
        selected_tracks.clone(),
        active_delete_target.clone(),
        active_delete_target_version.clone(),
    );

    // seq-toggle-track-selected — (seq-toggle-track-selected track-idx)
    // cmd-click multi-select: toggle this track's membership in the selection set.
    // The last-clicked track becomes the focused current track. The set never
    // becomes empty (toggling off the sole member re-selects it).
    let st = state.clone();
    let ct = current_track.clone();
    let sel_tracks = selected_tracks.clone();
    let delete_target = active_delete_target.clone();
    let delete_target_version = active_delete_target_version.clone();
    runtime.register_native("seq-toggle-track-selected", move |args, _ctx| {
        let Some(Value::Number(track)) = args.first() else {
            return Err("seq-toggle-track-selected: expected track number".into());
        };
        let track = *track as usize;
        if track >= st.active_track_count() {
            return Err(format!("seq-toggle-track-selected: track {track} out of range").into());
        }
        let selected = {
            let mut set = sel_tracks.lock().unwrap();
            let selected = if set.contains(&track) {
                set.remove(&track);
                if set.is_empty() {
                    set.insert(track);
                    true
                } else {
                    false
                }
            } else {
                set.insert(track);
                true
            };
            arm_selected_tracks_delete_target(&set, &delete_target, &delete_target_version);
            selected
        };
        ct.store(track, Ordering::Relaxed);
        Ok(Value::Bool(selected))
    });

    // seq-toggle-group-collapsed — (seq-toggle-group-collapsed group-id)
    // Flips the collapsed flag on the in-memory group; the main loop rebuilds the
    // SEQ.group-collapsed / SEQ.groups reactive surfaces from the project groups.
    let groups_state = track_groups.clone();
    let ui_inv = ui_invalidations.clone();
    runtime.register_native("seq-toggle-group-collapsed", move |args, _ctx| {
        let Some(Value::Number(group_id)) = args.first() else {
            return Err("seq-toggle-group-collapsed: expected group id".into());
        };
        let group_id = *group_id as u64;
        let collapsed = {
            let mut groups = groups_state.lock().unwrap();
            match groups.iter_mut().find(|g| g.id == group_id) {
                Some(group) => {
                    group.collapsed = !group.collapsed;
                    group.collapsed
                }
                None => {
                    return Err(
                        format!("seq-toggle-group-collapsed: group {group_id} not found").into(),
                    );
                }
            }
        };
        ui_inv.push(UiInvalidation::BusTopology);
        Ok(Value::Bool(collapsed))
    });

    // seq-set-track-volume — (seq-set-track-volume track-idx volume)
    let st = state.clone();
    let ui_inv = ui_invalidations.clone();
    runtime.register_native("seq-set-track-volume", move |args, ctx| {
        let (Some(Value::Number(track)), Some(Value::Number(vol))) = (args.first(), args.get(1))
        else {
            return Err("seq-set-track-volume: expected (track volume)".into());
        };
        let track = *track as usize;
        if track >= st.active_track_count() {
            return Err(format!("seq-set-track-volume: track {track} out of range").into());
        }
        let vol = (*vol as f32).clamp(0.0, 1.0);
        ctx.enqueue_command(slice3_numeric_history_command(
            "volume",
            Some(track),
            vol as f64,
        ));
        ui_inv.push(UiInvalidation::TrackMixer {
            track,
            change: TrackMixerInvalidation::Volume,
        });
        // Push the post-FX fader straight to the audio graph so a drag is
        // heard immediately; the enqueued history command re-pushes the same
        // value when it lands, and remains the sole writer of sequencer state.
        let fader_id = st.runtime.delay_lids[track].load(Ordering::Acquire);
        if fader_id != 0 {
            unsafe {
                sequencer::audiograph::params_push_wrapper(
                    lg_raw,
                    sequencer::audiograph::ParamMsg {
                        idx: sequencer::effects::stereo_panner::STEREO_PANNER_PARAM_VOLUME,
                        logical_id: fader_id,
                        fvalue: sequencer::mixer_volume::fader_to_gain(vol),
                    },
                );
            }
        }
        Ok(Value::Number(vol as f64))
    });

    // seq-set-track-pan — (seq-set-track-pan track-idx pan)
    let st = state.clone();
    let pan_ids = track_pan_ids.clone();
    let ui_inv = ui_invalidations.clone();
    runtime.register_native("seq-set-track-pan", move |args, ctx| {
        let (Some(Value::Number(track)), Some(Value::Number(pan))) = (args.first(), args.get(1))
        else {
            return Err("seq-set-track-pan: expected (track pan)".into());
        };
        let track = *track as usize;
        if track >= st.active_track_count() {
            return Err(format!("seq-set-track-pan: track {track} out of range").into());
        }
        let pan = (*pan as f32).clamp(-1.0, 1.0);
        ctx.enqueue_command(slice3_numeric_history_command(
            "pan",
            Some(track),
            pan as f64,
        ));
        ui_inv.push(UiInvalidation::TrackMixer {
            track,
            change: TrackMixerInvalidation::Pan,
        });
        // Same zero-latency push as the volume fader (see above).
        let pan_ids_lock = pan_ids.lock().unwrap();
        if let Some(&pan_id) = pan_ids_lock.get(track) {
            unsafe {
                sequencer::audiograph::params_push_wrapper(
                    lg_raw,
                    sequencer::audiograph::ParamMsg {
                        idx: sequencer::effects::stereo_panner::STEREO_PANNER_PARAM_PAN,
                        logical_id: pan_id as u64,
                        fvalue: pan,
                    },
                );
            }
        }
        Ok(Value::Number(pan as f64))
    });

    // seq-toggle-track-mute — (seq-toggle-track-mute track-idx)
    let st = state.clone();
    let ui_inv = ui_invalidations.clone();
    runtime.register_native("seq-toggle-track-mute", move |args, ctx| {
        let Some(Value::Number(track)) = args.first() else {
            return Err("seq-toggle-track-mute: expected track".into());
        };
        let track = *track as usize;
        if track >= st.active_track_count() {
            return Err(format!("seq-toggle-track-mute: track {track} out of range").into());
        }
        let muted = !st.pattern.track_params[track].is_muted();
        ctx.enqueue_command(slice3_numeric_history_command(
            "toggle-mute", Some(track), 0.0,
        ));
        ui_inv.push(UiInvalidation::TrackMixer {
            track,
            change: TrackMixerInvalidation::Mute,
        });
        if trace_ui_enabled() {
            eprintln!(
                "[ui-trace][native] seq-toggle-track-mute track={} muted={}",
                track, muted
            );
        }
        Ok(Value::Bool(muted))
    });

    // seq-toggle-track-collapsed — (seq-toggle-track-collapsed track-idx)
    let st = state.clone();
    let collapsed_tracks = track_collapsed.clone();
    let ui_inv = ui_invalidations.clone();
    runtime.register_native("seq-toggle-track-collapsed", move |args, _ctx| {
        let Some(Value::Number(track)) = args.first() else {
            return Err("seq-toggle-track-collapsed: expected track".into());
        };
        let track = *track as usize;
        if track >= st.active_track_count() {
            return Err(format!("seq-toggle-track-collapsed: track {track} out of range").into());
        }
        let collapsed = {
            let mut tracks = collapsed_tracks.lock().unwrap();
            if tracks.len() < st.active_track_count() {
                tracks.resize(st.active_track_count(), false);
            }
            tracks[track] = !tracks[track];
            tracks[track]
        };
        ui_inv.push(UiInvalidation::TrackMixer {
            track,
            change: TrackMixerInvalidation::Collapsed,
        });
        Ok(Value::Bool(collapsed))
    });

    // seq-toggle-track-solo — (seq-toggle-track-solo track-idx)
    let st = state.clone();
    let ui_inv = ui_invalidations.clone();
    runtime.register_native("seq-toggle-track-solo", move |args, ctx| {
        let Some(Value::Number(track)) = args.first() else {
            return Err("seq-toggle-track-solo: expected track".into());
        };
        let track = *track as usize;
        if track >= st.active_track_count() {
            return Err(format!("seq-toggle-track-solo: track {track} out of range").into());
        }
        let solo = !st.pattern.track_params[track].is_solo();
        ctx.enqueue_command(slice3_numeric_history_command(
            "toggle-solo", Some(track), 0.0,
        ));
        ui_inv.push(UiInvalidation::TrackMixer {
            track,
            change: TrackMixerInvalidation::Solo,
        });
        if trace_ui_enabled() {
            eprintln!(
                "[ui-trace][native] seq-toggle-track-solo track={} solo={}",
                track, solo
            );
        }
        Ok(Value::Bool(solo))
    });

    let bus_state = buses.clone();
    let bus_nodes = bus_node_ids.clone();
    runtime.register_native("seq-set-bus-volume", move |args, ctx| {
        let (Some(Value::Number(bus_idx)), Some(Value::Number(vol))) = (args.first(), args.get(1))
        else {
            return Err("seq-set-bus-volume: expected (bus volume)".into());
        };
        let bus_idx = *bus_idx as usize;
        let vol = (*vol as f32).clamp(0.0, 1.0);
        let bus_id = {
            let buses = bus_state.lock().unwrap();
            let Some(bus) = buses.get(bus_idx) else {
                return Err(format!("seq-set-bus-volume: bus {bus_idx} out of range").into());
            };
            bus.id
        };
        ctx.enqueue_command(bus_mixer_history_command(
            "volume",
            bus_idx,
            bus_id,
            Some(vol as f64),
        ));
        // Zero-latency gain push, mirroring seq-set-track-volume: the history
        // command stays the sole writer of bus state and re-pushes this value.
        if let Some(nodes) = bus_nodes.lock().unwrap().get(bus_idx).cloned() {
            unsafe {
                sequencer::audiograph::params_push_wrapper(
                    lg_raw,
                    sequencer::audiograph::ParamMsg {
                        idx: sequencer::effects::stereo_panner::STEREO_PANNER_PARAM_VOLUME,
                        logical_id: nodes.volume_id as u64,
                        fvalue: sequencer::mixer_volume::fader_to_gain(vol),
                    },
                );
            }
        }
        Ok(Value::Number(vol as f64))
    });

    let bus_state = buses.clone();
    runtime.register_native("seq-toggle-bus-mute", move |args, ctx| {
        let Some(Value::Number(bus_idx)) = args.first() else {
            return Err("seq-toggle-bus-mute: expected bus".into());
        };
        let bus_idx = *bus_idx as usize;
        let (bus_id, muted) = {
            let buses = bus_state.lock().unwrap();
            let Some(bus) = buses.get(bus_idx) else {
                return Err(format!("seq-toggle-bus-mute: bus {bus_idx} out of range").into());
            };
            (bus.id, !bus.mute)
        };
        ctx.enqueue_command(bus_mixer_history_command(
            "toggle-mute",
            bus_idx,
            bus_id,
            None,
        ));
        Ok(Value::Bool(muted))
    });

    let bus_state = buses.clone();
    runtime.register_native("seq-toggle-bus-solo", move |args, ctx| {
        let Some(Value::Number(bus_idx)) = args.first() else {
            return Err("seq-toggle-bus-solo: expected bus".into());
        };
        let bus_idx = *bus_idx as usize;
        let (bus_id, solo) = {
            let buses = bus_state.lock().unwrap();
            let Some(bus) = buses.get(bus_idx) else {
                return Err(format!("seq-toggle-bus-solo: bus {bus_idx} out of range").into());
            };
            (bus.id, !bus.solo)
        };
        ctx.enqueue_command(bus_mixer_history_command(
            "toggle-solo",
            bus_idx,
            bus_id,
            None,
        ));
        Ok(Value::Bool(solo))
    });

    // seq-set-effect-param — (seq-set-effect-param slot-idx param-idx value)
    let st = state.clone();
    let ct = current_track.clone();
    let descs = effect_descriptors.clone();
    let auto_follow_override = auto_follow_override_until.clone();
    let ui_inv = ui_invalidations.clone();
    runtime.register_native("seq-set-effect-param", move |args, _ctx| {
        let (Some(Value::Number(slot)), Some(Value::Number(param)), Some(Value::Number(val))) =
            (args.first(), args.get(1), args.get(2))
        else {
            return Err("seq-set-effect-param: expected (slot param value)".into());
        };
        let track = ct.load(Ordering::Relaxed);
        let slot_idx = *slot as usize;
        let param_idx = *param as usize;
        let val = *val as f32;

        let chain = &st.pattern.effect_chains[track];
        let Some(slot_state) = chain.get(slot_idx) else {
            return Err(format!("seq-set-effect-param: slot {slot_idx} out of range").into());
        };

        // Clamp to descriptor range if available
        let clamped = descs
            .get(track)
            .and_then(|d| d.get(slot_idx))
            .and_then(|d| d.params.get(param_idx))
            .map(|p| val.clamp(p.min, p.max))
            .unwrap_or(val);

        slot_state.defaults.set(param_idx, clamped);

        if let Some((logical_id, idx)) = effect_param_target(slot_state, param_idx) {
            // Check for host_control — skip if present
            let skip = descs
                .get(track)
                .and_then(|d| d.get(slot_idx))
                .and_then(|d| d.params.get(param_idx))
                .and_then(|p| p.host_control.as_ref())
                .is_some();
            if !skip {
                unsafe {
                    sequencer::audiograph::params_push_wrapper(
                        lg_raw,
                        sequencer::audiograph::ParamMsg {
                            idx,
                            logical_id,
                            fvalue: clamped,
                        },
                    );
                }
            }
        }

        // Publish snapshot so the scheduler sees the new default
        // (otherwise it re-applies the old value on next step trigger)
        st.publish_scheduler_snapshot();
        *auto_follow_override.lock().unwrap() = Some(Instant::now() + AUTO_FOLLOW_COOLDOWN);
        ui_inv.push(UiInvalidation::TrackFx {
            track,
            change: TrackFxInvalidation::Param {
                slot: slot_idx,
                param: param_idx,
            },
        });
        Ok(Value::Number(clamped as f64))
    });

    // seq-set-effect-param-pair — (seq-set-effect-param-pair slot-idx param-a value-a param-b value-b)
    let st = state.clone();
    let ct = current_track.clone();
    let descs = effect_descriptors.clone();
    let auto_follow_override = auto_follow_override_until.clone();
    let ui_inv = ui_invalidations.clone();
    let reactive_bindings = runtime.reactive_binding_store();
    runtime.register_native("seq-set-effect-param-pair", move |args, _ctx| {
        let (
            Some(Value::Number(slot)),
            Some(Value::Number(param_a)),
            Some(Value::Number(val_a)),
            Some(Value::Number(param_b)),
            Some(Value::Number(val_b)),
        ) = (
            args.first(),
            args.get(1),
            args.get(2),
            args.get(3),
            args.get(4),
        )
        else {
            return Err(
                "seq-set-effect-param-pair: expected (slot param-a value-a param-b value-b)".into(),
            );
        };
        let track = ct.load(Ordering::Relaxed);
        let slot_idx = *slot as usize;
        let updates = [
            (*param_a as usize, *val_a as f32),
            (*param_b as usize, *val_b as f32),
        ];

        let chain = &st.pattern.effect_chains[track];
        let Some(slot_state) = chain.get(slot_idx) else {
            return Err(format!("seq-set-effect-param-pair: slot {slot_idx} out of range").into());
        };

        let mut clamped_values = Vec::with_capacity(updates.len());
        for (param_idx, val) in updates {
            let clamped = descs
                .get(track)
                .and_then(|d| d.get(slot_idx))
                .and_then(|d| d.params.get(param_idx))
                .map(|p| val.clamp(p.min, p.max))
                .unwrap_or(val);

            slot_state.defaults.set(param_idx, clamped);

            if let Some((logical_id, idx)) = effect_param_target(slot_state, param_idx) {
                let skip = descs
                    .get(track)
                    .and_then(|d| d.get(slot_idx))
                    .and_then(|d| d.params.get(param_idx))
                    .and_then(|p| p.host_control.as_ref())
                    .is_some();
                if !skip {
                    unsafe {
                        sequencer::audiograph::params_push_wrapper(
                            lg_raw,
                            sequencer::audiograph::ParamMsg {
                                idx,
                                logical_id,
                                fvalue: clamped,
                            },
                        );
                    }
                }
            }
            if let Some(name) = descs
                .get(track)
                .and_then(|d| d.get(slot_idx))
                .and_then(|d| d.params.get(param_idx))
                .map(|p| p.name.as_str())
            {
                reactive_bindings.write_float(
                    "SEQ",
                    &track_effect_param_value_field(track, slot_idx, param_idx, name),
                    clamped as f64,
                );
            }
            clamped_values.push(Value::Number(clamped as f64));
            ui_inv.push(UiInvalidation::TrackFx {
                track,
                change: TrackFxInvalidation::Param {
                    slot: slot_idx,
                    param: param_idx,
                },
            });
        }

        st.publish_scheduler_snapshot();
        *auto_follow_override.lock().unwrap() = Some(Instant::now() + AUTO_FOLLOW_COOLDOWN);
        Ok(Value::List(
            clamped_values
                .into_iter()
                .map(|value| Rc::new(RefCell::new(value)))
                .collect(),
        ))
    });

    // seq-set-effect-param-pair-live — push two effect params during a drag without
    // forcing the expensive FX reactive rebuild. The final commit should use
    // seq-set-effect-param-pair so the scheduler/UI snapshot catches up.
    let st = state.clone();
    let ct = current_track.clone();
    let descs = effect_descriptors.clone();
    let reactive_bindings = runtime.reactive_binding_store();
    runtime.register_native("seq-set-effect-param-pair-live", move |args, _ctx| {
        let (
            Some(Value::Number(slot)),
            Some(Value::Number(param_a)),
            Some(Value::Number(val_a)),
            Some(Value::Number(param_b)),
            Some(Value::Number(val_b)),
        ) = (
            args.first(),
            args.get(1),
            args.get(2),
            args.get(3),
            args.get(4),
        )
        else {
            return Err(
                "seq-set-effect-param-pair-live: expected (slot param-a value-a param-b value-b)"
                    .into(),
            );
        };
        let track = ct.load(Ordering::Relaxed);
        let slot_idx = *slot as usize;
        let updates = [
            (*param_a as usize, *val_a as f32),
            (*param_b as usize, *val_b as f32),
        ];

        let chain = &st.pattern.effect_chains[track];
        let Some(slot_state) = chain.get(slot_idx) else {
            return Err(
                format!("seq-set-effect-param-pair-live: slot {slot_idx} out of range").into(),
            );
        };

        let mut clamped_values = Vec::with_capacity(updates.len());
        for (param_idx, val) in updates {
            let clamped = descs
                .get(track)
                .and_then(|d| d.get(slot_idx))
                .and_then(|d| d.params.get(param_idx))
                .map(|p| val.clamp(p.min, p.max))
                .unwrap_or(val);

            slot_state.defaults.set(param_idx, clamped);

            if let Some((logical_id, idx)) = effect_param_target(slot_state, param_idx) {
                let skip = descs
                    .get(track)
                    .and_then(|d| d.get(slot_idx))
                    .and_then(|d| d.params.get(param_idx))
                    .and_then(|p| p.host_control.as_ref())
                    .is_some();
                if !skip {
                    unsafe {
                        sequencer::audiograph::params_push_wrapper(
                            lg_raw,
                            sequencer::audiograph::ParamMsg {
                                idx,
                                logical_id,
                                fvalue: clamped,
                            },
                        );
                    }
                }
            }
            if let Some(name) = descs
                .get(track)
                .and_then(|d| d.get(slot_idx))
                .and_then(|d| d.params.get(param_idx))
                .map(|p| p.name.as_str())
            {
                reactive_bindings.write_float(
                    "SEQ",
                    &track_effect_param_value_field(track, slot_idx, param_idx, name),
                    clamped as f64,
                );
            }
            clamped_values.push(Value::Number(clamped as f64));
        }

        Ok(Value::List(
            clamped_values
                .into_iter()
                .map(|value| Rc::new(RefCell::new(value)))
                .collect(),
        ))
    });









    // ── Selection natives ──

    // seq-select-step — toggle step in/out of selection
    let sel = selected_steps.clone();
    let ct = current_track.clone();
    let ui_inv = ui_invalidations.clone();
    runtime.register_native("seq-select-step", move |args, _ctx| {
        let Some(Value::Number(step)) = args.first() else {
            return Err("seq-select-step: expected step number".into());
        };
        let step = *step as usize;
        let mut set = sel.lock().unwrap();
        let was_selected = !set.insert(step);
        if was_selected {
            set.remove(&step);
        }
        ui_inv.push(UiInvalidation::StepSelection {
            track: ct.load(Ordering::Relaxed),
            changed_steps: vec![step],
        });
        Ok(Value::Bool(!was_selected))
    });

    // seq-select-step-range — replace selection with inclusive step range
    let st = state.clone();
    let ct = current_track.clone();
    let sel = selected_steps.clone();
    let ui_inv = ui_invalidations.clone();
    runtime.register_native("seq-select-step-range", move |args, _ctx| {
        let (Some(Value::Number(a)), Some(Value::Number(b))) = (args.first(), args.get(1)) else {
            return Err("seq-select-step-range: expected start and end steps".into());
        };
        let track = ct.load(Ordering::Relaxed);
        let num_steps = st.pattern.track_params[track].get_num_steps();
        if num_steps == 0 {
            return Ok(Value::Number(0.0));
        }
        let a = (*a as usize).min(num_steps - 1);
        let b = (*b as usize).min(num_steps - 1);
        let lo = a.min(b);
        let hi = a.max(b);
        let len = hi - lo + 1;
        let mut set = sel.lock().unwrap();
        if set.len() == len && (lo..=hi).all(|step| set.contains(&step)) {
            return Ok(Value::Number(len as f64));
        }
        let previous = std::mem::take(&mut *set);
        set.extend(lo..=hi);
        let mut changed_steps = previous
            .symmetric_difference(&set)
            .copied()
            .collect::<Vec<_>>();
        changed_steps.sort_unstable();
        drop(set);
        ui_inv.push(UiInvalidation::StepSelection {
            track,
            changed_steps,
        });
        Ok(Value::Number(len as f64))
    });

    // seq-clear-selection
    let sel = selected_steps.clone();
    let ct = current_track.clone();
    let ui_inv = ui_invalidations.clone();
    runtime.register_native("seq-clear-selection", move |_args, _ctx| {
        let mut selected = sel.lock().unwrap();
        if selected.is_empty() {
            return Ok(Value::Nil);
        }
        let mut changed_steps = selected.drain().collect::<Vec<_>>();
        changed_steps.sort_unstable();
        drop(selected);
        ui_inv.push(UiInvalidation::StepSelection {
            track: ct.load(Ordering::Relaxed),
            changed_steps,
        });
        Ok(Value::Nil)
    });

    // seq-has-selection?
    let sel = selected_steps.clone();
    runtime.register_native("seq-has-selection?", move |_args, _ctx| {
        Ok(Value::Bool(!sel.lock().unwrap().is_empty()))
    });

    let sel = selected_steps.clone();
    runtime.register_native("seq-step-selected?", move |args, _ctx| {
        let Some(Value::Number(step)) = args.first() else {
            return Err("seq-step-selected?: expected step number".into());
        };
        Ok(Value::Bool(sel.lock().unwrap().contains(&(*step as usize))))
    });

    let sel = selected_steps.clone();
    runtime.register_native("seq-selected-step-indexes-native", move |_args, _ctx| {
        let mut steps = sel.lock().unwrap().iter().copied().collect::<Vec<_>>();
        steps.sort_unstable();
        Ok(Value::List(
            steps
                .into_iter()
                .map(|step| Rc::new(RefCell::new(Value::Number(step as f64))))
                .collect(),
        ))
    });

    // seq-select-all-steps — select every step in the current track pattern
    let st = state.clone();
    let ct = current_track.clone();
    let sel = selected_steps.clone();
    let ui_inv = ui_invalidations.clone();
    runtime.register_native("seq-select-all-steps", move |_args, _ctx| {
        let track = ct.load(Ordering::Relaxed);
        let num_steps = st.pattern.track_params[track].get_num_steps();
        let mut set = sel.lock().unwrap();
        let previous = std::mem::take(&mut *set);
        set.extend(0..num_steps);
        let mut changed_steps = previous
            .symmetric_difference(&set)
            .copied()
            .collect::<Vec<_>>();
        changed_steps.sort_unstable();
        drop(set);
        ui_inv.push(UiInvalidation::StepSelection {
            track,
            changed_steps,
        });
        Ok(Value::Number(num_steps as f64))
    });

    // seq-delete-selected-steps — clear all selected steps and clear selection
    let ct = current_track.clone();
    let sel = selected_steps.clone();
    runtime.register_native("seq-delete-selected-steps", move |_args, ctx| {
        let track = ct.load(Ordering::Relaxed);
        let steps: Vec<usize> = {
            let set = sel.lock().unwrap();
            let mut steps: Vec<usize> = set.iter().copied().collect();
            steps.sort_unstable();
            steps
        };
        let mut payload = HashMap::new();
        payload.insert(
            "track".to_string(),
            Rc::new(RefCell::new(Value::Number(track as f64))),
        );
        ctx.enqueue_command(HostCommand::Custom {
            name: "delete-selected-steps".to_string(),
            payload: Value::Map(payload),
        });
        Ok(Value::Number(steps.len() as f64))
    });

    // seq-move-step-drag — move clicked step, or selected steps if clicked step is selected.
    let st = state.clone();
    let ct = current_track.clone();
    let sel = selected_steps.clone();
    runtime.register_native("seq-move-step-drag", move |args, ctx| {
        let (Some(Value::Number(start)), Some(Value::Number(target))) = (args.first(), args.get(1))
        else {
            return Err("seq-move-step-drag: expected start and target steps".into());
        };
        let start = *start as usize;
        let target = *target as usize;
        if start == target {
            return Ok(Value::Bool(false));
        }
        let track = ct.load(Ordering::Relaxed);
        let num_steps = st.pattern.track_params[track].get_num_steps();
        if start >= num_steps || target >= num_steps {
            return Ok(Value::Bool(false));
        }
        let delta = target as isize - start as isize;
        let mut move_selection = false;
        let steps: Vec<usize> = {
            let set = sel.lock().unwrap();
            if set.contains(&start) {
                move_selection = true;
                let mut steps: Vec<usize> = set.iter().copied().collect();
                steps.sort_unstable();
                steps
            } else {
                vec![start]
            }
        };
        if steps.is_empty() {
            return Ok(Value::Bool(false));
        }
        let Some(&first) = steps.first() else {
            return Ok(Value::Bool(false));
        };
        let Some(&last) = steps.last() else {
            return Ok(Value::Bool(false));
        };
        let new_first = first as isize + delta;
        let new_last = last as isize + delta;
        if new_first < 0 || new_last >= num_steps as isize {
            return Ok(Value::Bool(false));
        }
        let step_values = steps
            .iter()
            .map(|step| Rc::new(RefCell::new(Value::Number(*step as f64))))
            .collect();
        let mut payload = HashMap::new();
        payload.insert("track".to_string(), Rc::new(RefCell::new(Value::Number(track as f64))));
        payload.insert("steps".to_string(), Rc::new(RefCell::new(Value::List(step_values))));
        payload.insert("delta".to_string(), Rc::new(RefCell::new(Value::Number(delta as f64))));
        payload.insert("move-selection".to_string(), Rc::new(RefCell::new(Value::Bool(move_selection))));
        ctx.enqueue_command(HostCommand::Custom {
            name: "move-step-history".to_string(),
            payload: Value::Map(payload),
        });
        Ok(Value::Bool(true))
    });

    // seq-shift-selected-steps — rotate selected step payloads left/right in place
    let st = state.clone();
    let ct = current_track.clone();
    let sel = selected_steps.clone();
    runtime.register_native("seq-shift-selected-steps", move |args, ctx| {
        let Some(Value::Number(direction)) = args.first() else {
            return Err("seq-shift-selected-steps: expected direction".into());
        };
        let direction = (*direction).round() as isize;
        if direction == 0 {
            return Ok(Value::Nil);
        }
        let delta = direction.signum();
        let track = ct.load(Ordering::Relaxed);
        let steps: Vec<usize> = {
            let set = sel.lock().unwrap();
            let mut steps: Vec<usize> = set.iter().copied().collect();
            steps.sort_unstable();
            steps
        };
        if steps.is_empty() {
            return Ok(Value::Bool(false));
        }
        let num_steps = st.pattern.track_params[track].get_num_steps();
        let can_shift = if delta < 0 {
            steps[0] > 0
        } else {
            steps[steps.len() - 1] + 1 < num_steps
        };
        if !can_shift {
            return Ok(Value::Bool(false));
        }

        let step_values = steps
            .iter()
            .map(|step| Rc::new(RefCell::new(Value::Number(*step as f64))))
            .collect();
        let mut payload = HashMap::new();
        payload.insert("track".to_string(), Rc::new(RefCell::new(Value::Number(track as f64))));
        payload.insert("steps".to_string(), Rc::new(RefCell::new(Value::List(step_values))));
        payload.insert("delta".to_string(), Rc::new(RefCell::new(Value::Number(delta as f64))));
        payload.insert("move-selection".to_string(), Rc::new(RefCell::new(Value::Bool(true))));
        ctx.enqueue_command(HostCommand::Custom {
            name: "move-step-history".to_string(),
            payload: Value::Map(payload),
        });
        Ok(Value::Bool(true))
    });

    // seq-set-effect-plock — apply p-lock to ALL selected steps. Rendered
    // effect controls include their node identity so a callback created
    // before a chain reorder still follows the intended effect.
    let st = state.clone();
    let ct = current_track.clone();
    let sel = selected_steps.clone();
    runtime.register_native("seq-set-effect-plock", move |args, ctx| {
        let (Some(Value::Number(slot)), Some(Value::Number(param)), Some(Value::Number(val))) =
            (args.first(), args.get(1), args.get(2))
        else {
            return Err("seq-set-effect-plock: expected (slot param value [target-node-id])".into());
        };
        let track = ct.load(Ordering::Relaxed);
        let reported_slot = *slot as usize;
        let target_node_id = match args.get(3) {
            Some(Value::Number(node_id)) => Some(*node_id as u32),
            Some(_) => return Err("seq-set-effect-plock: target node id must be a number".into()),
            None => None,
        };
        let Some(slot_idx) = st.resolve_effect_slot_target(
            track,
            reported_slot,
            target_node_id,
        ) else {
            return Err("seq-set-effect-plock: effect target is no longer available".into());
        };
        let param_idx = *param as usize;
        let val = *val as f32;
        let mut steps = sel.lock().unwrap().iter().copied().collect::<Vec<_>>();
        steps.sort_unstable();
        if steps.is_empty() {
            return Ok(Value::Bool(false));
        }
        ctx.enqueue_command(effect_plock_history_command(
            track,
            &steps,
            slot_idx,
            &[(param_idx, val)],
        ));
        Ok(Value::Number(val as f64))
    });

    // seq-set-effect-plock-pair — apply two effect p-locks to ALL selected steps
    let ct = current_track.clone();
    let sel = selected_steps.clone();
    runtime.register_native("seq-set-effect-plock-pair", move |args, ctx| {
        let (
            Some(Value::Number(slot)),
            Some(Value::Number(param_a)),
            Some(Value::Number(val_a)),
            Some(Value::Number(param_b)),
            Some(Value::Number(val_b)),
        ) = (
            args.first(),
            args.get(1),
            args.get(2),
            args.get(3),
            args.get(4),
        )
        else {
            return Err(
                "seq-set-effect-plock-pair: expected (slot param-a value-a param-b value-b)".into(),
            );
        };
        let track = ct.load(Ordering::Relaxed);
        let slot_idx = *slot as usize;
        let updates = [
            (*param_a as usize, *val_a as f32),
            (*param_b as usize, *val_b as f32),
        ];
        let mut steps = sel.lock().unwrap().iter().copied().collect::<Vec<_>>();
        steps.sort_unstable();
        if steps.is_empty() {
            return Ok(Value::Bool(false));
        }
        ctx.enqueue_command(effect_plock_history_command(track, &steps, slot_idx, &updates));
        Ok(Value::Bool(true))
    });

    // seq-set-step-param-plock — apply step param p-lock to selected steps
    let ct = current_track.clone();
    let sel = selected_steps.clone();
    runtime.register_native("seq-set-step-param-plock", move |args, ctx| {
        let (Some(Value::Keyword(param_name)), Some(Value::Number(val))) =
            (args.first(), args.get(1))
        else {
            return Err("seq-set-step-param-plock: expected (:param value)".into());
        };
        let param = match param_name.as_str() {
            "velocity" | "vel" => StepParam::Velocity,
            "duration" | "dur" => StepParam::Duration,
            "aux-a" | "aux_a" | "auxa" | "axa" => StepParam::AuxA,
            "transpose" => StepParam::Transpose,
            "pan" => StepParam::Pan,
            "sync" | "syn" => StepParam::Sync,
            "delay" | "dly" => StepParam::Delay,
            "speed" => StepParam::Speed,
            other => return Err(format!("unknown param :{other}").into()),
        };
        let track = ct.load(Ordering::Relaxed);
        let val = (*val as f32).clamp(param.min(), param.max());
        let steps = sel.lock().unwrap().iter().copied().collect::<Vec<_>>();
        let step_values = steps
            .iter()
            .map(|step| Rc::new(RefCell::new(Value::Number(*step as f64))))
            .collect();
        let mut payload = HashMap::new();
        payload.insert("track".to_string(), Rc::new(RefCell::new(Value::Number(track as f64))));
        payload.insert("param".to_string(), Rc::new(RefCell::new(Value::Keyword(param_name.clone()))));
        payload.insert("value".to_string(), Rc::new(RefCell::new(Value::Number(val as f64))));
        payload.insert("steps".to_string(), Rc::new(RefCell::new(Value::List(step_values))));
        ctx.enqueue_command(HostCommand::Custom {
            name: "set-step-param-history".to_string(),
            payload: Value::Map(payload),
        });
        Ok(Value::Number(val as f64))
    });

    register_transport_toggle_play_native(&mut runtime, state.clone());

    runtime.register_native("seq-set-bpm", move |args, ctx| {
        let Some(Value::Number(bpm)) = args.first() else {
            return Err("seq-set-bpm: expected bpm number".into());
        };
        let bpm = (*bpm as u32).clamp(20, 300);
        ctx.enqueue_command(slice3_numeric_history_command("bpm", None, bpm as f64));
        Ok(Value::Number(bpm as f64))
    });

    // seq-set-track-param — set a track parameter on the current track
    let st = state.clone();
    let ct = current_track.clone();
    let sel = selected_steps.clone();
    let auto_follow_override = auto_follow_override_until.clone();
    let ui_inv = ui_invalidations.clone();
    runtime.register_native("seq-set-track-param", move |args, ctx| {
        let (Some(Value::Keyword(param_name)), Some(param_value)) = (args.first(), args.get(1))
        else {
            return Err("seq-set-track-param: expected (:param value)".into());
        };
        let numeric_value = match param_value {
            Value::Number(value) => Some(*value),
            _ => None,
        };
        let track = ct.load(Ordering::Relaxed);
        let tp = &st.pattern.track_params[track];
        let invalidation = match param_name.as_str() {
            "attack" => {
                let Some(val) = numeric_value else {
                    return Err("seq-set-track-param: :attack expects a number".into());
                };
                let v = (val as f32).clamp(0.0, 500.0);
                ctx.enqueue_command(slice3_numeric_history_command(
                    "attack", Some(track), v as f64,
                ));
                (TrackParamInvalidation::Attack, Ok(Value::Number(v as f64)))
            }
            "release" => {
                let Some(val) = numeric_value else {
                    return Err("seq-set-track-param: :release expects a number".into());
                };
                let v = (val as f32).clamp(0.0, 2000.0);
                ctx.enqueue_command(slice3_numeric_history_command(
                    "release", Some(track), v as f64,
                ));
                (TrackParamInvalidation::Release, Ok(Value::Number(v as f64)))
            }
            "swing" => {
                let Some(val) = numeric_value else {
                    return Err("seq-set-track-param: :swing expects a number".into());
                };
                let v = (val as f32).clamp(50.0, 75.0);
                let steps = sel.lock().unwrap();
                if steps.is_empty() {
                    ctx.enqueue_command(slice3_numeric_history_command(
                        "swing", Some(track), v as f64,
                    ));
                } else {
                    let values = steps.iter().map(|step| Rc::new(RefCell::new(Value::Number(*step as f64)))).collect();
                    let mut payload = HashMap::new();
                    payload.insert("op".to_string(), Rc::new(RefCell::new(Value::Keyword("swing-plock".to_string()))));
                    payload.insert("track".to_string(), Rc::new(RefCell::new(Value::Number(track as f64))));
                    payload.insert("value".to_string(), Rc::new(RefCell::new(Value::Number(v as f64))));
                    payload.insert("steps".to_string(), Rc::new(RefCell::new(Value::List(values))));
                    ctx.enqueue_command(HostCommand::Custom {
                        name: "slice2-history-action".to_string(),
                        payload: Value::Map(payload),
                    });
                    return Ok(Value::Number(v as f64));
                }
                (TrackParamInvalidation::Swing, Ok(Value::Number(v as f64)))
            }
            "num-steps" => {
                let Some(val) = numeric_value else {
                    return Err("seq-set-track-param: :num-steps expects a number".into());
                };
                let v = (val as usize).clamp(1, MAX_STEPS);
                let mut payload = HashMap::new();
                payload.insert("op".to_string(), Rc::new(RefCell::new(Value::Keyword("set-length".to_string()))));
                payload.insert("track".to_string(), Rc::new(RefCell::new(Value::Number(track as f64))));
                payload.insert("value".to_string(), Rc::new(RefCell::new(Value::Number(v as f64))));
                ctx.enqueue_command(HostCommand::Custom {
                    name: "slice2-history-action".to_string(),
                    payload: Value::Map(payload),
                });
                return Ok(Value::Number(v as f64));
            }
            "send" => {
                let Some(val) = numeric_value else {
                    return Err("seq-set-track-param: :send expects a number".into());
                };
                let v = (val as f32).clamp(0.0, 1.0);
                ctx.enqueue_command(slice3_numeric_history_command(
                    "send", Some(track), v as f64,
                ));
                (TrackParamInvalidation::Send, Ok(Value::Number(v as f64)))
            }
            "gate" => {
                let Some(val) = numeric_value else {
                    return Err("seq-set-track-param: :gate expects a number".into());
                };
                let want_on = val != 0.0;
                if want_on != tp.is_gate_on() {
                    ctx.enqueue_command(slice3_numeric_history_command(
                        "toggle-gate", Some(track), 0.0,
                    ));
                }
                (
                    TrackParamInvalidation::Gate,
                    Ok(Value::Bool(want_on)),
                )
            }
            "poly" => {
                let Some(val) = numeric_value else {
                    return Err("seq-set-track-param: :poly expects a number".into());
                };
                let want_on = val != 0.0;
                if want_on != tp.is_polyphonic() {
                    ctx.enqueue_command(slice3_numeric_history_command(
                        "toggle-poly", Some(track), 0.0,
                    ));
                }
                (
                    TrackParamInvalidation::Poly,
                    Ok(Value::Bool(want_on)),
                )
            }
            "max-poly" | "max-polyphony" | "voices" => {
                let Some(val) = numeric_value else {
                    return Err("seq-set-track-param: :max-poly expects a number".into());
                };
                let value = val.round().max(1.0) as usize;
                ctx.enqueue_command(slice3_numeric_history_command(
                    "max-polyphony", Some(track), value as f64,
                ));
                (
                    TrackParamInvalidation::MaxPolyphony,
                    Ok(Value::Number(value as f64)),
                )
            }
            "mute-group" => {
                let Some(val) = numeric_value else {
                    return Err("seq-set-track-param: :mute-group expects a number".into());
                };
                let value = val.round().clamp(0.0, 8.0) as u8;
                ctx.enqueue_command(slice3_numeric_history_command(
                    "mute-group", Some(track), value as f64,
                ));
                (
                    TrackParamInvalidation::MuteGroup,
                    Ok(Value::Number(value as f64)),
                )
            }
            "global-transpose" => {
                let enabled = match param_value {
                    Value::Bool(value) => *value,
                    Value::Number(value) => *value != 0.0,
                    _ => {
                        return Err(
                            "seq-set-track-param: :global-transpose expects a bool or number"
                                .into(),
                        );
                    }
                };
                ctx.enqueue_command(slice3_numeric_history_command(
                    "global-transpose",
                    Some(track),
                    if enabled { 1.0 } else { 0.0 },
                ));
                (
                    TrackParamInvalidation::GlobalTranspose,
                    Ok(Value::Bool(enabled)),
                )
            }
            other => return Err(format!("seq-set-track-param: unknown param :{other}").into()),
        };
        let result = invalidation.1;
        result.inspect(|_| {
            *auto_follow_override.lock().unwrap() = Some(Instant::now() + AUTO_FOLLOW_COOLDOWN);
            ui_inv.push(UiInvalidation::TrackParam {
                track,
                change: invalidation.0,
            });
        })
    });

    let ct = current_track.clone();
    let ui_ep = ui_epoch.clone();
    let accumulator_names_for_native = accumulator_names.clone();
    let auto_follow_override = auto_follow_override_until.clone();
    runtime.register_native("seq-set-accumulator", move |args, ctx| {
        let label = match args.first() {
            Some(Value::String(s)) => s.as_str(),
            _ => return Err("seq-set-accumulator: expected string label".into()),
        };
        let names = accumulator_names_for_native.lock().unwrap();
        let idx = names
            .iter()
            .position(|name| name.eq_ignore_ascii_case(label))
            .ok_or_else(|| format!("seq-set-accumulator: unknown accumulator '{label}'"))?;
        let track = ct.load(Ordering::Relaxed);
        let mut payload = HashMap::new();
        payload.insert("op".to_string(), Rc::new(RefCell::new(Value::Keyword("accumulator".to_string()))));
        payload.insert("track".to_string(), Rc::new(RefCell::new(Value::Number(track as f64))));
        payload.insert("value".to_string(), Rc::new(RefCell::new(Value::Number(idx as f64))));
        if idx < BUILTIN_ACCUMULATOR_NAMES.len() {
            payload.insert(
                "default-limit".to_string(),
                Rc::new(RefCell::new(Value::Number(
                    builtin_accumulator_default_limit(idx) as f64,
                ))),
            );
        } else {
            payload.insert(
                "script-name".to_string(),
                Rc::new(RefCell::new(Value::String(names[idx].clone()))),
            );
        }
        ctx.enqueue_command(HostCommand::Custom {
            name: "slice3-history-action".to_string(),
            payload: Value::Map(payload),
        });
        *auto_follow_override.lock().unwrap() = Some(Instant::now() + AUTO_FOLLOW_COOLDOWN);
        ui_ep.fetch_add(1, Ordering::Relaxed);
        Ok(Value::String(names[idx].clone()))
    });

    let accumulator_names_for_native = accumulator_names.clone();
    let debug_accum_preview = debug_accum;
    runtime.register_native("__register-accumulator-preview", move |args, _ctx| {
        let label = match args.first() {
            Some(Value::String(s)) => s.clone(),
            _ => return Err("__register-accumulator-preview: expected string label".into()),
        };
        let mut names = accumulator_names_for_native.lock().unwrap();
        if !names.iter().any(|name| name.eq_ignore_ascii_case(&label)) {
            names.push(label.clone());
        }
        if debug_accum_preview {
            eprintln!("[accum-ui] preview register label={label} names={names:?}");
        }
        Ok(Value::String(label))
    });

    register_ui_def_accumulator_dispatch(
        &mut runtime,
        Arc::clone(&accumulator_names),
        process_authoring_natives,
        debug_accum_preview,
    );

    let midi_fx_names_for_native = midi_fx_names.clone();
    let debug_midi_fx_preview = debug_accum;
    runtime.register_native("__register-midi-fx-preview", move |args, _ctx| {
        let label = match args.first() {
            Some(Value::String(s)) => s.clone(),
            _ => return Err("__register-midi-fx-preview: expected string label".into()),
        };
        let mut names = midi_fx_names_for_native.lock().unwrap();
        if !names.iter().any(|name| name.eq_ignore_ascii_case(&label)) {
            names.push(label.clone());
        }
        if debug_midi_fx_preview {
            eprintln!("[midi-fx-ui] preview register label={label} names={names:?}");
        }
        Ok(Value::String(label))
    });
    let _ = runtime.eval_str(
        r#"
        (defmacro def-midi-fx (name body)
          `(__register-midi-fx-preview ,name))
        "#,
    );

    // `def-sequencer` in the editor/UI runtime publishes its definition to the
    // scheduler VM (where the generator actually runs). The compiler auto-quotes
    // :tick / :init, so the body arrives here as list data — never evaluated in the
    // UI — which we serialize and ship via SequencerState. Re-evaluating the
    // authoring file republishes (upsert by id) for live hot-reload.
    let st_def_sequencer = state.clone();
    let ui_ep_def_sequencer = ui_epoch.clone();
    runtime.register_native_with_docs_and_keywords(
        "def-sequencer",
        sequencer::lisp_host::DEF_SEQUENCER_SIGNATURE,
        sequencer::lisp_host::DEF_SEQUENCER_DOCS,
        sequencer::lisp_host::DEF_SEQUENCER_KEYWORDS.iter().copied(),
        move |args, _ctx| {
            let published = sequencer::lisp_host::published_sequencer_from_def_args(&args)?;
            let name = published.name.clone();
            st_def_sequencer.publish_sequencer(published);
            ui_ep_def_sequencer.fetch_add(1, Ordering::Relaxed);
            Ok(Value::String(name))
        },
    );
    runtime.document_symbol_with_keywords(
        "seq-emit",
        sequencer::lisp_host::SEQ_EMIT_SIGNATURE,
        sequencer::lisp_host::SEQ_EMIT_DOCS,
        sequencer::lisp_host::SEQ_EMIT_KEYWORDS.iter().copied(),
    );

    register_song_natives(&mut runtime);

    let st_unpublish_sequencer = state.clone();
    let ui_ep_unpublish_sequencer = ui_epoch.clone();
    runtime.register_native("seq-unpublish-sequencer", move |args, _ctx| {
        let name = match args.first() {
            Some(Value::String(name) | Value::Symbol(name) | Value::Keyword(name)) => {
                name.trim_start_matches(':').trim_start_matches('@')
            }
            _ => return Err("seq-unpublish-sequencer expects a sequencer name".into()),
        };
        let removed = st_unpublish_sequencer.unpublish_sequencer_by_name(name);
        if removed {
            ui_ep_unpublish_sequencer.fetch_add(1, Ordering::Relaxed);
        }
        Ok(Value::Bool(removed))
    });

    let st = state.clone();
    let ct = current_track.clone();
    let ui_ep = ui_epoch.clone();
    let accumulator_names_for_native = accumulator_names.clone();
    let auto_follow_override = auto_follow_override_until.clone();
    let debug_accum_use = debug_accum;
    runtime.register_native("seq-use-accumulator", move |args, ctx| {
        let (track, label) = match args.as_slice() {
            [Value::String(label)] => (ct.load(Ordering::Relaxed), label.clone()),
            [Value::Number(track), Value::String(label)] if *track >= 0.0 => {
                (*track as usize, label.clone())
            }
            _ => {
                return Err(
                    "seq-use-accumulator: expected string label or track/string label".into(),
                );
            }
        };
        if track >= st.active_track_count() {
            return Err("seq-use-accumulator: track out of range".into());
        }

        let mut names = accumulator_names_for_native.lock().unwrap();
        if !names.iter().any(|name| name.eq_ignore_ascii_case(&label)) {
            names.push(label.clone());
        }
        let idx = names
            .iter()
            .position(|name| name.eq_ignore_ascii_case(&label))
            .ok_or_else(|| format!("seq-use-accumulator: unknown accumulator '{label}'"))?;

        let mut payload = HashMap::new();
        payload.insert("op".to_string(), Rc::new(RefCell::new(Value::Keyword("accumulator".to_string()))));
        payload.insert("track".to_string(), Rc::new(RefCell::new(Value::Number(track as f64))));
        payload.insert("value".to_string(), Rc::new(RefCell::new(Value::Number(idx as f64))));
        if idx < BUILTIN_ACCUMULATOR_NAMES.len() {
            payload.insert(
                "default-limit".to_string(),
                Rc::new(RefCell::new(Value::Number(
                    builtin_accumulator_default_limit(idx) as f64,
                ))),
            );
        } else {
            payload.insert(
                "script-name".to_string(),
                Rc::new(RefCell::new(Value::String(names[idx].clone()))),
            );
        }
        ctx.enqueue_command(HostCommand::Custom {
            name: "slice3-history-action".to_string(),
            payload: Value::Map(payload),
        });
        *auto_follow_override.lock().unwrap() = Some(Instant::now() + AUTO_FOLLOW_COOLDOWN);
        ui_ep.fetch_add(1, Ordering::Relaxed);
        if debug_accum_use {
            eprintln!(
                "[accum-ui] seq-use track={} label={} idx={} script={:?} names={:?}",
                track,
                label,
                idx,
                if idx < BUILTIN_ACCUMULATOR_NAMES.len() {
                    None
                } else {
                    Some(names[idx].clone())
                },
                *names
            );
        }
        Ok(Value::String(names[idx].clone()))
    });

    let st = state.clone();
    let ct = current_track.clone();
    let ui_ep = ui_epoch.clone();
    let midi_fx_names_for_native = midi_fx_names.clone();
    let auto_follow_override = auto_follow_override_until.clone();
    let debug_midi_fx_use = debug_accum;
    runtime.register_native("seq-use-midi-fx", move |args, ctx| {
        if args.is_empty() {
            return Err("seq-use-midi-fx: expected one or more MIDI FX names".into());
        }
        let (track, start_idx) = match args.first() {
            Some(Value::Number(track)) if *track >= 0.0 => (*track as usize, 1),
            _ => (ct.load(Ordering::Relaxed), 0),
        };
        if track >= st.active_track_count() {
            return Err("seq-use-midi-fx: track out of range".into());
        }
        let mut chain = Vec::new();
        for arg in args.iter().skip(start_idx) {
            match arg {
                Value::String(label) => chain.push(label.clone()),
                _ => return Err("seq-use-midi-fx: expected string MIDI FX names".into()),
            }
        }
        if chain.is_empty() {
            return Err("seq-use-midi-fx: expected at least one MIDI FX name".into());
        }
        let mut names = midi_fx_names_for_native.lock().unwrap();
        for label in &chain {
            if !names.iter().any(|name| name.eq_ignore_ascii_case(label)) {
                names.push(label.clone());
            }
        }
        ctx.enqueue_command(midi_fx_history_command(
            "set-chain",
            track,
            Value::List(chain.iter().cloned().map(|name| {
                Rc::new(RefCell::new(Value::String(name)))
            }).collect()),
        ));
        *auto_follow_override.lock().unwrap() = Some(Instant::now() + AUTO_FOLLOW_COOLDOWN);
        ui_ep.fetch_add(1, Ordering::Relaxed);
        if debug_midi_fx_use {
            eprintln!("[midi-fx-ui] seq-use track={} chain={:?}", track, chain);
        }
        Ok(Value::List(
            chain
                .into_iter()
                .map(|name| Rc::new(RefCell::new(Value::String(name))))
                .collect(),
        ))
    });

    let st = state.clone();
    let ct = current_track.clone();
    let ui_ep = ui_epoch.clone();
    runtime.register_native("seq-clear-midi-fx", move |args, ctx| {
        let track = match args.first() {
            Some(Value::Number(track)) if *track >= 0.0 => *track as usize,
            None => ct.load(Ordering::Relaxed),
            _ => return Err("seq-clear-midi-fx: expected no args or track".into()),
        };
        if track >= st.active_track_count() {
            return Err("seq-clear-midi-fx: track out of range".into());
        }
        ctx.enqueue_command(midi_fx_history_command(
            "set-chain",
            track,
            Value::List(Vec::new()),
        ));
        ui_ep.fetch_add(1, Ordering::Relaxed);
        Ok(Value::Bool(true))
    });

    let st = state.clone();
    let ct = current_track.clone();
    let ui_ep = ui_epoch.clone();
    runtime.register_native("seq-set-midi-fx-position", move |args, ctx| {
        if args.is_empty() {
            return Err("seq-set-midi-fx-position: expected position".into());
        }
        let (track, pos_idx) = match args.first() {
            Some(Value::Number(track)) if *track >= 0.0 => (*track as usize, 1),
            _ => (ct.load(Ordering::Relaxed), 0),
        };
        if track >= st.active_track_count() {
            return Err("seq-set-midi-fx-position: track out of range".into());
        }
        let position = match args.get(pos_idx) {
            Some(Value::Keyword(name)) | Some(Value::String(name))
                if name == "post-accumulator" || name == "post" =>
            {
                MidiFxPosition::PostAccumulator
            }
            Some(Value::Keyword(name)) | Some(Value::String(name))
                if name == "pre-accumulator" || name == "pre" =>
            {
                return Err(
                    "seq-set-midi-fx-position: pre-accumulator is not implemented yet".into(),
                );
            }
            _ => {
                return Err(
                    "seq-set-midi-fx-position: expected :pre-accumulator or :post-accumulator"
                        .into(),
                );
            }
        };
        ctx.enqueue_command(midi_fx_history_command(
            "set-position",
            track,
            Value::Keyword(match position {
                MidiFxPosition::PreAccumulator => "pre-accumulator",
                MidiFxPosition::PostAccumulator => "post-accumulator",
            }.to_string()),
        ));
        ui_ep.fetch_add(1, Ordering::Relaxed);
        Ok(Value::Bool(true))
    });

    let ct = current_track.clone();
    let ui_ep = ui_epoch.clone();
    let auto_follow_override = auto_follow_override_until.clone();
    runtime.register_native("seq-set-accum-mode", move |args, ctx| {
        let label = match args.first() {
            Some(Value::String(s)) => s.as_str(),
            _ => return Err("seq-set-accum-mode: expected string label".into()),
        };
        let mode = ACCUM_MODE_LABELS
            .iter()
            .position(|entry: &&str| entry.eq_ignore_ascii_case(label))
            .map(|idx| idx as u32)
            .ok_or_else(|| format!("seq-set-accum-mode: unknown mode '{label}'"))?;
        let track = ct.load(Ordering::Relaxed);
        ctx.enqueue_command(slice3_numeric_history_command(
            "accum-mode", Some(track), mode as f64,
        ));
        *auto_follow_override.lock().unwrap() = Some(Instant::now() + AUTO_FOLLOW_COOLDOWN);
        ui_ep.fetch_add(1, Ordering::Relaxed);
        Ok(Value::String(accum_mode_label(mode).to_string()))
    });

    let ct = current_track.clone();
    let ui_ep = ui_epoch.clone();
    let auto_follow_override = auto_follow_override_until.clone();
    runtime.register_native("seq-set-accum-limit", move |args, ctx| {
        let Some(Value::Number(limit)) = args.first() else {
            return Err("seq-set-accum-limit: expected number".into());
        };
        let limit = (*limit as f32).clamp(0.0, 127.0);
        let track = ct.load(Ordering::Relaxed);
        ctx.enqueue_command(slice3_numeric_history_command(
            "accum-limit", Some(track), limit as f64,
        ));
        *auto_follow_override.lock().unwrap() = Some(Instant::now() + AUTO_FOLLOW_COOLDOWN);
        ui_ep.fetch_add(1, Ordering::Relaxed);
        Ok(Value::Number(limit as f64))
    });

    // seq-double-track-pattern — duplicate current track pattern to double its length
    let st = state.clone();
    let ct = current_track.clone();
    runtime.register_native("seq-double-track-pattern", move |_args, ctx| {
        let track = ct.load(Ordering::Relaxed);
        let new_len = (st.pattern.track_params[track].get_num_steps() * 2).min(MAX_STEPS);
        let mut payload = HashMap::new();
        payload.insert("op".to_string(), Rc::new(RefCell::new(Value::Keyword("duplicate".to_string()))));
        payload.insert("track".to_string(), Rc::new(RefCell::new(Value::Number(track as f64))));
        ctx.enqueue_command(HostCommand::Custom {
            name: "slice2-history-action".to_string(),
            payload: Value::Map(payload),
        });
        Ok(Value::Number(new_len as f64))
    });

    // seq-halve-track-pattern — halve current track pattern length
    let st = state.clone();
    let ct = current_track.clone();
    runtime.register_native("seq-halve-track-pattern", move |_args, ctx| {
        let track = ct.load(Ordering::Relaxed);
        let new_len = (st.pattern.track_params[track].get_num_steps() / 2).max(1);
        let mut payload = HashMap::new();
        payload.insert("op".to_string(), Rc::new(RefCell::new(Value::Keyword("halve".to_string()))));
        payload.insert("track".to_string(), Rc::new(RefCell::new(Value::Number(track as f64))));
        ctx.enqueue_command(HostCommand::Custom {
            name: "slice2-history-action".to_string(),
            payload: Value::Map(payload),
        });
        Ok(Value::Number(new_len as f64))
    });

    // Rack headers select their backing bus. The host resolves that stable bus
    // to a rack group and applies the operation to all real member tracks.
    runtime.register_native("seq-resize-drum-rack-patterns", move |args, ctx| {
        let Some(Value::Number(bus)) = args.first() else {
            return Err("seq-resize-drum-rack-patterns: expected bus index".into());
        };
        if *bus < 0.0 {
            return Err(
                "seq-resize-drum-rack-patterns: bus index must be non-negative".into(),
            );
        }
        let operation = match args.get(1) {
            Some(Value::Keyword(operation))
            | Some(Value::String(operation))
            | Some(Value::Symbol(operation))
                if operation == "double" || operation == "halve" => operation.clone(),
            _ => {
                return Err(
                    "seq-resize-drum-rack-patterns: expected :double or :halve".into(),
                )
            }
        };
        let mut payload = HashMap::new();
        payload.insert(
            "bus".to_string(),
            Rc::new(RefCell::new(Value::Number(*bus))),
        );
        payload.insert(
            "op".to_string(),
            Rc::new(RefCell::new(Value::Keyword(operation))),
        );
        ctx.enqueue_command(HostCommand::Custom {
            name: "resize-drum-rack-patterns".to_string(),
            payload: Value::Map(payload),
        });
        Ok(Value::Bool(true))
    });

    let ct = current_track.clone();
    runtime.register_native(
        "seq-propagate-current-track-to-all-patterns",
        move |_args, ctx| {
            let track = ct.load(Ordering::Relaxed);
            ctx.enqueue_command(HostCommand::Custom {
                name: "propagate-current-track-to-all-patterns".to_string(),
                payload: Value::Number(track as f64),
            });
            Ok(Value::Bool(true))
        },
    );

    // seq-set-timebase — set the default timebase for the current track (by label string)
    let ct = current_track.clone();
    let ui_ep = ui_epoch.clone();
    let auto_follow_override = auto_follow_override_until.clone();
    runtime.register_native("seq-set-timebase", move |args, ctx| {
        let label = match args.first() {
            Some(Value::String(s)) => s.as_str(),
            _ => return Err("seq-set-timebase: expected string label".into()),
        };
        let normalized = label.to_ascii_lowercase();
        let tb = Timebase::LABELS
            .iter()
            .position(|l| l.to_ascii_lowercase() == normalized)
            .map(|i| Timebase::ALL[i])
            .ok_or_else(|| format!("seq-set-timebase: unknown timebase '{label}'"))?;
        let track = ct.load(Ordering::Relaxed);
        ctx.enqueue_command(slice3_numeric_history_command(
            "timebase", Some(track), tb as u32 as f64,
        ));
        *auto_follow_override.lock().unwrap() = Some(Instant::now() + AUTO_FOLLOW_COOLDOWN);
        ui_ep.fetch_add(1, Ordering::Relaxed);
        Ok(Value::String(tb.label().to_string()))
    });

    let ct = current_track.clone();
    let ui_ep = ui_epoch.clone();
    let auto_follow_override = auto_follow_override_until.clone();
    runtime.register_native("seq-set-fts", move |args, ctx| {
        let label = match args.first() {
            Some(Value::String(s)) => s.as_str(),
            _ => return Err("seq-set-fts: expected string label".into()),
        };
        let normalized = label.to_ascii_lowercase();
        let scale_idx = FTS_SCALE_NAMES
            .iter()
            .position(|scale| scale.to_ascii_lowercase() == normalized)
            .ok_or_else(|| format!("seq-set-fts: unknown scale '{label}'"))?;
        let track = ct.load(Ordering::Relaxed);
        ctx.enqueue_command(slice3_numeric_history_command(
            "fts", Some(track), scale_idx as f64,
        ));
        *auto_follow_override.lock().unwrap() = Some(Instant::now() + AUTO_FOLLOW_COOLDOWN);
        ui_ep.fetch_add(1, Ordering::Relaxed);
        Ok(Value::String(FTS_SCALE_NAMES[scale_idx].to_string()))
    });

    // seq-plock-timebase — set a timebase p-lock on selected steps
    let ct = current_track.clone();
    let sel = selected_steps.clone();
    runtime.register_native("seq-plock-timebase", move |args, ctx| {
        let label = match args.first() {
            Some(Value::String(s)) => s.as_str(),
            _ => return Err("seq-plock-timebase: expected string label".into()),
        };
        let normalized = label.to_ascii_lowercase();
        let tb = Timebase::LABELS
            .iter()
            .position(|l| l.to_ascii_lowercase() == normalized)
            .map(|i| Timebase::ALL[i])
            .ok_or_else(|| format!("seq-plock-timebase: unknown timebase '{label}'"))?;
        let track = ct.load(Ordering::Relaxed);
        let steps = sel.lock().unwrap().iter().copied().collect::<Vec<_>>();
        let values = steps.iter().map(|step| Rc::new(RefCell::new(Value::Number(*step as f64)))).collect();
        let mut payload = HashMap::new();
        payload.insert("op".to_string(), Rc::new(RefCell::new(Value::Keyword("timebase-plock".to_string()))));
        payload.insert("track".to_string(), Rc::new(RefCell::new(Value::Number(track as f64))));
        payload.insert("value".to_string(), Rc::new(RefCell::new(Value::Number(tb as u32 as f64))));
        payload.insert("steps".to_string(), Rc::new(RefCell::new(Value::List(values))));
        ctx.enqueue_command(HostCommand::Custom {
            name: "slice2-history-action".to_string(),
            payload: Value::Map(payload),
        });
        Ok(Value::String(tb.label().to_string()))
    });

    // seq-set-swing-resolution — set the default swing resolution for the current track (by label string)
    let ct = current_track.clone();
    let sel = selected_steps.clone();
    let ui_ep = ui_epoch.clone();
    let auto_follow_override = auto_follow_override_until.clone();
    runtime.register_native("seq-set-swing-resolution", move |args, ctx| {
        let label = match args.first() {
            Some(Value::String(s)) => s.as_str(),
            _ => return Err("seq-set-swing-resolution: expected string label".into()),
        };
        let normalized = label.to_ascii_lowercase();
        let resolution = SwingResolution::LABELS
            .iter()
            .position(|l| l.to_ascii_lowercase() == normalized)
            .map(|i| SwingResolution::ALL[i])
            .ok_or_else(|| format!("seq-set-swing-resolution: unknown resolution '{label}'"))?;
        let track = ct.load(Ordering::Relaxed);
        let steps = sel.lock().unwrap();
        if steps.is_empty() {
            ctx.enqueue_command(slice3_numeric_history_command(
                "swing-resolution",
                Some(track),
                resolution as u32 as f64,
            ));
        } else {
            let values = steps.iter().map(|step| Rc::new(RefCell::new(Value::Number(*step as f64)))).collect();
            let mut payload = HashMap::new();
            payload.insert("op".to_string(), Rc::new(RefCell::new(Value::Keyword("swing-resolution-plock".to_string()))));
            payload.insert("track".to_string(), Rc::new(RefCell::new(Value::Number(track as f64))));
            payload.insert("value".to_string(), Rc::new(RefCell::new(Value::Number(resolution as u32 as f64))));
            payload.insert("steps".to_string(), Rc::new(RefCell::new(Value::List(values))));
            ctx.enqueue_command(HostCommand::Custom {
                name: "slice2-history-action".to_string(),
                payload: Value::Map(payload),
            });
            return Ok(Value::String(resolution.label().to_string()));
        }
        *auto_follow_override.lock().unwrap() = Some(Instant::now() + AUTO_FOLLOW_COOLDOWN);
        ui_ep.fetch_add(1, Ordering::Relaxed);
        Ok(Value::String(resolution.label().to_string()))
    });

    let ui_ep = ui_epoch.clone();
    let auto_follow_override = auto_follow_override_until.clone();
    runtime.register_native("seq-pause-auto-follow", move |_args, _ctx| {
        let now = Instant::now();
        let was_following = {
            let mut guard = auto_follow_override.lock().unwrap();
            // Mirror `auto_follow_enabled`: no deadline, or an expired one,
            // means the playhead is still following.
            let was_following = guard.map_or(true, |until| now >= until);
            *guard = Some(now + AUTO_FOLLOW_COOLDOWN);
            was_following
        };
        // Only the FOLLOWING -> PAUSED transition changes a UI surface, and
        // `SEQ.auto-follow` has its own per-tick delta writer in
        // `reactive_tick` that re-reads this cell every frame. Re-arming an
        // already-paused cooldown is what every drag update after the first
        // does — `cool-off-follow` runs on every step/param edit — and bumping
        // ui_epoch there forced a whole-project resync (~7ms of
        // `sync_all_track_sequencer_state` + a `*sequencer*` layout refresh)
        // per pointer event.
        if was_following {
            ui_ep.fetch_add(1, Ordering::Relaxed);
        }
        Ok(Value::Bool(false))
    });

    // seq-toggle-record — toggle recording mode. No armed-track gate:
    // recording also drives arrangement capture (record + play records
    // scene/clip changes into the song), which needs no armed track. Armed
    // tracks only decide where live NOTES land.
    let rec = recording.clone();
    let ui_ep = ui_epoch.clone();
    runtime.register_native("seq-toggle-record", move |_args, _ctx| {
        let was = rec.load(Ordering::Relaxed);
        rec.store(!was, Ordering::Relaxed);
        ui_ep.fetch_add(1, Ordering::Relaxed);
        Ok(Value::Bool(!was))
    });

    let master_rec = master_recording.clone();
    let master = master_recorder.clone();
    let ui_ep = ui_epoch.clone();
    runtime.register_native("seq-toggle-master-recording", move |_args, ctx| {
        let result = toggle_master_recording_capture(&master_rec, &master);
        ui_ep.fetch_add(1, Ordering::Relaxed);
        match result {
            Ok((active, status)) => {
                ctx.set_status(status);
                Ok(Value::Bool(active))
            }
            Err(error) => Err(error.into()),
        }
    });

    // seq-toggle-record-arm — toggle record arm for a given track index
    let ra = record_armed.clone();
    let ar = armed_rack.clone();
    let groups_state = track_groups.clone();
    let ui_ep = ui_epoch.clone();
    runtime.register_native("seq-toggle-record-arm", move |args, _ctx| {
        let Some(Value::Number(track_idx)) = args.first() else {
            return Err("seq-toggle-record-arm: expected track index".into());
        };
        let track = *track_idx as usize;
        let armed = toggle_track_record_arm(
            &mut ra.lock().unwrap(),
            &mut ar.lock().unwrap(),
            &groups_state.lock().unwrap(),
            track,
        );
        match armed {
            // Disarming the last track no longer stops recording: record
            // mode stands on its own now (arrangement capture needs no
            // armed track).
            Some(armed) => {
                ui_ep.fetch_add(1, Ordering::Relaxed);
                Ok(Value::Bool(armed))
            }
            None => Ok(Value::Bool(false)),
        }
    });

    // seq-toggle-rack-arm — (seq-toggle-rack-arm group-id)
    // Arms a drum rack for pad play: the live keyboard stops playing tracks
    // chromatically and starts hitting this kit's pads. Only one rack is armed
    // at a time, and arming it disarms its own member tracks so a member never
    // answers a key both as a pad and chromatically.
    let ra = record_armed.clone();
    let ar = armed_rack.clone();
    let groups_state = track_groups.clone();
    let ui_ep = ui_epoch.clone();
    runtime.register_native("seq-toggle-rack-arm", move |args, _ctx| {
        let Some(Value::Number(group_id)) = args.first() else {
            return Err("seq-toggle-rack-arm: expected group id".into());
        };
        let group_id = *group_id as u64;
        let members = {
            let groups = groups_state.lock().unwrap();
            match groups
                .iter()
                .find(|group| group.id == group_id && group.is_rack())
            {
                Some(group) => group.members.clone(),
                None => {
                    return Err(format!("seq-toggle-rack-arm: rack {group_id} not found").into());
                }
            }
        };
        // Lock order matches `seq-toggle-record-arm`: record arms, then the
        // rack arm.
        let mut track_armed = ra.lock().unwrap();
        let mut rack = ar.lock().unwrap();
        let armed = toggle_rack_pad_arm(&mut rack, &mut track_armed, &members, group_id);
        ui_ep.fetch_add(1, Ordering::Relaxed);
        Ok(Value::Bool(armed))
    });

    let sample_db = Rc::new(
        sequencer::sample_db::SampleDb::open(
            &sequencer::app_paths::app_paths().sample_db_path(),
        )
        .expect("metal_seq requires the AppPaths sample database for sample browsing"),
    );
    eprintln!("metal_seq: sample db opened");

    let sample_db_for_search = sample_db.clone();
    runtime.register_native("seq-search-samples", move |args, _ctx| {
        let query = match args.first() {
            Some(Value::String(s)) => s.trim(),
            _ => "",
        };
        let rows = sample_db_for_search
            .query_samples_for_browser_limited(&[], (!query.is_empty()).then_some(query), 100)
            .map_err(|error| format!("failed to search samples.db: {error}"))?;
        Ok(Value::List(
            rows.into_iter()
                .map(|row| {
                    let name = row
                        .title
                        .as_deref()
                        .map(str::trim)
                        .filter(|title| !title.is_empty())
                        .unwrap_or(&row.hash)
                        .to_string();
                    Rc::new(RefCell::new(map_value([
                        ("name", Value::String(name)),
                        ("parent", Value::String(row.tags.join(", "))),
                        ("path", Value::String(format!("samples/{}.wav", row.hash))),
                    ])))
                })
                .collect(),
        ))
    });

    let sample_browser = Rc::new(RefCell::new(DebouncedSampleBrowser::new(
        sample_db.clone(),
        Duration::from_millis(150),
    )));
    let sample_browser_for_native = sample_browser.clone();
    runtime.register_native("seq-sample-browser", move |args, _ctx| {
        let query = match args.first() {
            Some(Value::String(s)) => s.as_str(),
            _ => "",
        };
        let selected_tags = value_string_list(args.get(1));
        let selected_tag_refs: Vec<&str> = selected_tags.iter().map(String::as_str).collect();
        let selected_origins = value_string_list(args.get(2));
        let selected_origin_refs: Vec<&str> = selected_origins.iter().map(String::as_str).collect();
        sample_browser_for_native
            .borrow_mut()
            .query_with_origins(query, &selected_tag_refs, &selected_origin_refs)
            .map_err(|error| format!("failed to query samples.db browser state: {error}"))
    });

    let sample_db_for_tree = sample_db.clone();
    runtime.register_native("seq-sample-tree", move |_args, _ctx| {
        build_sample_tree_value_from_db(&sample_db_for_tree, "", &[], &[])
            .map_err(|error| format!("failed to query samples.db sample tree: {error}"))
    });
    let sample_db_for_filter = sample_db.clone();
    runtime.register_native("seq-filter-sample-tree", move |args, _ctx| {
        let query = match args.first() {
            Some(Value::String(s)) => s.trim(),
            _ => "",
        };
        build_sample_tree_value_from_db(&sample_db_for_filter, query, &[], &[])
            .map_err(|error| format!("failed to filter samples.db sample tree: {error}"))
    });
    let sample_db_for_tags = sample_db.clone();
    runtime.register_native("seq-sample-tags-for-path", move |args, _ctx| {
        let path = match args.first() {
            Some(Value::String(path)) => std::path::Path::new(path),
            _ => return Ok(Value::List(vec![])),
        };
        let Some(hash) = path.file_stem().and_then(|stem| stem.to_str()) else {
            return Ok(Value::List(vec![]));
        };
        let tags = sample_db_for_tags
            .tags_for(hash)
            .map_err(|error| format!("failed to query sample tags for {hash}: {error}"))?;
        Ok(build_string_list(&tags))
    });
    // Tolerant waveform loader for the browser preview strip: reuses an
    // already-registered sample instead of re-decoding (unlike
    // `sample-load-wav`), and answers `false` instead of erroring so a
    // missing/undecodable file on tree-cursor moves never aborts the handler.
    runtime.register_native("seq-sample-waveform", move |args, _ctx| {
        let Some(Value::String(path)) = args.first() else {
            return Ok(Value::Bool(false));
        };
        if let Some(sample) = eseqlisp::audio::sample::get_registered_sample(path) {
            return Ok(sample.to_value());
        }
        match eseqlisp::audio::sample::SampleBuffer::load_wav(std::path::Path::new(path)) {
            Ok(sample) => Ok(sample.register().to_value()),
            Err(error) => {
                eprintln!("browser preview: failed to load waveform for {path}: {error}");
                Ok(Value::Bool(false))
            }
        }
    });
    runtime.register_native("seq-project-tree", move |args, _ctx| {
        let query = match args.first() {
            Some(Value::String(s)) => s.clone(),
            _ => String::new(),
        };
        Ok(build_project_tree(&query))
    });
    runtime.register_native("seq-script-tree", move |args, _ctx| {
        let query = match args.first() {
            Some(Value::String(s)) => s.as_str(),
            _ => "",
        };
        Ok(build_script_tree(query))
    });
    runtime.register_native("seq-preset-tree", move |args, _ctx| {
        let query = match args.get(1) {
            Some(Value::String(s)) => s.as_str(),
            _ => "",
        };
        Ok(build_preset_tree_from_list(args.first(), query))
    });
    runtime.register_native("seq-saved-instruments", move |_args, _ctx| {
        Ok(Value::List(
            sequencer::lisp_host::list_saved_instruments()
                .into_iter()
                .map(|name| Rc::new(RefCell::new(Value::String(name))))
                .collect(),
        ))
    });
    runtime.register_native("seq-saved-instrument-tree", move |args, _ctx| {
        let query = match args.first() {
            Some(Value::String(s)) => s.as_str(),
            _ => "",
        };
        let project_engines = value_string_list(args.get(1));
        Ok(build_instrument_tree_value(query, &project_engines))
    });
    runtime.register_native("seq-audio-effect-tree", move |args, _ctx| {
        let query = match args.first() {
            Some(Value::String(s)) => s.as_str(),
            _ => "",
        };
        Ok(build_audio_effect_tree(query))
    });
    runtime.register_native("seq-midi-effect-tree", move |args, _ctx| {
        let query = match args.first() {
            Some(Value::String(s)) => s.as_str(),
            _ => "",
        };
        Ok(build_midi_effect_tree(query))
    });
    register_agent_mode_natives(&mut runtime, app.agent_store.clone());
    document_metal_seq_natives(&mut runtime);

    RuntimeInit {
        runtime,
        accumulator_names,
        midi_fx_names,
        sample_browser,
        piano_roll_clipboard: piano_clipboard,
    }
}

fn register_agent_mode_natives(
    runtime: &mut Runtime,
    store: sequencer::agent::store::ConversationStore,
) {
    let s = store.clone();
    runtime.register_native("agent/new", move |args, _ctx| {
        let kind = parse_agent_kind(&args).unwrap_or(sequencer::agent::store::AgentKind::General);
        let id = s.new_conversation(kind);
        eprintln!("[agent-ui] agent/new kind={kind:?} conv={id}");
        Ok(Value::Number(id as f64))
    });

    let s = store.clone();
    runtime.register_native("agent/list", move |_args, _ctx| {
        Ok(Value::List(
            s.list()
                .into_iter()
                .map(|id| Rc::new(RefCell::new(Value::Number(id as f64))))
                .collect(),
        ))
    });

    let s = store.clone();
    runtime.register_native("agent/send", move |args, ctx| {
        let id = conv_id_arg(args.first())?;
        let prompt = match args.get(1) {
            Some(Value::String(value)) => value.clone(),
            _ => return Err("agent/send: expected conv-id and prompt string".to_string()),
        };
        eprintln!(
            "[agent-ui] agent/send conv={id} prompt_len={} prompt={:?}",
            prompt.len(),
            prompt
        );
        if s.snapshot(id).is_none() {
            return Err(format!("unknown agent conversation {id}"));
        }
        let mut payload = std::collections::HashMap::new();
        payload.insert(
            "id".to_string(),
            Rc::new(RefCell::new(Value::Number(id as f64))),
        );
        payload.insert(
            "prompt".to_string(),
            Rc::new(RefCell::new(Value::String(prompt))),
        );
        ctx.enqueue_command(HostCommand::Custom {
            name: "agent-send".to_string(),
            payload: Value::Map(payload),
        });
        Ok(Value::Nil)
    });

    let s = store.clone();
    runtime.register_native("agent/cancel", move |args, _ctx| {
        let id = conv_id_arg(args.first())?;
        eprintln!("[agent-ui] agent/cancel conv={id}");
        s.cancel(id)?;
        Ok(Value::Nil)
    });

    let s = store.clone();
    runtime.register_native("agent/discard", move |args, _ctx| {
        let id = conv_id_arg(args.first())?;
        eprintln!("[agent-ui] agent/discard conv={id}");
        s.discard(id)?;
        Ok(Value::Nil)
    });

    let s = store.clone();
    runtime.register_native("agent/close", move |args, _ctx| {
        let id = conv_id_arg(args.first())?;
        eprintln!("[agent-ui] agent/close conv={id}");
        s.close(id);
        Ok(Value::Nil)
    });

    let s = store.clone();
    runtime.register_native("agent/accept", move |args, ctx| {
        let id = conv_id_arg(args.first())?;
        eprintln!("[agent-ui] agent/accept conv={id}");
        ctx.enqueue_command(HostCommand::Custom {
            name: "agent-accept".to_string(),
            payload: Value::Number(id as f64),
        });
        // The host command performs graph mutation and returns status asynchronously.
        let _ = &s;
        Ok(Value::Number(id as f64))
    });

    runtime.register_native("agent/finalize", move |args, ctx| {
        let id = conv_id_arg(args.first())?;
        let name = match args.get(1) {
            Some(Value::String(value)) if !value.trim().is_empty() => value.clone(),
            _ => return Err("agent/finalize: expected conv-id and name string".to_string()),
        };
        eprintln!("[agent-ui] agent/finalize conv={id} name={name:?}");
        let mut payload = std::collections::HashMap::new();
        payload.insert(
            "id".to_string(),
            Rc::new(RefCell::new(Value::Number(id as f64))),
        );
        payload.insert(
            "name".to_string(),
            Rc::new(RefCell::new(Value::String(name))),
        );
        ctx.enqueue_command(HostCommand::Custom {
            name: "agent-finalize".to_string(),
            payload: Value::Map(payload),
        });
        Ok(Value::Number(id as f64))
    });

    let s = store.clone();
    runtime.register_native("agent/artifact", move |args, _ctx| {
        let state = snapshot_state(&s, args.first())?;
        Ok(agent_artifact_value(state))
    });

    let s = store.clone();
    runtime.register_native("agent/kind", move |args, _ctx| {
        let state = snapshot_state(&s, args.first())?;
        Ok(Value::Symbol(agent_kind_symbol(state.kind).to_string()))
    });

    let s = store.clone();
    runtime.register_native("agent/status", move |args, _ctx| {
        let state = snapshot_state(&s, args.first())?;
        Ok(Value::Symbol(agent_status_symbol(state.status).to_string()))
    });

    let s = store.clone();
    runtime.register_native("agent/messages", move |args, _ctx| {
        let state = snapshot_state(&s, args.first())?;
        Ok(Value::List(
            state
                .messages
                .into_iter()
                .map(agent_message_value)
                .map(|value| Rc::new(RefCell::new(value)))
                .collect(),
        ))
    });

    let s = store.clone();
    runtime.register_native("agent/draft-source", move |args, _ctx| {
        let state = snapshot_state(&s, args.first())?;
        Ok(state
            .draft
            .map(|draft| Value::String(draft.dsp_source))
            .or_else(|| {
                state
                    .effect_draft
                    .map(|draft| Value::String(draft.dsp_source))
            })
            .unwrap_or(Value::Nil))
    });

    let s = store.clone();
    runtime.register_native("agent/draft-ui-source", move |args, _ctx| {
        let state = snapshot_state(&s, args.first())?;
        Ok(state
            .draft
            .map(|draft| Value::String(draft.ui_source))
            .or_else(|| {
                state
                    .effect_draft
                    .map(|draft| Value::String(draft.ui_source))
            })
            .unwrap_or(Value::Nil))
    });

    let s = store.clone();
    runtime.register_native("agent/draft-handle", move |args, _ctx| {
        let state = snapshot_state(&s, args.first())?;
        Ok(if state.draft_handle.is_some() {
            Value::Number(state.id as f64)
        } else {
            Value::Nil
        })
    });

    let s = store.clone();
    runtime.register_native("agent/last-error", move |args, _ctx| {
        let state = snapshot_state(&s, args.first())?;
        Ok(state
            .last_compile_error
            .map(Value::String)
            .unwrap_or(Value::Nil))
    });

    let s = store.clone();
    runtime.register_native("agent/last-audition", move |args, _ctx| {
        let state = snapshot_state(&s, args.first())?;
        Ok(state
            .last_audition
            .map(agent_audition_value)
            .unwrap_or(Value::Nil))
    });

    let s = store.clone();
    runtime.register_native("agent/generation", move |args, _ctx| {
        let state = snapshot_state(&s, args.first())?;
        Ok(Value::Number(state.generation as f64))
    });

    // The patcher's agentic bubbles are one-shot turns, not conversations, so
    // their model is a process-global choice rather than a per-conversation
    // one (see agent::model_choice). `M-x choose-model` drives these two.
    runtime.register_native_with_docs(
        "agent/patch-model",
        "(agent/patch-model)",
        "The model patcher agentic bubbles (cmd+k) run on, or \"\" when unset.",
        move |_args, _ctx| {
            Ok(Value::String(
                sequencer::agent::model_choice::agentic_model().unwrap_or_default(),
            ))
        },
    );

    runtime.register_native_with_docs(
        "agent/set-patch-model",
        "(agent/set-patch-model model)",
        "Choose the model patcher agentic bubbles run on. Pass \"\" to reset.",
        move |args, _ctx| {
            let model = match args.first() {
                Some(Value::String(value)) => value.clone(),
                _ => return Err("agent/set-patch-model: expected a model string".to_string()),
            };
            if model.trim().is_empty() {
                sequencer::agent::model_choice::clear_agentic_model();
            } else {
                sequencer::agent::model_choice::set_agentic_model(&model)
                    .map_err(|error| format!("agent/set-patch-model: {error}"))?;
            }
            Ok(Value::Bool(true))
        },
    );

    runtime.register_native("agent/models", move |_args, _ctx| {
        Ok(Value::List(
            sequencer::agent::providers::default_model_presets()
                .into_iter()
                .map(|model| Rc::new(RefCell::new(Value::String(model.id))))
                .collect(),
        ))
    });

    let s = store.clone();
    runtime.register_native("agent/model", move |args, _ctx| {
        let state = snapshot_state(&s, args.first())?;
        Ok(Value::String(state.model))
    });

    let s = store;
    runtime.register_native("agent/set-model", move |args, _ctx| {
        let id = conv_id_arg(args.first())?;
        let model = match args.get(1) {
            Some(Value::String(value)) => value.clone(),
            _ => return Err("agent/set-model: expected conv-id and model string".to_string()),
        };
        let provider = sequencer::agent::providers::default_model_presets()
            .into_iter()
            .find(|preset| preset.id == model)
            .map(|preset| preset.provider)
            .unwrap_or(sequencer::agent::providers::AgentProviderKind::OpenAi);
        s.set_model(id, provider, model)?;
        Ok(Value::Nil)
    });
}

fn conv_id_arg(value: Option<&Value>) -> Result<sequencer::agent::store::ConvId, String> {
    match value {
        Some(Value::Number(id)) if *id >= 1.0 => Ok(*id as sequencer::agent::store::ConvId),
        _ => Err("expected agent conversation id".to_string()),
    }
}

fn snapshot_state(
    store: &sequencer::agent::store::ConversationStore,
    value: Option<&Value>,
) -> Result<sequencer::agent::store::ConversationState, String> {
    let id = conv_id_arg(value)?;
    store
        .snapshot(id)
        .map(|snapshot| snapshot.state)
        .ok_or_else(|| format!("unknown agent conversation {id}"))
}

fn parse_agent_kind(args: &[Value]) -> Option<sequencer::agent::store::AgentKind> {
    let candidate = args.iter().rev().find_map(|value| match value {
        Value::String(value) | Value::Symbol(value) | Value::Keyword(value) => Some(value.as_str()),
        _ => None,
    })?;
    match candidate.trim_start_matches(':') {
        "instrument" => Some(sequencer::agent::store::AgentKind::Instrument),
        "effect" => Some(sequencer::agent::store::AgentKind::Effect),
        "general" => Some(sequencer::agent::store::AgentKind::General),
        _ => None,
    }
}

fn agent_kind_symbol(kind: sequencer::agent::store::AgentKind) -> &'static str {
    match kind {
        sequencer::agent::store::AgentKind::General => "general",
        sequencer::agent::store::AgentKind::Instrument => "instrument",
        sequencer::agent::store::AgentKind::Effect => "effect",
    }
}

fn agent_status_symbol(status: sequencer::agent::store::AgentStatus) -> &'static str {
    match status {
        sequencer::agent::store::AgentStatus::Idle => "idle",
        sequencer::agent::store::AgentStatus::Streaming => "streaming",
        sequencer::agent::store::AgentStatus::Compiling => "compiling",
        sequencer::agent::store::AgentStatus::Auditioning => "auditioning",
        sequencer::agent::store::AgentStatus::Error => "error",
        sequencer::agent::store::AgentStatus::Cancelled => "cancelled",
    }
}

fn agent_role_symbol(role: sequencer::agent::store::Role) -> &'static str {
    match role {
        sequencer::agent::store::Role::User => "user",
        sequencer::agent::store::Role::Assistant => "assistant",
        sequencer::agent::store::Role::System => "system",
        sequencer::agent::store::Role::Tool => "tool",
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct AgentMessageDisplay {
    text: String,
    code_block_count: usize,
}

fn agent_message_display_text(text: &str) -> AgentMessageDisplay {
    let mut visible_lines = Vec::new();
    let mut in_fenced_block = false;
    let mut code_block_count = 0;

    for line in text.lines() {
        if line.trim_start().starts_with("```") {
            if !in_fenced_block {
                code_block_count += 1;
            }
            in_fenced_block = !in_fenced_block;
            continue;
        }

        if !in_fenced_block {
            visible_lines.push(line);
        }
    }

    let mut display = visible_lines.join("\n");
    display = display.trim().to_string();

    if code_block_count > 0 {
        if display.is_empty() {
            display = "Generated instrument source.".to_string();
        }
    }

    AgentMessageDisplay {
        text: display,
        code_block_count,
    }
}

fn agent_message_value(message: sequencer::agent::store::Message) -> Value {
    let display = agent_message_display_text(&message.text);
    let mut map = std::collections::HashMap::new();
    map.insert(
        "role".to_string(),
        Rc::new(RefCell::new(Value::Symbol(
            agent_role_symbol(message.role).to_string(),
        ))),
    );
    map.insert(
        "text".to_string(),
        Rc::new(RefCell::new(Value::String(message.text))),
    );
    map.insert(
        "display-text".to_string(),
        Rc::new(RefCell::new(Value::String(display.text))),
    );
    map.insert(
        "has-code-blocks".to_string(),
        Rc::new(RefCell::new(Value::Bool(display.code_block_count > 0))),
    );
    map.insert(
        "code-block-count".to_string(),
        Rc::new(RefCell::new(Value::Number(display.code_block_count as f64))),
    );
    let ts = message
        .ts
        .duration_since(std::time::SystemTime::UNIX_EPOCH)
        .map(|duration| duration.as_secs_f64())
        .unwrap_or(0.0);
    map.insert("ts".to_string(), Rc::new(RefCell::new(Value::Number(ts))));
    Value::Map(map)
}

fn agent_artifact_value(state: sequencer::agent::store::ConversationState) -> Value {
    fn display_name(name: &str) -> String {
        let trimmed = name.trim_end_matches('/');
        std::path::Path::new(trimmed)
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or(trimmed)
            .trim_end_matches(".lisp")
            .to_string()
    }

    let target_name = state
        .accepted_instrument_target
        .as_ref()
        .map(|target| target.instrument_name.clone());
    let stub_name = state
        .stub_instrument_target
        .as_ref()
        .map(|target| target.instrument_name.clone());
    let instrument_has_unapplied_draft =
        state.draft.is_some() && state.finalized_instrument_name.is_none();
    let draft_name = state
        .draft
        .as_ref()
        .map(|_| format!("agent-draft-{}/", state.id));
    let instrument_name = state
        .finalized_instrument_name
        .clone()
        .or(target_name)
        .or(draft_name)
        .or(stub_name);
    let effect_name = state
        .finalized_effect_name
        .clone()
        .or_else(|| state.effect_draft.as_ref().map(|draft| draft.name.clone()));
    let artifact_name = match state.kind {
        sequencer::agent::store::AgentKind::Instrument => instrument_name.clone(),
        sequencer::agent::store::AgentKind::Effect => effect_name.clone(),
        sequencer::agent::store::AgentKind::General => {
            instrument_name.clone().or_else(|| effect_name.clone())
        }
    };
    let unapplied_effect_draft = state.effect_draft.is_some()
        && !state.effect_draft_applied
        && state.finalized_effect_name.is_none();

    let mut map = std::collections::HashMap::new();
    let exists = artifact_name.is_some();
    map.insert(
        "exists".to_string(),
        Rc::new(RefCell::new(Value::Bool(exists))),
    );
    map.insert(
        "name".to_string(),
        Rc::new(RefCell::new(
            artifact_name
                .clone()
                .map(Value::String)
                .unwrap_or(Value::Nil),
        )),
    );
    map.insert(
        "display-name".to_string(),
        Rc::new(RefCell::new(
            artifact_name
                .as_ref()
                .map(|name| Value::String(display_name(name)))
                .unwrap_or(Value::Nil),
        )),
    );
    let status = match state.kind {
        sequencer::agent::store::AgentKind::Instrument => {
            if state.finalized_instrument_name.is_some() {
                "saved"
            } else if instrument_has_unapplied_draft && state.accepted_instrument_target.is_some() {
                "updated"
            } else if instrument_has_unapplied_draft {
                "ready"
            } else if state.accepted_instrument_target.is_some() {
                "applied"
            } else if state.stub_instrument_target.is_some() {
                "working"
            } else {
                "none"
            }
        }
        sequencer::agent::store::AgentKind::Effect => {
            if state.finalized_effect_name.is_some() {
                "saved"
            } else if unapplied_effect_draft && state.accepted_effect_target.is_some() {
                "updated"
            } else if unapplied_effect_draft {
                "ready"
            } else if state.accepted_effect_target.is_some() {
                "applied"
            } else {
                "none"
            }
        }
        sequencer::agent::store::AgentKind::General => {
            if state.finalized_instrument_name.is_some() || state.finalized_effect_name.is_some() {
                "saved"
            } else if instrument_has_unapplied_draft && state.accepted_instrument_target.is_some() {
                "updated"
            } else if unapplied_effect_draft && state.accepted_effect_target.is_some() {
                "updated"
            } else if instrument_has_unapplied_draft || unapplied_effect_draft {
                "ready"
            } else if state.accepted_instrument_target.is_some()
                || state.accepted_effect_target.is_some()
            {
                "applied"
            } else if state.stub_instrument_target.is_some() {
                "working"
            } else {
                "none"
            }
        }
    };
    map.insert(
        "status".to_string(),
        Rc::new(RefCell::new(Value::Symbol(status.to_string()))),
    );
    map.insert(
        "track".to_string(),
        Rc::new(RefCell::new(match state.kind {
            sequencer::agent::store::AgentKind::Instrument => state
                .accepted_instrument_target
                .as_ref()
                .or(state.stub_instrument_target.as_ref())
                .map(|target| Value::Number((target.track_index + 1) as f64))
                .unwrap_or(Value::Nil),
            sequencer::agent::store::AgentKind::Effect => state
                .accepted_effect_target
                .as_ref()
                .map(|target| Value::Number((target.track_index + 1) as f64))
                .unwrap_or(Value::Nil),
            sequencer::agent::store::AgentKind::General => state
                .accepted_instrument_target
                .as_ref()
                .or(state.stub_instrument_target.as_ref())
                .map(|target| Value::Number((target.track_index + 1) as f64))
                .or_else(|| {
                    state
                        .accepted_effect_target
                        .as_ref()
                        .map(|target| Value::Number((target.track_index + 1) as f64))
                })
                .unwrap_or(Value::Nil),
        })),
    );
    map.insert(
        "has-draft".to_string(),
        Rc::new(RefCell::new(Value::Bool(
            state.draft.is_some() || state.effect_draft.is_some(),
        ))),
    );
    map.insert(
        "can-finalize".to_string(),
        Rc::new(RefCell::new(Value::Bool(match state.kind {
            sequencer::agent::store::AgentKind::Instrument => {
                state.accepted_instrument_target.is_some()
                    && state.finalized_instrument_name.is_none()
            }
            sequencer::agent::store::AgentKind::Effect => {
                (state.effect_draft.is_some() || state.accepted_effect_target.is_some())
                    && state.finalized_effect_name.is_none()
            }
            sequencer::agent::store::AgentKind::General => {
                (state.accepted_instrument_target.is_some()
                    && state.finalized_instrument_name.is_none())
                    || ((state.effect_draft.is_some() || state.accepted_effect_target.is_some())
                        && state.finalized_effect_name.is_none())
            }
        }))),
    );
    map.insert(
        "can-apply".to_string(),
        Rc::new(RefCell::new(Value::Bool(match state.kind {
            sequencer::agent::store::AgentKind::Instrument => instrument_has_unapplied_draft,
            sequencer::agent::store::AgentKind::Effect => unapplied_effect_draft,
            sequencer::agent::store::AgentKind::General => {
                instrument_has_unapplied_draft || unapplied_effect_draft
            }
        }))),
    );
    map.insert(
        "apply-label".to_string(),
        Rc::new(RefCell::new(Value::String(
            if (instrument_has_unapplied_draft && state.accepted_instrument_target.is_some())
                || (unapplied_effect_draft && state.accepted_effect_target.is_some())
            {
                "Update artifact"
            } else {
                "Apply artifact"
            }
            .to_string(),
        ))),
    );
    Value::Map(map)
}

fn agent_audition_value(audition: sequencer::agent::store::AuditionResult) -> Value {
    let mut map = std::collections::HashMap::new();
    map.insert(
        "ran".to_string(),
        Rc::new(RefCell::new(Value::Bool(audition.ran))),
    );
    map.insert(
        "peak-db".to_string(),
        Rc::new(RefCell::new(Value::Number(audition.peak_db as f64))),
    );
    map.insert(
        "rms-db".to_string(),
        Rc::new(RefCell::new(Value::Number(audition.rms_db as f64))),
    );
    map.insert(
        "clipped".to_string(),
        Rc::new(RefCell::new(Value::Bool(audition.clipped))),
    );
    map.insert(
        "duration-ms".to_string(),
        Rc::new(RefCell::new(Value::Number(audition.duration_ms as f64))),
    );
    map.insert(
        "silent".to_string(),
        Rc::new(RefCell::new(Value::Bool(audition.silent))),
    );
    map.insert(
        "differs-from-input".to_string(),
        Rc::new(RefCell::new(
            audition
                .differs_from_input
                .map(Value::Bool)
                .unwrap_or(Value::Nil),
        )),
    );
    map.insert(
        "diff-rms-db".to_string(),
        Rc::new(RefCell::new(
            audition
                .diff_rms_db
                .map(|value| Value::Number(value as f64))
                .unwrap_or(Value::Nil),
        )),
    );
    Value::Map(map)
}

fn document_metal_seq_natives(runtime: &mut Runtime) {
    runtime.document_symbols([
        (
            "seq-toggle-step",
            "(seq-toggle-step step)",
            "Toggle the current track's step on/off and clear that step's p-locks.",
        ),
        (
            "seq-toggle-track-step",
            "(seq-toggle-track-step track step)",
            "Toggle a step on a specific track without changing the current track.",
        ),
        (
            "seq-set-step-param",
            "(seq-set-step-param step :param value)",
            "Set a per-step parameter on the current track.",
        ),
        (
            "seq-set-process-lane-step",
            "(seq-set-process-lane-step track instance-id inlet step value)",
            "Set one value in an attached process lane.",
        ),
        (
            "seq-set-process-inlet",
            "(seq-set-process-inlet track instance-id inlet value)",
            "Set a scalar inlet on an attached process slot.",
        ),
        (
            "seq-set-process-slot-enabled",
            "(seq-set-process-slot-enabled track instance-id enabled)",
            "Enable or bypass one attached process slot on a track.",
        ),
        (
            "seq-move-process-slot-before",
            "(seq-move-process-slot-before track instance-id before-instance-id-or-nil)",
            "Move an attached process slot before another slot, or to the end with nil.",
        ),
        (
            "seq-remove-process-slot",
            "(seq-remove-process-slot track instance-id)",
            "Detach one process slot from one track in the current pattern.",
        ),
        (
            "seq-bind-process-port",
            "(seq-bind-process-port track instance-id port target-map)",
            "Bind an attached process slot port to a live step, instrument, effect, or MIDI-FX parameter target.",
        ),
        (
            "seq-clear-process-port-binding",
            "(seq-clear-process-port-binding track instance-id port)",
            "Clear a manual process slot port binding so the authored target hint is used again.",
        ),
        (
            "seq-set-process-trace",
            "(seq-set-process-trace enabled)",
            "Toggle process-port backend trace logging; with no argument, return the current state.",
        ),
        (
            "seq-piano-roll-action",
            "(seq-piano-roll-action action-map)",
            "Apply a piano-roll edit action to the current track.",
        ),
        (
            "seq-set-track",
            "(seq-set-track track)",
            "Select the current track by 0-based index.",
        ),
        (
            "seq-set-delete-target",
            "(seq-set-delete-target kind payload)",
            "Set the active destructive keyboard target, such as :mixer-track, :mod-route, or :fx-effect.",
        ),
        (
            "seq-clear-delete-target",
            "(seq-clear-delete-target)",
            "Clear the active destructive keyboard target.",
        ),
        (
            "seq-delete-target?",
            "(seq-delete-target? kind payload)",
            "Return true when the active destructive keyboard target matches kind and payload.",
        ),
        (
            "seq-active-delete-target-kind",
            "(seq-active-delete-target-kind)",
            "Return the active destructive keyboard target kind, or false when none is active.",
        ),
        (
            "seq-delete-active-target",
            "(seq-delete-active-target)",
            "Delete the active destructive keyboard target when valid for the current buffer.",
        ),
        (
            "seq-clone-active-track-pattern",
            "(seq-clone-active-track-pattern)",
            "Clone the selected mixer track-pattern cell into the current scene.",
        ),
        (
            "seq-set-track-volume",
            "(seq-set-track-volume track volume)",
            "Set a track's mixer volume and update its audio panner.",
        ),
        (
            "seq-toggle-track-mute",
            "(seq-toggle-track-mute track)",
            "Toggle a track's mute state.",
        ),
        (
            "seq-toggle-track-solo",
            "(seq-toggle-track-solo track)",
            "Toggle a track's solo state and update solo mute routing.",
        ),
        (
            "seq-set-bus-volume",
            "(seq-set-bus-volume bus volume)",
            "Set a bus mixer volume and update its bus gain nodes.",
        ),
        (
            "seq-toggle-bus-mute",
            "(seq-toggle-bus-mute bus)",
            "Toggle a bus mute state.",
        ),
        (
            "seq-toggle-bus-solo",
            "(seq-toggle-bus-solo bus)",
            "Toggle a bus solo state.",
        ),
        (
            "seq-set-effect-param",
            "(seq-set-effect-param slot param value)",
            "Set an effect parameter default on the current track.",
        ),
        (
            "seq-set-effect-param-pair",
            "(seq-set-effect-param-pair slot param-a value-a param-b value-b)",
            "Set two effect parameter defaults on the current track with one UI/FX invalidation.",
        ),
        (
            "seq-set-effect-param-pair-live",
            "(seq-set-effect-param-pair-live slot param-a value-a param-b value-b)",
            "Push two effect parameter defaults during a drag without rebuilding reactive FX state.",
        ),
        (
            "seq-select-step",
            "(seq-select-step step)",
            "Toggle a step in the current selection.",
        ),
        (
            "seq-select-step-range",
            "(seq-select-step-range start end)",
            "Replace the selection with an inclusive step range.",
        ),
        (
            "seq-clear-selection",
            "(seq-clear-selection)",
            "Clear the selected steps.",
        ),
        (
            "seq-has-selection?",
            "(seq-has-selection?)",
            "Return true when one or more steps are selected.",
        ),
        (
            "seq-select-all-steps",
            "(seq-select-all-steps)",
            "Select all steps in the current track pattern.",
        ),
        (
            "seq-delete-selected-steps",
            "(seq-delete-selected-steps)",
            "Clear all selected step payloads and clear the selection.",
        ),
        (
            "seq-move-step-drag",
            "(seq-move-step-drag start target)",
            "Move a step payload or the selected step payloads by drag delta.",
        ),
        (
            "seq-shift-selected-steps",
            "(seq-shift-selected-steps direction)",
            "Shift selected step payloads left or right by one step.",
        ),
        (
            "seq-set-effect-plock",
            "(seq-set-effect-plock slot param value [target-node-id])",
            "Set an effect parameter p-lock on each selected step.",
        ),
        (
            "seq-set-effect-plock-pair",
            "(seq-set-effect-plock-pair slot param-a value-a param-b value-b)",
            "Set two effect parameter p-locks on each selected step with one UI/FX invalidation.",
        ),
        (
            "seq-set-step-param-plock",
            "(seq-set-step-param-plock :param value)",
            "Set a step parameter p-lock on each selected step.",
        ),
        (
            "seq-toggle-play",
            "(seq-toggle-play)",
            "Toggle sequencer playback.",
        ),
        (
            "seq-set-bpm",
            "(seq-set-bpm bpm)",
            "Set the project tempo in beats per minute.",
        ),
        (
            "seq-set-track-param",
            "(seq-set-track-param track :param value)",
            "Set a track-level parameter such as length, scale, transpose, or pan.",
        ),
        (
            "seq-set-accumulator",
            "(seq-set-accumulator value)",
            "Set the accumulator value for the current track.",
        ),
        (
            "__register-accumulator-preview",
            "(__register-accumulator-preview label)",
            "Internal helper that registers an accumulator preview label for UI selection.",
        ),
        (
            "__register-midi-fx-preview",
            "(__register-midi-fx-preview label)",
            "Internal helper that registers a MIDI FX preview label for UI selection.",
        ),
        (
            "seq-unpublish-sequencer",
            "(seq-unpublish-sequencer name)",
            "Remove a UI-published def-sequencer by name.",
        ),
        (
            "seq-use-accumulator",
            "(seq-use-accumulator [track] name)",
            "Assign a script accumulator to a track.",
        ),
        (
            "seq-use-midi-fx",
            "(seq-use-midi-fx [track] name ...)",
            "Set the MIDI FX chain for a track.",
        ),
        (
            "seq-clear-midi-fx",
            "(seq-clear-midi-fx [track])",
            "Clear a track's MIDI FX chain.",
        ),
        (
            "seq-set-midi-fx-position",
            "(seq-set-midi-fx-position [track] position)",
            "Set where the MIDI FX chain runs relative to the accumulator.",
        ),
        (
            "seq-set-accum-mode",
            "(seq-set-accum-mode label)",
            "Set the current track's accumulator mode by label.",
        ),
        (
            "seq-set-accum-limit",
            "(seq-set-accum-limit limit)",
            "Set the current track's accumulator limit.",
        ),
        (
            "seq-double-track-pattern",
            "(seq-double-track-pattern)",
            "Duplicate the current track pattern to double its length.",
        ),
        (
            "seq-halve-track-pattern",
            "(seq-halve-track-pattern)",
            "Halve the current track pattern length.",
        ),
        (
            "seq-propagate-current-track-to-all-patterns",
            "(seq-propagate-current-track-to-all-patterns)",
            "Request host propagation of the current track to every pattern.",
        ),
        (
            "seq-set-timebase",
            "(seq-set-timebase label)",
            "Set the current track's default timebase by label.",
        ),
        (
            "seq-set-fts",
            "(seq-set-fts label)",
            "Set the current track's force-to-scale mode by label.",
        ),
        (
            "seq-plock-timebase",
            "(seq-plock-timebase label)",
            "Set a timebase p-lock on selected steps.",
        ),
        (
            "seq-set-swing-resolution",
            "(seq-set-swing-resolution label)",
            "Set default swing resolution or p-lock it on selected steps.",
        ),
        (
            "seq-pause-auto-follow",
            "(seq-pause-auto-follow)",
            "Temporarily pause automatic playhead following.",
        ),
        (
            "seq-toggle-record",
            "(seq-toggle-record)",
            "Toggle recording when at least one track is armed.",
        ),
        (
            "seq-toggle-master-recording",
            "(seq-toggle-master-recording)",
            "Toggle final master-output WAV recording.",
        ),
        (
            "seq-toggle-track-collapsed",
            "(seq-toggle-track-collapsed track)",
            "Toggle whether a track is collapsed in track overview UIs.",
        ),
        (
            "seq-toggle-record-arm",
            "(seq-toggle-record-arm track)",
            "Toggle record-arm state for a track.",
        ),
        (
            "seq-search-samples",
            "(seq-search-samples query)",
            "Search samples.db by sample title, hash, or tag.",
        ),
        (
            "seq-sample-browser",
            "(seq-sample-browser query selected-tags)",
            "Return DB-backed sample tag facets and a flat sample list.",
        ),
        (
            "seq-sample-tags-for-path",
            "(seq-sample-tags-for-path path)",
            "Return tags for a DB-backed sample path.",
        ),
        (
            "seq-sample-tree",
            "(seq-sample-tree)",
            "Return the DB-backed sample browser tree.",
        ),
        (
            "seq-filter-sample-tree",
            "(seq-filter-sample-tree query)",
            "Return a filtered DB-backed sample browser tree.",
        ),
        (
            "seq-project-tree",
            "(seq-project-tree query)",
            "Return the project browser tree filtered by query.",
        ),
        (
            "seq-script-tree",
            "(seq-script-tree query)",
            "Return the script browser tree filtered by query.",
        ),
        (
            "seq-preset-tree",
            "(seq-preset-tree presets query)",
            "Return a preset browser tree for a preset list and query.",
        ),
        (
            "seq-saved-instruments",
            "(seq-saved-instruments)",
            "Return saved custom instrument names.",
        ),
        (
            "seq-saved-instrument-tree",
            "(seq-saved-instrument-tree query)",
            "Return the saved instrument browser tree filtered by query.",
        ),
        (
            "seq-audio-effect-tree",
            "(seq-audio-effect-tree query)",
            "Return the audio effect browser tree filtered by query.",
        ),
        (
            "seq-midi-effect-tree",
            "(seq-midi-effect-tree query)",
            "Return the MIDI effect browser tree filtered by query.",
        ),
    ]);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn track_selection_natives_replace_selection_atomically() {
        let state = Arc::new(SequencerState::new(7, vec![]));
        let current_track = Arc::new(AtomicUsize::new(0));
        let selected_tracks = Arc::new(Mutex::new(HashSet::from([0, 6])));
        let delete_target = Arc::new(Mutex::new(None));
        let delete_target_version = Arc::new(AtomicUsize::new(0));
        let mut runtime = Runtime::new();
        register_track_selection_natives(
            &mut runtime,
            Arc::clone(&state),
            Arc::clone(&current_track),
            Arc::clone(&selected_tracks),
            Arc::clone(&delete_target),
            Arc::clone(&delete_target_version),
        );

        runtime
            .eval_str("(seq-select-track-range 2 6)")
            .expect("select forward track range");
        assert_eq!(
            *selected_tracks.lock().unwrap(),
            HashSet::from([2, 3, 4, 5, 6])
        );
        assert_eq!(current_track.load(Ordering::Relaxed), 6);
        assert_eq!(
            *delete_target.lock().unwrap(),
            Some(ActiveDeleteTarget::MixerTracks {
                tracks: vec![2, 3, 4, 5, 6],
            })
        );

        runtime
            .eval_str("(seq-select-track-range 6 2)")
            .expect("select reverse track range");
        assert_eq!(
            *selected_tracks.lock().unwrap(),
            HashSet::from([2, 3, 4, 5, 6])
        );
        assert_eq!(current_track.load(Ordering::Relaxed), 2);

        let _ = runtime.eval_str("(seq-select-track-range 2 7)");
        assert_eq!(
            *selected_tracks.lock().unwrap(),
            HashSet::from([2, 3, 4, 5, 6]),
            "an invalid range must not partially mutate selection"
        );

        runtime
            .eval_str("(seq-select-tracks (list 1 6 3) 6)")
            .expect("select explicit visual range");
        assert_eq!(*selected_tracks.lock().unwrap(), HashSet::from([1, 3, 6]));
        assert_eq!(current_track.load(Ordering::Relaxed), 6);
        assert_eq!(
            *delete_target.lock().unwrap(),
            Some(ActiveDeleteTarget::MixerTracks {
                tracks: vec![1, 3, 6],
            })
        );

        for invalid in [
            "(seq-select-tracks (list 1 1 3) 1)",
            "(seq-select-tracks (list 1 7 3) 1)",
            "(seq-select-tracks (list 1 3) 6)",
        ] {
            let _ = runtime.eval_str(invalid);
            assert_eq!(
                *selected_tracks.lock().unwrap(),
                HashSet::from([1, 3, 6]),
                "an invalid explicit selection must not partially mutate selection"
            );
        }
    }

    /// Drum rack v2 slice 3 (`docs/drum-rack-v2-spec.md`, "Arming & live
    /// play"): the rack header is one more armable target, exclusive with its
    /// own members.
    fn arm_test_rack() -> sequencer::project::ProjectTrackGroup {
        sequencer::project::ProjectTrackGroup {
            id: 7,
            name: "Kit".to_string(),
            color: [0.5, 0.5, 0.5],
            collapsed: false,
            members: vec![1, 2],
            bus_id: 2,
            rack: Some(sequencer::project::ProjectRackConfig::default()),
            rack_members: Vec::new(),
        }
    }

    #[test]
    fn arming_a_rack_disarms_its_members_and_only_one_rack_at_a_time() {
        let groups = [arm_test_rack()];
        let mut record_armed = vec![true, true, false, true];
        let mut armed_rack = None;

        assert!(toggle_rack_pad_arm(
            &mut armed_rack,
            &mut record_armed,
            &groups[0].members,
            7
        ));
        assert_eq!(armed_rack, Some(7));
        assert_eq!(
            record_armed,
            vec![true, false, false, true],
            "arming the rack disarms its members, and only its members"
        );

        // A second rack takes the keyboard from the first.
        assert!(toggle_rack_pad_arm(&mut armed_rack, &mut record_armed, &[3], 9));
        assert_eq!(armed_rack, Some(9));
        assert_eq!(record_armed, vec![true, false, false, false]);

        // Toggling the armed rack again disarms it outright.
        assert!(!toggle_rack_pad_arm(
            &mut armed_rack,
            &mut record_armed,
            &[3],
            9
        ));
        assert_eq!(armed_rack, None);
    }

    #[test]
    fn arming_a_member_track_disarms_the_rack_but_a_loose_track_does_not() {
        let groups = [arm_test_rack()];
        let mut record_armed = vec![false; 4];
        let mut armed_rack = Some(7);

        assert_eq!(
            toggle_track_record_arm(&mut record_armed, &mut armed_rack, &groups, 3),
            Some(true)
        );
        assert_eq!(
            armed_rack,
            Some(7),
            "arming a track outside the rack leaves pad play alone"
        );

        assert_eq!(
            toggle_track_record_arm(&mut record_armed, &mut armed_rack, &groups, 2),
            Some(true)
        );
        assert_eq!(
            armed_rack, None,
            "arming a member hands the keyboard back to chromatic play"
        );

        // Disarming never re-arms the rack.
        armed_rack = Some(7);
        assert_eq!(
            toggle_track_record_arm(&mut record_armed, &mut armed_rack, &groups, 2),
            Some(false)
        );
        assert_eq!(armed_rack, Some(7));
        assert_eq!(
            toggle_track_record_arm(&mut record_armed, &mut armed_rack, &groups, 9),
            None,
            "an out-of-range track toggles nothing"
        );
    }

    /// A plain (non-rack) group with the same id shape as `arm_test_rack`,
    /// for the ungroup/convert-away cases and for id recycling.
    fn plain_test_group(id: u64) -> sequencer::project::ProjectTrackGroup {
        sequencer::project::ProjectTrackGroup {
            rack: None,
            id,
            ..arm_test_rack()
        }
    }

    fn shared_group_state(
        armed_rack: Option<u64>,
        target: Option<ActiveDeleteTarget>,
    ) -> (
        Arc<Mutex<Option<u64>>>,
        Arc<Mutex<Option<ActiveDeleteTarget>>>,
        Arc<AtomicUsize>,
    ) {
        (
            Arc::new(Mutex::new(armed_rack)),
            Arc::new(Mutex::new(target)),
            Arc::new(AtomicUsize::new(0)),
        )
    }

    /// Group ids are recycled (`max(id) + 1`), so an arm left behind by a
    /// deleted rack does not merely dangle — the next group created inherits
    /// the id and would come up pad-armed, swallowing the bare-key sequencer
    /// shortcuts.
    #[test]
    fn deleting_an_armed_rack_clears_the_arm_so_a_recycled_id_is_not_armed() {
        let (armed_rack, delete_target, delete_target_version) =
            shared_group_state(Some(7), None);

        // The rack is deleted: no group carries id 7 any more.
        prune_stale_group_references(
            &armed_rack,
            &delete_target,
            &delete_target_version,
            &[],
        );
        assert_eq!(*armed_rack.lock().unwrap(), None);

        // A brand-new group recycles id 7 and must not inherit the arm.
        prune_stale_group_references(
            &armed_rack,
            &delete_target,
            &delete_target_version,
            &[plain_test_group(7)],
        );
        assert_eq!(
            *armed_rack.lock().unwrap(),
            None,
            "a recycled group id must not revive a dead rack's pad arm"
        );
    }

    /// Ungroup and convert-to-plain leave the id alive but no longer a rack;
    /// pad routing has nothing to answer with, so the arm must go too.
    #[test]
    fn an_armed_group_that_stops_being_a_rack_is_disarmed() {
        let (armed_rack, delete_target, delete_target_version) =
            shared_group_state(Some(7), None);

        prune_stale_group_references(
            &armed_rack,
            &delete_target,
            &delete_target_version,
            &[arm_test_rack()],
        );
        assert_eq!(
            *armed_rack.lock().unwrap(),
            Some(7),
            "a live rack keeps its arm across unrelated topology edits"
        );

        prune_stale_group_references(
            &armed_rack,
            &delete_target,
            &delete_target_version,
            &[plain_test_group(7)],
        );
        assert_eq!(*armed_rack.lock().unwrap(), None);
    }

    /// Same recycling hazard on the delete target: a stale group id turns one
    /// stray Delete into the destruction of a brand-new, unrelated group.
    #[test]
    fn deleting_the_targeted_group_clears_the_delete_target() {
        let (armed_rack, delete_target, delete_target_version) =
            shared_group_state(None, Some(ActiveDeleteTarget::MixerGroup { group_id: 7 }));

        prune_stale_group_references(
            &armed_rack,
            &delete_target,
            &delete_target_version,
            &[arm_test_rack()],
        );
        assert_eq!(
            *delete_target.lock().unwrap(),
            Some(ActiveDeleteTarget::MixerGroup { group_id: 7 }),
            "a live group keeps its delete target"
        );
        assert_eq!(delete_target_version.load(Ordering::Relaxed), 0);

        prune_stale_group_references(
            &armed_rack,
            &delete_target,
            &delete_target_version,
            &[],
        );
        assert_eq!(*delete_target.lock().unwrap(), None);
        assert_eq!(
            delete_target_version.load(Ordering::Relaxed),
            1,
            "clearing the target must republish the delete-target read surfaces"
        );

        // Recycling id 7 onto a new group must not re-aim Delete at it.
        prune_stale_group_references(
            &armed_rack,
            &delete_target,
            &delete_target_version,
            &[plain_test_group(7)],
        );
        assert_eq!(*delete_target.lock().unwrap(), None);
    }

    /// Track-addressed delete targets are reindexed on their own paths; a
    /// group edit must not clear them out from under the mixer.
    #[test]
    fn group_pruning_leaves_track_addressed_delete_targets_alone() {
        let (armed_rack, delete_target, delete_target_version) =
            shared_group_state(None, Some(ActiveDeleteTarget::MixerTracks { tracks: vec![1, 2] }));

        prune_stale_group_references(
            &armed_rack,
            &delete_target,
            &delete_target_version,
            &[],
        );
        assert_eq!(
            *delete_target.lock().unwrap(),
            Some(ActiveDeleteTarget::MixerTracks { tracks: vec![1, 2] })
        );
        assert_eq!(delete_target_version.load(Ordering::Relaxed), 0);
    }

    fn def_song_value_number(map: &HashMap<String, Rc<RefCell<Value>>>, key: &str) -> f64 {
        match &*map.get(key).expect(key).borrow() {
            Value::Number(value) => *value,
            other => panic!("{key} should be a number, got {other:?}"),
        }
    }

    /// End-to-end: the compiler auto-quotes every def-song argument, so the
    /// `(at ...)` rows arrive at the native as list data and a full spec
    /// parses, including fractional beats preserved exactly.
    #[test]
    fn def_song_form_is_captured_as_data_and_parses() {
        let mut runtime = Runtime::new();
        let captured: Rc<RefCell<Option<DefSongSpec>>> = Rc::new(RefCell::new(None));
        let captured_in = Rc::clone(&captured);
        runtime.register_native("def-song", move |args, _ctx| {
            let spec = parse_def_song_args(&args)?;
            *captured_in.borrow_mut() = Some(spec.clone());
            Ok(Value::String(spec.name))
        });
        runtime
            .eval_str(
                r#"(def-song "bossa-1"
                     (at 0 :scene 0)
                     (at 32 :scene 1 :patterns ((1 3)))
                     (at 47.5 :scene 2 :patterns ((1 5) (3 2)))
                     :end 64
                     :loop true)"#,
            )
            .expect("def-song evaluates");
        let spec = captured.borrow().clone().expect("def-song native ran");
        assert_eq!(spec.name, "bossa-1");
        assert_eq!(spec.end_beat, 64.0);
        assert!(spec.loop_enabled);
        assert_eq!(spec.rows.len(), 3);
        assert_eq!(spec.rows[0], DefSongRow { start_beat: 0.0, scene: 0, patterns: vec![] });
        assert_eq!(
            spec.rows[1],
            DefSongRow { start_beat: 32.0, scene: 1, patterns: vec![(1, Some(3))] }
        );
        assert_eq!(
            spec.rows[2],
            DefSongRow {
                start_beat: 47.5,
                scene: 2,
                patterns: vec![(1, Some(5)), (3, Some(2))],
            }
        );
    }

    #[test]
    fn def_song_invalid_definitions_are_rejected_wholesale() {
        let mut runtime = Runtime::new();
        let calls = Rc::new(RefCell::new(0usize));
        let calls_in = Rc::clone(&calls);
        let errors = Rc::new(RefCell::new(Vec::<String>::new()));
        let errors_in = Rc::clone(&errors);
        runtime.register_native("def-song", move |args, _ctx| {
            match parse_def_song_args(&args) {
                Ok(_) => {
                    *calls_in.borrow_mut() += 1;
                    Ok(Value::Bool(true))
                }
                Err(error) => {
                    errors_in.borrow_mut().push(error.clone());
                    Err(error)
                }
            }
        });
        // Missing :end.
        let _ = runtime.eval_str(r#"(def-song "x" (at 0 :scene 0))"#);
        // Row missing :scene.
        let _ = runtime.eval_str(r#"(def-song "x" (at 0) :end 8)"#);
        // Unknown row key.
        let _ = runtime.eval_str(r#"(def-song "x" (at 0 :scene 0 :bogus 1) :end 8)"#);
        // No rows.
        let _ = runtime.eval_str(r#"(def-song "x" :end 8)"#);
        // Malformed pattern pair.
        let _ = runtime.eval_str(r#"(def-song "x" (at 0 :scene 0 :patterns ((1))) :end 8)"#);
        assert_eq!(*calls.borrow(), 0, "no invalid definition may parse");
        assert_eq!(errors.borrow().len(), 5, "each failure reports why");
        assert!(errors.borrow()[0].contains(":end"), "{:?}", errors.borrow()[0]);
        assert!(errors.borrow()[1].contains(":scene"), "{:?}", errors.borrow()[1]);
        assert!(errors.borrow()[2].contains(":bogus"), "{:?}", errors.borrow()[2]);
        assert!(errors.borrow()[3].contains("at least one"), "{:?}", errors.borrow()[3]);
        assert!(errors.borrow()[4].contains("pair"), "{:?}", errors.borrow()[4]);
    }

    #[test]
    fn def_song_lowers_to_song_replace_payload() {
        let spec = DefSongSpec {
            name: "bossa-1".to_string(),
            rows: vec![
                DefSongRow { start_beat: 0.0, scene: 0, patterns: vec![] },
                DefSongRow { start_beat: 47.5, scene: 2, patterns: vec![(1, Some(5))] },
            ],
            end_beat: 64.0,
            loop_enabled: true,
        };
        let Value::Map(payload) = def_song_replace_payload(&spec) else {
            panic!("payload must be a map");
        };
        assert_eq!(def_song_value_number(&payload, "end-beat"), 64.0);
        assert_eq!(
            payload.get("loop").map(|cell| cell.borrow().clone()),
            Some(Value::Bool(true))
        );
        assert_eq!(
            payload.get("name").map(|cell| cell.borrow().clone()),
            Some(Value::String("bossa-1".to_string()))
        );
        let rows = payload.get("rows").unwrap().borrow().clone();
        let Value::List(rows) = rows else {
            panic!("rows must be a list");
        };
        assert_eq!(rows.len(), 2);
        let Value::Map(second) = rows[1].borrow().clone() else {
            panic!("row payload must be a map");
        };
        assert_eq!(
            def_song_value_number(&second, "start-beat"),
            47.5,
            "fractional beats must be preserved exactly"
        );
        assert_eq!(def_song_value_number(&second, "scene"), 2.0);
        let overrides = second.get("overrides").unwrap().borrow().clone();
        let Value::List(overrides) = overrides else {
            panic!("overrides must be a list");
        };
        assert_eq!(overrides.len(), 1);
    }

    #[test]
    fn custom_effect_plock_native_dispatches_frozen_history_target() {
        let command = effect_plock_history_command(
            2,
            &[3, 7],
            11,
            &[(4, 0.25), (5, 0.75)],
        );
        let HostCommand::Custom { name, payload } = command else {
            panic!("effect p-lock history must use a custom host command");
        };
        assert_eq!(name, "set-effect-plock-batch");
        let Value::Map(payload) = payload else {
            panic!("effect p-lock history payload must be a map");
        };
        assert_eq!(
            payload.get("track").map(|value| value.borrow().clone()),
            Some(Value::Number(2.0)),
        );
        assert_eq!(
            payload.get("slot-idx").map(|value| value.borrow().clone()),
            Some(Value::Number(11.0)),
        );
        let steps = payload.get("steps").unwrap().borrow().clone();
        assert!(matches!(steps, Value::List(values) if values.len() == 2));
        let updates = payload.get("updates").unwrap().borrow().clone();
        assert!(matches!(updates, Value::List(values) if values.len() == 2));
    }

    fn map_bool(value: &Value, key: &str) -> bool {
        let Value::Map(map) = value else {
            panic!("expected map value");
        };
        match map.get(key).map(|cell| cell.borrow().clone()) {
            Some(Value::Bool(value)) => value,
            other => panic!("expected bool field {key}, got {other:?}"),
        }
    }

    fn map_symbol(value: &Value, key: &str) -> String {
        let Value::Map(map) = value else {
            panic!("expected map value");
        };
        match map.get(key).map(|cell| cell.borrow().clone()) {
            Some(Value::Symbol(value)) => value,
            other => panic!("expected symbol field {key}, got {other:?}"),
        }
    }

    fn map_string(value: &Value, key: &str) -> String {
        let Value::Map(map) = value else {
            panic!("expected map value");
        };
        match map.get(key).map(|cell| cell.borrow().clone()) {
            Some(Value::String(value)) => value,
            other => panic!("expected string field {key}, got {other:?}"),
        }
    }

    struct TempDirGuard {
        path: std::path::PathBuf,
    }

    impl TempDirGuard {
        fn create(name: &str) -> Self {
            let unique = format!(
                "{}-{}-{}",
                name,
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .expect("system clock after unix epoch")
                    .as_nanos()
            );
            let path = std::env::temp_dir().join(unique);
            std::fs::create_dir_all(&path).expect("create temp directory");
            Self { path }
        }
    }

    #[test]
    fn ui_def_accumulator_dispatch_keeps_process_form_after_preview_layer() {
        let state = Arc::new(SequencerState::new(
            1,
            vec![sequencer::sequencer::default_empty_effect_chain()],
        ));
        let mut runtime = Runtime::new();
        let process_authoring = sequencer::lisp_host::register_published_process_authoring_natives(
            &mut runtime,
            Arc::clone(&state),
            Arc::new(AtomicUsize::new(0)),
        );
        let accumulator_names = Arc::new(Mutex::new(Vec::new()));
        register_ui_def_accumulator_dispatch(
            &mut runtime,
            Arc::clone(&accumulator_names),
            process_authoring,
            false,
        );

        runtime
            .eval_str(
                r#"
                (def-accumulator sparse-transpose
                  :target (step-param :transpose)
                  :amount (amount :lane true :default 0)
                  :range (-24 24)
                  :mode :clip)
                "#,
            )
            .expect("main UI runtime should accept process accumulator form");
        runtime
            .eval_str(
                r#"(def-accumulator "legacy-preview"
                    (acc-add-step-param :transpose acc-value))"#,
            )
            .expect("main UI runtime should keep legacy accumulator preview form");

        let published = state.published_process_authoring().to_runtime();
        assert!(
            published
                .defs
                .iter()
                .any(|def| def.name == "sparse-transpose"),
            "process accumulator definition should publish from the UI runtime"
        );
        assert_eq!(
            accumulator_names.lock().unwrap().as_slice(),
            &["legacy-preview".to_string()]
        );
    }

    #[test]
    fn expanded_step_viewport_parser_preserves_dynamic_process_modes() {
        let viewport =
            expanded_step_viewport_from_numbers(0.0, 12.0, 0.0, 7.0, 3.0).expect("viewport");
        assert_eq!(viewport.track, 0);
        assert_eq!(viewport.track_id, 12);
        assert_eq!(viewport.page, 0);
        assert_eq!(viewport.mode, 7);
        assert_eq!(viewport.cursor_step, 3);

        let err = expanded_step_viewport_from_numbers(0.0, 12.0, 0.0, 7.5, 3.0)
            .expect_err("fractional mode should be rejected");
        assert!(err.contains("mode"), "unexpected error: {err}");
    }

    impl Drop for TempDirGuard {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }

    fn wav_files(path: &std::path::Path) -> Vec<std::path::PathBuf> {
        let Ok(entries) = std::fs::read_dir(path) else {
            return Vec::new();
        };
        let mut files = entries
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| path.extension().and_then(|ext| ext.to_str()) == Some("wav"))
            .collect::<Vec<_>>();
        files.sort();
        files
    }

    #[test]
    fn master_recording_toggle_starts_saves_wav_and_reports_empty_take() {
        let recordings = TempDirGuard::create("metal-master-recording-toggle");
        let recorder = sequencer::recorder::MasterRecorder::new(44_100, 2);
        let master_recording = AtomicBool::new(false);

        let (active, status) =
            toggle_master_recording_capture_in(&master_recording, &recorder, &recordings.path)
                .expect("start capture");
        assert!(active, "start should return active");
        assert!(
            status.contains("started"),
            "start status should mention recording start: {status}"
        );
        assert!(master_recording.load(Ordering::Acquire));
        assert!(recorder.is_active());

        recorder.capture(&[0.25, -0.25, 0.5, -0.5]);
        let (active, status) =
            toggle_master_recording_capture_in(&master_recording, &recorder, &recordings.path)
                .expect("stop capture");
        assert!(!active, "stop should return inactive");
        assert!(
            status.contains(&recordings.path.to_string_lossy().into_owned()),
            "stop status should include the configured recordings path: {status}"
        );
        assert!(!master_recording.load(Ordering::Acquire));
        assert!(!recorder.is_active());

        let saved = wav_files(&recordings.path);
        assert_eq!(saved.len(), 1, "expected one saved WAV in recordings/");
        assert!(
            std::fs::metadata(&saved[0])
                .expect("saved WAV metadata")
                .len()
                > 44,
            "saved WAV should contain audio samples"
        );

        let (active, _) =
            toggle_master_recording_capture_in(&master_recording, &recorder, &recordings.path)
                .expect("start empty take");
        assert!(active);
        let error =
            toggle_master_recording_capture_in(&master_recording, &recorder, &recordings.path)
                .expect_err("empty take should fail instead of writing");
        assert!(
            error.contains("Recording is empty"),
            "empty-take error should be explicit: {error}"
        );
        assert!(!master_recording.load(Ordering::Acquire));
        assert!(!recorder.is_active());
        assert_eq!(
            wav_files(&recordings.path),
            saved,
            "empty recording should not create another WAV"
        );
    }

    #[test]
    fn applied_general_effect_artifact_can_finalize_but_not_apply_again() {
        let store = sequencer::agent::store::ConversationStore::new(44_100);
        let id = store.new_conversation(sequencer::agent::store::AgentKind::General);
        let mut state = store.snapshot(id).unwrap().state;
        state.effect_draft = Some(sequencer::agent::store::EffectDraft {
            name: "simple-tape-delay".to_string(),
            dsp_source: "(def in_l (in 1 @name left))".to_string(),
            ui_source: "(defeffect-ui (label \"ok\"))".to_string(),
        });
        state.accepted_effect_target = Some(sequencer::agent::store::AcceptedEffectTarget {
            track_index: 0,
            slot_index: 0,
            effect_name: "agent-effect-draft-1/".to_string(),
        });
        state.effect_draft_applied = true;

        let artifact = agent_artifact_value(state);

        assert_eq!(map_symbol(&artifact, "status"), "applied");
        assert!(!map_bool(&artifact, "can-apply"));
        assert!(map_bool(&artifact, "can-finalize"));
        assert!(map_bool(&artifact, "has-draft"));
    }

    #[test]
    fn updated_applied_effect_artifact_can_apply_again() {
        let store = sequencer::agent::store::ConversationStore::new(44_100);
        let id = store.new_conversation(sequencer::agent::store::AgentKind::Effect);
        let mut state = store.snapshot(id).unwrap().state;
        state.effect_draft = Some(sequencer::agent::store::EffectDraft {
            name: "simple-tape-delay".to_string(),
            dsp_source: "(def in_l (in 1 @name left))".to_string(),
            ui_source: "(defeffect-ui (label \"ok\"))".to_string(),
        });
        state.accepted_effect_target = Some(sequencer::agent::store::AcceptedEffectTarget {
            track_index: 0,
            slot_index: 0,
            effect_name: "agent-effect-draft-1/".to_string(),
        });
        state.effect_draft_applied = false;

        let artifact = agent_artifact_value(state);

        assert_eq!(map_symbol(&artifact, "status"), "updated");
        assert!(map_bool(&artifact, "can-apply"));
        assert_eq!(map_string(&artifact, "apply-label"), "Update artifact");
        assert!(map_bool(&artifact, "can-finalize"));
    }

    #[test]
    fn updated_applied_instrument_artifact_can_apply_again() {
        let store = sequencer::agent::store::ConversationStore::new(44_100);
        let id = store.new_conversation(sequencer::agent::store::AgentKind::Instrument);
        let mut state = store.snapshot(id).unwrap().state;
        state.draft = Some(sequencer::agent::store::InstrumentDraft {
            dsp_source: "(out 0 1 @name audio)".to_string(),
            ui_source: "(defsynth-ui (label \"updated\"))".to_string(),
        });
        state.accepted_instrument_target =
            Some(sequencer::agent::store::AcceptedInstrumentTarget {
                track_index: 0,
                instrument_name: "agent-draft-1/".to_string(),
            });

        let artifact = agent_artifact_value(state);

        assert_eq!(map_symbol(&artifact, "status"), "updated");
        assert!(map_bool(&artifact, "can-apply"));
        assert_eq!(map_string(&artifact, "apply-label"), "Update artifact");
        assert!(map_bool(&artifact, "can-finalize"));
    }

    #[test]
    fn agent_display_text_omits_fenced_source_blocks() {
        let display = agent_message_display_text(
            "Built it.\n```dgenlisp\n(def x 1)\n(out x 1 @name audio)\n```\n```eseqlisp\n(defsynth-ui)\n```",
        );

        assert_eq!(display.code_block_count, 2);
        assert_eq!(display.text, "Built it.");
    }

    #[test]
    fn agent_display_text_uses_placeholder_for_source_only_messages() {
        let display = agent_message_display_text("```dgenlisp\n(def x 1)\n```");

        assert_eq!(display.code_block_count, 1);
        assert_eq!(display.text, "Generated instrument source.");
    }

    #[test]
    fn agent_display_text_preserves_plain_messages() {
        let display = agent_message_display_text("No code here.\nStill normal.");

        assert_eq!(display.code_block_count, 0);
        assert_eq!(display.text, "No code here.\nStill normal.");
    }
}
