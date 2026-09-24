use super::*;

/// Modes 0..8 are the built-in step params (see `metal_step_param_for_mode`);
/// process lanes start after them. MUST match `seqv-process-lane-mode-offset`
/// in content/ui/seqv-track-params.lisp.
pub(crate) const PROCESS_LANE_MODE_OFFSET: usize = 9;

#[derive(Clone, Debug)]
pub(super) struct ProcessLaneUiEntry {
    instance_id: sequencer::process::ProcessInstanceId,
    slot_index: usize,
    class_name: String,
    inlet_name: String,
    label: String,
    short_label: String,
    kind: String,
    min: f32,
    max: f32,
    default: f32,
    decimals: u8,
    target: String,
    map_ports: Vec<Value>,
    values: Vec<f32>,
    project: bool,
    forked: bool,
    instance_name: Option<String>,
    default_lane: bool,
    /// A slot the user added to this track through the patch bay's + box
    /// (eseq-53y7): named per track, so the lane dropdown lists it under
    /// that instance name instead of the numbered class/inlet form.
    roster: bool,
}

pub(super) fn process_literal_as_f32(value: &sequencer::process::ProcessLiteral) -> Option<f32> {
    match value {
        sequencer::process::ProcessLiteral::Number(value) => Some(*value as f32),
        sequencer::process::ProcessLiteral::Bool(value) => Some(if *value { 1.0 } else { 0.0 }),
        _ => None,
    }
}

pub(super) fn process_inlet_kind_name(kind: &sequencer::process::ProcessInletKind) -> &'static str {
    match kind {
        sequencer::process::ProcessInletKind::Float => "float",
        sequencer::process::ProcessInletKind::Int => "int",
        sequencer::process::ProcessInletKind::Gate => "gate",
        sequencer::process::ProcessInletKind::Track => "track",
        sequencer::process::ProcessInletKind::Field => "field",
        sequencer::process::ProcessInletKind::Any => "any",
        sequencer::process::ProcessInletKind::Enum(_) => "enum",
    }
}

pub(super) fn process_target_hint_label(target: Option<&sequencer::process::ProcessTargetHint>) -> String {
    match target {
        Some(sequencer::process::ProcessTargetHint::StepParam { param }) => {
            format!("step-param:{param}")
        }
        Some(sequencer::process::ProcessTargetHint::ParamTag { tag }) => {
            format!("param-tag:{tag}")
        }
        Some(sequencer::process::ProcessTargetHint::InstrumentParam { param }) => {
            format!("instrument-param:{param}")
        }
        Some(sequencer::process::ProcessTargetHint::EffectParam { effect, param }) => {
            format!("effect-param:{effect}:{param}")
        }
        Some(sequencer::process::ProcessTargetHint::MidiFxParam { fx, param }) => {
            format!("midi-fx-param:{fx}:{param}")
        }
        Some(sequencer::process::ProcessTargetHint::RackMacroParam { macro_id }) => {
            format!("rack-macro:macro_{}", macro_id + 1)
        }
        None => String::new(),
    }
}

pub(super) fn process_param_target_label(target: &sequencer::process::ParamTarget) -> String {
    match target {
        sequencer::process::ParamTarget::StepParam { param } => {
            format!("step-param:{param}")
        }
        sequencer::process::ParamTarget::InstrumentParam { param, .. } => {
            format!("instrument:{param}")
        }
        sequencer::process::ParamTarget::EffectParam {
            slot,
            effect,
            param,
            ..
        } => format!("fx{}:{effect}:{param}", slot + 1),
        sequencer::process::ParamTarget::MidiFxParam { slot, fx, param } => {
            format!("midi-fx{}:{fx}:{param}", slot + 1)
        }
        sequencer::process::ParamTarget::ProcessInlet {
            process,
            inlet,
            instance_id,
        } => instance_id
            .map(|id| format!("process:{process}#{}:{inlet}", id.0))
            .unwrap_or_else(|| format!("process:{process}:{inlet}")),
        sequencer::process::ParamTarget::RackSlotParam { slot, param } => {
            format!("rack{}:{param}", slot + 1)
        }
        sequencer::process::ParamTarget::RackSlotInstrumentParam { slot, param, .. } => {
            format!("rack{}:instrument:{param}", slot + 1)
        }
        sequencer::process::ParamTarget::RackMacroParam { macro_id } => {
            format!("rack-macro:macro_{}", macro_id + 1)
        }
        sequencer::process::ParamTarget::BusSend { bus } => {
            format!("send:{}", sequencer::process::bus_send_label(*bus))
        }
    }
}

pub(super) fn macro_mapping_current_value(
    app: &app::App,
    mapping: &sequencer::macro_engine::MacroMapping,
) -> Option<f32> {
    match (mapping.scope, &mapping.target) {
        (
            sequencer::macro_engine::ParamScope::Track(track),
            sequencer::process::ParamTarget::EffectParam {
                slot,
                effect,
                param,
                ..
            },
        ) => {
            let param_idx = app
                .graph
                .effect_descriptors
                .get(track)?
                .get(*slot)
                .filter(|descriptor| descriptor.name.eq_ignore_ascii_case(effect))?
                .params
                .iter()
                .position(|descriptor| descriptor.has_tag_or_name(param))?;
            app.effective_slot_param_value(track, *slot, param_idx)
        }
        (
            sequencer::macro_engine::ParamScope::Track(track),
            sequencer::process::ParamTarget::InstrumentParam { param, .. },
        ) => {
            let param_idx = app
                .graph
                .instrument_descriptors
                .get(track)?
                .params
                .iter()
                .position(|descriptor| descriptor.has_tag_or_name(param))?;
            app.effective_instrument_param_value(track, param_idx)
        }
        (
            sequencer::macro_engine::ParamScope::Bus(bus_id),
            sequencer::process::ParamTarget::EffectParam {
                slot,
                effect,
                param,
                ..
            },
        ) => {
            let bus_idx = app.buses.iter().position(|bus| bus.id == bus_id)?;
            let param_idx = app.buses[bus_idx]
                .effect_descriptors
                .get(*slot)
                .filter(|descriptor| descriptor.name.eq_ignore_ascii_case(effect))?
                .params
                .iter()
                .position(|descriptor| descriptor.has_tag_or_name(param))?;
            app.effective_bus_slot_param_value(bus_idx, *slot, param_idx)
        }
        _ => None,
    }
}

pub(super) fn macro_mapping_param_descriptor<'a>(
    app: &'a app::App,
    mapping: &sequencer::macro_engine::MacroMapping,
) -> Option<(
    &'a sequencer::effects::EffectDescriptor,
    &'a sequencer::effects::ParamDescriptor,
)> {
    match (mapping.scope, &mapping.target) {
        (
            sequencer::macro_engine::ParamScope::Track(track),
            sequencer::process::ParamTarget::EffectParam {
                slot,
                effect,
                param,
                ..
            },
        ) => {
            let device = app
                .graph
                .effect_descriptors
                .get(track)?
                .get(*slot)
                .filter(|descriptor| descriptor.name.eq_ignore_ascii_case(effect))?;
            let param = device
                .params
                .iter()
                .find(|descriptor| descriptor.has_tag_or_name(param))?;
            Some((device, param))
        }
        (
            sequencer::macro_engine::ParamScope::Track(track),
            sequencer::process::ParamTarget::InstrumentParam { param, .. },
        ) => {
            let device = app.graph.instrument_descriptors.get(track)?;
            let param = device
                .params
                .iter()
                .find(|descriptor| descriptor.has_tag_or_name(param))?;
            Some((device, param))
        }
        (
            sequencer::macro_engine::ParamScope::Bus(bus_id),
            sequencer::process::ParamTarget::EffectParam {
                slot,
                effect,
                param,
                ..
            },
        ) => {
            let bus = app.buses.iter().find(|bus| bus.id == bus_id)?;
            let device = bus
                .effect_descriptors
                .get(*slot)
                .filter(|descriptor| descriptor.name.eq_ignore_ascii_case(effect))?;
            let param = device
                .params
                .iter()
                .find(|descriptor| descriptor.has_tag_or_name(param))?;
            Some((device, param))
        }
        _ => None,
    }
}

pub(super) fn macro_mapping_display_metadata(
    app: &app::App,
    mapping: &sequencer::macro_engine::MacroMapping,
) -> (String, String, f32, f32, f32, f32, f32, u8, String) {
    let Some((device, param)) = macro_mapping_param_descriptor(app, mapping) else {
        let scope_label = match mapping.scope {
            sequencer::macro_engine::ParamScope::Track(track) => format!("Track {}", track + 1),
            sequencer::macro_engine::ParamScope::Bus(bus_id) => app
                .buses
                .iter()
                .find(|bus| bus.id == bus_id)
                .map(|bus| bus.name.clone())
                .unwrap_or_else(|| format!("Bus {}", bus_id.0)),
        };
        return (
            scope_label,
            process_param_target_label(&mapping.target),
            mapping.range_min,
            mapping.range_max,
            mapping.range_min,
            mapping.range_max,
            1.0,
            2,
            String::new(),
        );
    };
    let scale = if param.is_percent() { 100.0 } else { 1.0 };
    let (decimals, unit) = match &param.kind {
        sequencer::effects::ParamKind::Boolean | sequencer::effects::ParamKind::Enum { .. } => {
            (0, String::new())
        }
        sequencer::effects::ParamKind::Continuous { unit } => {
            let decimals = if unit.as_deref() == Some("%") { 1 } else { 2 };
            (decimals, unit.clone().unwrap_or_default())
        }
    };
    let scope_label = match mapping.scope {
        sequencer::macro_engine::ParamScope::Track(track) => format!("T{}", track + 1),
        sequencer::macro_engine::ParamScope::Bus(bus_id) => app
            .buses
            .iter()
            .find(|bus| bus.id == bus_id)
            .map(|bus| bus.name.clone())
            .unwrap_or_else(|| format!("Bus {}", bus_id.0)),
    };
    (
        format!("{scope_label} · {}", device.name),
        param.name.clone(),
        param.stored_to_user(mapping.range_min),
        param.stored_to_user(mapping.range_max),
        param.stored_to_user(param.min),
        param.stored_to_user(param.max),
        scale,
        decimals,
        unit,
    )
}

pub(crate) fn build_macros_value(app: &app::App) -> Value {
    list_value(app.macro_engine.macros().iter().map(|macro_definition| {
        let kind = match macro_definition.kind {
            sequencer::macro_engine::MacroKind::Mapped => "mapped",
            sequencer::macro_engine::MacroKind::Scene(_) => "scene",
        };
        let mappings = list_value(macro_definition.mappings.iter().enumerate().map(
            |(mapping_idx, mapping)| {
                let (
                    path_label,
                    param_label,
                    display_min,
                    display_max,
                    domain_min,
                    domain_max,
                    display_scale,
                    display_decimals,
                    display_unit,
                ) = macro_mapping_display_metadata(app, mapping);
                let curve = match mapping.curve {
                    sequencer::macro_engine::MacroCurve::Linear => "linear",
                    sequencer::macro_engine::MacroCurve::Exp => "exp",
                    sequencer::macro_engine::MacroCurve::Log => "log",
                    sequencer::macro_engine::MacroCurve::LogDomain => "log-domain",
                };
                map_value([
                    ("mapping-idx", Value::Number(mapping_idx as f64)),
                    (
                        "track",
                        Value::Number(match mapping.scope {
                            sequencer::macro_engine::ParamScope::Track(track) => track as f64,
                            sequencer::macro_engine::ParamScope::Bus(_) => -1.0,
                        }),
                    ),
                    (
                        "scope",
                        Value::String(match mapping.scope {
                            sequencer::macro_engine::ParamScope::Track(_) => "track".to_string(),
                            sequencer::macro_engine::ParamScope::Bus(_) => "bus".to_string(),
                        }),
                    ),
                    ("target", macro_mapping_target_value(&mapping.target)),
                    (
                        "target-label",
                        Value::String(format!(
                            "{} · {}",
                            path_label,
                            process_param_target_label(&mapping.target)
                        )),
                    ),
                    ("min", Value::Number(mapping.range_min as f64)),
                    ("max", Value::Number(mapping.range_max as f64)),
                    ("path-label", Value::String(path_label)),
                    ("param-label", Value::String(param_label)),
                    ("display-min", Value::Number(display_min as f64)),
                    ("display-max", Value::Number(display_max as f64)),
                    ("domain-min", Value::Number(domain_min as f64)),
                    ("domain-max", Value::Number(domain_max as f64)),
                    ("display-scale", Value::Number(display_scale as f64)),
                    ("display-decimals", Value::Number(display_decimals as f64)),
                    ("display-unit", Value::String(display_unit)),
                    ("curve", Value::String(curve.to_string())),
                    (
                        "current",
                        macro_mapping_current_value(app, mapping)
                            .map(|value| Value::Number(value as f64))
                            .unwrap_or(Value::Nil),
                    ),
                    (
                        "display-current",
                        macro_mapping_current_value(app, mapping)
                            .and_then(|value| {
                                macro_mapping_param_descriptor(app, mapping)
                                    .map(|(_, param)| param.stored_to_user(value))
                            })
                            .map(|value| Value::Number(value as f64))
                            .unwrap_or(Value::Nil),
                    ),
                    ("suspended", Value::Bool(mapping.suspended)),
                ])
            },
        ));
        let (target_scene, morph_params, steal_patterns, quantize, track_mask, diff_count) =
            match &macro_definition.kind {
                sequencer::macro_engine::MacroKind::Mapped => (
                    Value::Nil,
                    Value::Nil,
                    Value::Nil,
                    Value::Nil,
                    Value::Nil,
                    Value::Nil,
                ),
                sequencer::macro_engine::MacroKind::Scene(config) => (
                    Value::Number(config.target_scene as f64),
                    Value::Bool(config.morph_params),
                    Value::Bool(config.steal_patterns),
                    Value::String(
                        match config.quantize {
                            sequencer::macro_engine::StealQuantize::Off => "off",
                            sequencer::macro_engine::StealQuantize::Sixteenth => "sixteenth",
                            sequencer::macro_engine::StealQuantize::Bar => "bar",
                        }
                        .to_string(),
                    ),
                    config
                        .track_mask
                        .as_ref()
                        .map(|mask| list_value(mask.iter().copied().map(Value::Bool)))
                        .unwrap_or(Value::Nil),
                    Value::Number(app.scene_macro_diff_count(config) as f64),
                ),
            };
        map_value([
            ("id", Value::Number(macro_definition.id as f64)),
            (
                "key",
                macro_definition
                    .key
                    .as_ref()
                    .map(|key| Value::String(key.clone()))
                    .unwrap_or(Value::Nil),
            ),
            ("name", Value::String(macro_definition.name.clone())),
            ("kind", Value::String(kind.to_string())),
            ("value", Value::Number(macro_definition.value as f64)),
            ("mappings", mappings),
            ("target-scene", target_scene),
            ("morph-params", morph_params),
            ("steal-patterns", steal_patterns),
            ("quantize", quantize),
            ("track-mask", track_mask),
            ("diff-count", diff_count),
        ])
    }))
}

pub(crate) fn sync_macro_state(rt: &mut Runtime, app: &app::App) {
    rt.set_reactive("SEQ", "macros", build_macros_value(app));
}

pub(super) fn macro_mapping_target_value(target: &sequencer::process::ParamTarget) -> Value {
    use sequencer::process::ParamTarget;

    let mut entries = Vec::new();
    match target {
        ParamTarget::StepParam { param } => {
            entries.push(("kind", Value::String("step".to_string())));
            entries.push(("param", Value::String(param.clone())));
        }
        ParamTarget::InstrumentParam { param, .. } => {
            entries.push(("kind", Value::String("instrument".to_string())));
            entries.push(("param", Value::String(param.clone())));
        }
        ParamTarget::EffectParam {
            slot,
            effect,
            param,
            ..
        } => {
            entries.push(("kind", Value::String("effect".to_string())));
            entries.push(("slot-idx", Value::Number(*slot as f64)));
            entries.push(("effect", Value::String(effect.clone())));
            entries.push(("param", Value::String(param.clone())));
        }
        ParamTarget::MidiFxParam { slot, fx, param } => {
            entries.push(("kind", Value::String("midi-fx".to_string())));
            entries.push(("slot-idx", Value::Number(*slot as f64)));
            entries.push(("fx", Value::String(fx.clone())));
            entries.push(("param", Value::String(param.clone())));
        }
        ParamTarget::ProcessInlet { process, inlet, .. } => {
            entries.push(("kind", Value::String("process-inlet".to_string())));
            entries.push(("process", Value::String(process.clone())));
            entries.push(("inlet", Value::String(inlet.clone())));
        }
        ParamTarget::RackSlotParam { slot, param } => {
            entries.push(("kind", Value::String("rack-slot".to_string())));
            entries.push(("slot-idx", Value::Number(*slot as f64)));
            entries.push(("param", Value::String(param.clone())));
        }
        ParamTarget::RackSlotInstrumentParam { slot, param, .. } => {
            entries.push(("kind", Value::String("rack-slot-instrument".to_string())));
            entries.push(("slot-idx", Value::Number(*slot as f64)));
            entries.push(("param", Value::String(param.clone())));
        }
        ParamTarget::RackMacroParam { macro_id } => {
            entries.push(("kind", Value::String("rack-macro".to_string())));
            entries.push(("macro-id", Value::Number(*macro_id as f64)));
        }
        ParamTarget::BusSend { bus } => {
            entries.push(("kind", Value::String("bus-send".to_string())));
            entries.push(("bus-id", Value::Number(*bus as f64)));
        }
    }
    map_value(entries)
}

pub(super) fn process_param_target_is_bindable(target: &sequencer::process::ParamTarget) -> bool {
    !matches!(
        target,
        sequencer::process::ParamTarget::RackSlotParam { .. }
            | sequencer::process::ParamTarget::RackSlotInstrumentParam { .. }
    )
}

pub(super) fn process_target_kind_label(kind: Option<sequencer::process::ProcessTargetKind>) -> String {
    kind.map(|kind| kind.as_str().to_string())
        .unwrap_or_default()
}

pub(super) fn process_ports_label(ports: &[sequencer::process::ProcessPortDef]) -> String {
    match ports {
        [] => String::new(),
        [port] if port.name == sequencer::process::DEFAULT_PROCESS_PORT => {
            process_target_hint_label(port.target.as_ref())
        }
        _ => ports
            .iter()
            .map(|port| {
                let target = process_target_hint_label(port.target.as_ref());
                if target.is_empty() {
                    format!("{}:unbound", port.name)
                } else {
                    format!("{}:{target}", port.name)
                }
            })
            .collect::<Vec<_>>()
            .join(", "),
    }
}

pub(super) fn process_slot_ports_value(
    slot: &sequencer::process::TrackProcessSlot,
    def: Option<&sequencer::process::PublishedProcessDef>,
) -> Value {
    let mut ports = def.map(|def| def.ports.clone()).unwrap_or_default();
    for name in slot.bindings.keys() {
        if !ports.iter().any(|port| &port.name == name) {
            ports.push(sequencer::process::ProcessPortDef {
                name: name.clone(),
                target: None,
                binding_mode: sequencer::process::ProcessPortBindingMode::Fixed,
                target_kind: None,
            });
        }
    }
    list_value(
        ports
            .into_iter()
            .map(|port| process_port_value(slot, &port)),
    )
}

pub(super) fn process_mappable_port_values(
    slot: &sequencer::process::TrackProcessSlot,
    def: Option<&sequencer::process::PublishedProcessDef>,
) -> Vec<Value> {
    def.map(|def| {
        def.ports
            .iter()
            .filter(|port| port.is_mappable())
            .map(|port| process_port_value(slot, port))
            .collect()
    })
    .unwrap_or_default()
}

pub(super) fn process_port_value(
    slot: &sequencer::process::TrackProcessSlot,
    port: &sequencer::process::ProcessPortDef,
) -> Value {
    let binding = slot.bindings.get(&port.name);
    let disconnected = slot.unbound_ports.contains(&port.name);
    let manual = matches!(binding, Some(Some(_)));
    let hint_label = process_target_hint_label(port.target.as_ref());
    let target_label = match binding {
        _ if disconnected => "unbound".to_string(),
        Some(Some(target)) => process_param_target_label(target),
        _ if !hint_label.is_empty() => hint_label.clone(),
        _ => "unbound".to_string(),
    };
    let status = match binding {
        _ if disconnected => "unbound",
        Some(Some(_)) => "bound",
        Some(None) | None if port.target.is_some() => "hint",
        Some(None) | None => "unbound",
    };
    let bindable = port.is_mappable()
        && binding
            .and_then(|binding| binding.as_ref())
            .map(process_param_target_is_bindable)
            .unwrap_or(true);
    map_value([
        ("name", Value::String(port.name.clone())),
        (
            "label",
            Value::String(if port.name == sequencer::process::DEFAULT_PROCESS_PORT {
                "default".to_string()
            } else {
                port.name.clone()
            }),
        ),
        ("hint", Value::String(hint_label)),
        ("target", Value::String(target_label)),
        ("status", Value::String(status.to_string())),
        ("manual", Value::Bool(manual)),
        ("clearable", Value::Bool(manual || disconnected)),
        // A port that writes somewhere (manual or hint) can be disconnected.
        ("disconnectable", Value::Bool(!disconnected && status != "unbound")),
        ("mappable", Value::Bool(port.is_mappable())),
        ("connectable", Value::Bool(port.is_connectable())),
        ("bindable", Value::Bool(bindable)),
        (
            "target-kind",
            Value::String(process_target_kind_label(port.effective_target_kind())),
        ),
        // Structured view of a manual binding for UI that names the target
        // itself (the lane strip): the wired lane's instance id and inlet, or
        // the step param name. Nil when the port follows its hint or is unbound.
        (
            "target-instance-id",
            match binding {
                Some(Some(sequencer::process::ParamTarget::ProcessInlet {
                    instance_id: Some(id),
                    ..
                })) => Value::Number(id.0 as f64),
                _ => Value::Nil,
            },
        ),
        (
            "target-inlet",
            match binding {
                Some(Some(sequencer::process::ParamTarget::ProcessInlet { inlet, .. })) => {
                    Value::String(inlet.clone())
                }
                _ => Value::Nil,
            },
        ),
        (
            "target-step-param",
            match binding {
                Some(Some(sequencer::process::ParamTarget::StepParam { param })) => {
                    Value::String(param.clone())
                }
                _ => Value::Nil,
            },
        ),
        (
            "fanout",
            list_value(
                slot.fanout
                    .get(&port.name)
                    .map(Vec::as_slice)
                    .unwrap_or(&[])
                    .iter()
                    .enumerate()
                    .map(|(index, entry)| {
                        map_value([
                            ("index", Value::Number(index as f64)),
                            ("target", Value::String(process_param_target_label(&entry.target))),
                            (
                                "target-step-param",
                                match &entry.target {
                                    sequencer::process::ParamTarget::StepParam { param } => {
                                        Value::String(param.clone())
                                    }
                                    _ => Value::Nil,
                                },
                            ),
                            (
                                "target-instance-id",
                                match &entry.target {
                                    sequencer::process::ParamTarget::ProcessInlet {
                                        instance_id: Some(id),
                                        ..
                                    } => Value::Number(id.0 as f64),
                                    _ => Value::Nil,
                                },
                            ),
                            ("lo", Value::Number(entry.lo as f64)),
                            ("hi", Value::Number(entry.hi as f64)),
                        ])
                    }),
            ),
        ),
    ])
}

pub(super) fn process_name_initials(name: &str) -> String {
    let initials = name
        .split(|ch: char| !ch.is_ascii_alphanumeric())
        .filter(|part| !part.is_empty())
        .filter_map(|part| part.chars().next())
        .take(4)
        .collect::<String>();
    if initials.len() >= 2 {
        initials
    } else {
        name.chars().take(4).collect::<String>()
    }
}

pub(super) fn process_short_label(class_name: &str, inlet_name: &str) -> String {
    let class = process_name_initials(class_name);
    match (class.is_empty(), inlet_name.is_empty()) {
        (_, true) => "lane".to_string(),
        (true, false) => inlet_name.to_string(),
        (false, false) => format!("{class}/{inlet_name}"),
    }
}

pub(super) fn process_inlet_decimals(kind: Option<&sequencer::process::ProcessInletKind>) -> u8 {
    match kind {
        Some(sequencer::process::ProcessInletKind::Int)
        | Some(sequencer::process::ProcessInletKind::Gate)
        | Some(sequencer::process::ProcessInletKind::Track)
        | Some(sequencer::process::ProcessInletKind::Enum(_)) => 0,
        _ => 2,
    }
}

pub(super) fn process_inlet_range(
    def: Option<&sequencer::process::PublishedProcessDef>,
    inlet: Option<&sequencer::process::PublishedProcessInletDef>,
    default: f32,
) -> (f32, f32) {
    if let (Some(min), Some(max)) = (
        inlet.and_then(|entry| entry.min),
        inlet.and_then(|entry| entry.max),
    ) {
        return (min, max);
    }
    match inlet.map(|entry| &entry.kind) {
        Some(sequencer::process::ProcessInletKind::Gate) => (0.0, 1.0),
        Some(sequencer::process::ProcessInletKind::Track) => (0.0, 127.0),
        Some(sequencer::process::ProcessInletKind::Int) => {
            let center = default.round();
            (center - 24.0, center + 24.0)
        }
        _ => def
            .and_then(|def| def.accumulator.as_ref())
            .and_then(|acc| acc.range)
            .unwrap_or((default - 1.0, default + 1.0)),
    }
}

pub(super) fn process_lane_entries_for_track(
    state: &Arc<SequencerState>,
    track: usize,
) -> Vec<ProcessLaneUiEntry> {
    let Some(chain) = state.composed_track_process_chain(track) else {
        return Vec::new();
    };
    let published = state.published_process_authoring();
    let mut entries = Vec::new();
    for (slot_index, slot) in chain.slots.iter().enumerate() {
        let def = published
            .defs
            .iter()
            .find(|def| def.name == slot.class_name);
        let mut lane_names = def
            .map(|def| {
                def.inlets
                    .iter()
                    .filter(|inlet| inlet.lane)
                    .map(|inlet| inlet.name.clone())
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        for name in slot.lanes.keys() {
            if !lane_names.iter().any(|entry| entry == name) {
                lane_names.push(name.clone());
            }
        }
        for inlet_name in lane_names {
            let inlet =
                def.and_then(|def| def.inlets.iter().find(|entry| entry.name == inlet_name));
            let default = slot
                .inlets
                .get(&inlet_name)
                .and_then(process_literal_as_f32)
                .or_else(|| inlet.and_then(|entry| process_literal_as_f32(&entry.default)))
                .unwrap_or(0.0);
            let (mut min, mut max) = process_inlet_range(def, inlet, default);
            // A slot with lo/hi knobs (the default accumulators and
            // generators) ranges its value lane over them, so the sliders
            // and number picker span what the lane can actually output.
            let is_gate = inlet
                .is_some_and(|entry| matches!(entry.kind, sequencer::process::ProcessInletKind::Gate));
            if !is_gate {
                if let (Some(lo), Some(hi)) = (
                    slot.inlets.get("lo").and_then(process_literal_as_f32),
                    slot.inlets.get("hi").and_then(process_literal_as_f32),
                ) {
                    if hi > lo {
                        min = lo;
                        max = hi;
                    }
                }
            }
            let lane = slot.lanes.get(&inlet_name);
            let values = (0..MAX_STEPS)
                .map(|step| {
                    lane.map(|lane| lane.value_at(step, default))
                        .unwrap_or(default)
                })
                .collect::<Vec<_>>();
            let map_ports = process_mappable_port_values(slot, def);
            let default_lane = sequencer::process::is_default_lane_slot(slot);
            let roster = sequencer::process::is_track_roster_slot(slot);
            // Default lanes read like the builtin step params: the instance
            // name alone ("prob", "acc A"), never "N class/inlet".
            let (label, short_label) = match slot.instance_name.as_deref() {
                Some(name) if default_lane => (name.to_string(), name.to_string()),
                _ => (
                    format!("{} / {}", slot.class_name, inlet_name),
                    process_short_label(&slot.class_name, &inlet_name),
                ),
            };
            entries.push(ProcessLaneUiEntry {
                instance_id: slot.instance_id,
                slot_index,
                class_name: slot.class_name.clone(),
                inlet_name: inlet_name.clone(),
                label,
                short_label,
                kind: inlet
                    .map(|entry| process_inlet_kind_name(&entry.kind).to_string())
                    .unwrap_or_else(|| "float".to_string()),
                min,
                max,
                default,
                decimals: process_inlet_decimals(inlet.map(|entry| &entry.kind)),
                target: def
                    .map(|def| process_ports_label(&def.ports))
                    .unwrap_or_default(),
                map_ports,
                values,
                project: slot.project_layer,
                forked: slot.project_layer
                    && state.has_project_process_lane_override(
                        track,
                        slot.instance_id,
                        &inlet_name,
                    ),
                instance_name: slot.instance_name.clone(),
                default_lane,
                roster,
            });
        }
    }
    entries
}

pub(super) fn process_lane_entry_value(entry: &ProcessLaneUiEntry, mode: usize) -> Value {
    map_value([
        ("mode", Value::Number(mode as f64)),
        (
            "lane-index",
            Value::Number((mode - PROCESS_LANE_MODE_OFFSET) as f64),
        ),
        ("slot-index", Value::Number(entry.slot_index as f64)),
        ("instance-id", Value::Number(entry.instance_id.0 as f64)),
        ("class", Value::String(entry.class_name.clone())),
        ("process", Value::String(entry.class_name.clone())),
        ("inlet", Value::String(entry.inlet_name.clone())),
        ("name", Value::String(entry.inlet_name.clone())),
        ("project", Value::Bool(entry.project)),
        ("forked", Value::Bool(entry.forked)),
        ("default-lane", Value::Bool(entry.default_lane)),
        ("roster", Value::Bool(entry.roster)),
        (
            "instance-name",
            entry
                .instance_name
                .clone()
                .map(Value::String)
                .unwrap_or(Value::Nil),
        ),
        ("label", Value::String(entry.label.clone())),
        ("short-label", Value::String(entry.short_label.clone())),
        ("kind", Value::String(entry.kind.clone())),
        ("min", Value::Number(entry.min as f64)),
        ("max", Value::Number(entry.max as f64)),
        ("default", Value::Number(entry.default as f64)),
        ("decimals", Value::Number(entry.decimals as f64)),
        ("target", Value::String(entry.target.clone())),
        ("map-ports", list_value(entry.map_ports.iter().cloned())),
    ])
}

pub(crate) fn build_process_lanes_value(state: &Arc<SequencerState>, track: usize) -> Value {
    list_value(
        process_lane_entries_for_track(state, track)
            .iter()
            .enumerate()
            .map(|(lane_index, entry)| {
                process_lane_entry_value(entry, PROCESS_LANE_MODE_OFFSET + lane_index)
            }),
    )
}

pub(crate) fn build_all_track_process_lanes_value(
    state: &Arc<SequencerState>,
    track_count: usize,
) -> Value {
    list_value((0..track_count).map(|track| build_process_lanes_value(state, track)))
}

/// Numeric lane data is separate from metadata so slider writes do not
/// invalidate the expanded row's selector, ranges, labels and patchbay.
fn process_lane_values_value(entries: &[ProcessLaneUiEntry]) -> Value {
    list_value(entries.iter().map(|entry| {
        list_value(entry.values.iter().map(|value| Value::Number(*value as f64)))
    }))
}

pub(crate) fn build_all_track_process_lane_values(
    state: &Arc<SequencerState>,
    track_count: usize,
) -> Value {
    list_value((0..track_count).map(|track| {
        process_lane_values_value(&process_lane_entries_for_track(state, track))
    }))
}

/// A lane edit changes one track's values and, on its first project-lane
/// write, its fork marker. It cannot change ports, the library or other tracks.
pub(crate) fn sync_process_lane_track_state(
    rt: &mut Runtime,
    state: &Arc<SequencerState>,
    track: usize,
    current_track: usize,
    viewports: &[ExpandedStepViewport],
) -> bool {
    let entries = process_lane_entries_for_track(state, track);
    let metadata = list_value(entries.iter().enumerate().map(|(index, entry)| {
        process_lane_entry_value(entry, PROCESS_LANE_MODE_OFFSET + index)
    }));
    let mut dirty = rt.set_reactive_list_index(
        "SEQ", "track-process-lanes", track, metadata.clone(),
    ).effects_dirty;
    dirty |= rt.set_reactive_list_index(
        "SEQ", "track-process-lane-values", track, process_lane_values_value(&entries),
    ).effects_dirty;
    if track == current_track {
        dirty |= rt.set_reactive("SEQ", "process-lanes", metadata).effects_dirty;
    }
    let num_steps = state.pattern.track_params[track].get_num_steps().min(MAX_STEPS);
    for viewport in viewports {
        let Some(entry) = viewport.mode.checked_sub(PROCESS_LANE_MODE_OFFSET)
            .and_then(|index| entries.get(index)) else { continue; };
        for slot in 0..PAGE_SIZE {
            let step = viewport.page.saturating_mul(PAGE_SIZE).saturating_add(slot);
            let value = if step < num_steps { entry.values[step] } else { 0.0 };
            for field in [
                expanded_step_slot_param_slider_field(viewport.track_id, viewport.mode, slot),
                expanded_step_slot_param_haptic_field(viewport.track_id, viewport.mode, slot),
            ] {
                dirty |= rt.set_reactive("SEQ", &field, Value::Number(value as f64)).effects_dirty;
            }
        }
        let value = if viewport.cursor_step < num_steps {
            entry.values[viewport.cursor_step]
        } else { 0.0 };
        dirty |= rt.set_reactive(
            "SEQ", &expanded_step_cursor_param_value_field(viewport.track_id),
            Value::Number(value as f64),
        ).effects_dirty;
    }
    dirty
}

pub(super) fn process_lane_value_for_mode(
    state: &Arc<SequencerState>,
    track: usize,
    mode: usize,
    step: usize,
) -> Option<f32> {
    let lane_index = mode.checked_sub(PROCESS_LANE_MODE_OFFSET)?;
    process_lane_entries_for_track(state, track)
        .get(lane_index)
        .and_then(|entry| entry.values.get(step).copied())
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ProcessLaneEditInfo {
    pub(crate) instance_id: sequencer::process::ProcessInstanceId,
    pub(crate) inlet_name: String,
    pub(crate) value: f32,
    pub(crate) min: f32,
    pub(crate) max: f32,
    pub(crate) decimals: u8,
}

pub(crate) fn process_lane_edit_info_for_mode(
    state: &Arc<SequencerState>,
    track: usize,
    mode: usize,
    step: usize,
) -> Option<ProcessLaneEditInfo> {
    let lane_index = mode.checked_sub(PROCESS_LANE_MODE_OFFSET)?;
    process_lane_entries_for_track(state, track)
        .get(lane_index)
        .and_then(|entry| process_lane_edit_info_from_entry(entry, step))
}

pub(crate) fn process_lane_edit_info_for_target(
    state: &Arc<SequencerState>,
    track: usize,
    instance_id: sequencer::process::ProcessInstanceId,
    inlet_name: &str,
    step: usize,
) -> Option<ProcessLaneEditInfo> {
    process_lane_entries_for_track(state, track)
        .iter()
        .find(|entry| entry.instance_id == instance_id && entry.inlet_name == inlet_name)
        .and_then(|entry| process_lane_edit_info_from_entry(entry, step))
}

pub(super) fn process_lane_edit_info_from_entry(
    entry: &ProcessLaneUiEntry,
    step: usize,
) -> Option<ProcessLaneEditInfo> {
    Some(ProcessLaneEditInfo {
        instance_id: entry.instance_id,
        inlet_name: entry.inlet_name.clone(),
        value: *entry.values.get(step)?,
        min: entry.min,
        max: entry.max,
        decimals: entry.decimals,
    })
}

pub(super) fn process_scalar_inlet_value(
    slot: &sequencer::process::TrackProcessSlot,
    inlet: Option<&sequencer::process::PublishedProcessInletDef>,
    inlet_name: &str,
) -> Option<f32> {
    slot.inlets
        .get(inlet_name)
        .and_then(process_literal_as_f32)
        .or_else(|| inlet.and_then(|entry| process_literal_as_f32(&entry.default)))
}

pub(super) fn process_scalar_inlet_entry_value(
    slot: &sequencer::process::TrackProcessSlot,
    def: Option<&sequencer::process::PublishedProcessDef>,
    inlet_name: &str,
    inlet: Option<&sequencer::process::PublishedProcessInletDef>,
) -> Option<Value> {
    let value = process_scalar_inlet_value(slot, inlet, inlet_name)?;
    let default = inlet
        .and_then(|entry| process_literal_as_f32(&entry.default))
        .unwrap_or(value);
    let (min, max) = process_inlet_range(def, inlet, value);
    // Enum inlets carry their option labels so the strip can render a
    // dropdown; the value stays the option index.
    let options = match inlet.map(|entry| &entry.kind) {
        Some(sequencer::process::ProcessInletKind::Enum(options)) => options
            .iter()
            .map(|label| Value::String(label.clone()))
            .collect::<Vec<_>>(),
        _ => Vec::new(),
    };
    Some(map_value([
        ("name", Value::String(inlet_name.to_string())),
        ("label", Value::String(inlet_name.to_string())),
        ("options", list_value(options)),
        (
            "kind",
            Value::String(
                inlet
                    .map(|entry| process_inlet_kind_name(&entry.kind))
                    .unwrap_or("float")
                    .to_string(),
            ),
        ),
        ("value", Value::Number(value as f64)),
        ("default", Value::Number(default as f64)),
        ("min", Value::Number(min as f64)),
        ("max", Value::Number(max as f64)),
        (
            "decimals",
            Value::Number(process_inlet_decimals(inlet.map(|entry| &entry.kind)) as f64),
        ),
        (
            "doc",
            Value::String(
                inlet
                    .and_then(|entry| entry.doc.clone())
                    .unwrap_or_default(),
            ),
        ),
    ]))
}

pub(crate) fn build_process_slots_value(state: &Arc<SequencerState>, track: usize) -> Value {
    let Some(chain) = state.composed_track_process_chain(track) else {
        return list_value(Vec::<Value>::new());
    };
    let published = state.published_process_authoring();
    list_value(chain.slots.iter().enumerate().map(|(slot_index, slot)| {
        let def = published
            .defs
            .iter()
            .find(|def| def.name == slot.class_name);
        let mut scalar_names = def
            .map(|def| {
                def.inlets
                    .iter()
                    .filter(|inlet| !inlet.lane)
                    .map(|inlet| inlet.name.clone())
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        for name in slot.inlets.keys() {
            if !scalar_names.iter().any(|entry| entry == name) && !slot.lanes.contains_key(name) {
                scalar_names.push(name.clone());
            }
        }
        let inlet_values = scalar_names.into_iter().filter_map(|name| {
            let inlet = def.and_then(|def| def.inlets.iter().find(|entry| entry.name == name));
            process_scalar_inlet_entry_value(slot, def, &name, inlet)
        });
        map_value([
            ("slot-index", Value::Number(slot_index as f64)),
            ("instance-id", Value::Number(slot.instance_id.0 as f64)),
            ("class", Value::String(slot.class_name.clone())),
            ("process", Value::String(slot.class_name.clone())),
            ("label", Value::String(slot.class_name.clone())),
            (
                "doc",
                Value::String(def.and_then(|def| def.doc.clone()).unwrap_or_default()),
            ),
            (
                "source-path",
                Value::String(
                    def.and_then(|def| def.source_path.clone())
                        .unwrap_or_default(),
                ),
            ),
            ("enabled", Value::Bool(slot.enabled)),
            ("project", Value::Bool(slot.project_layer)),
            ("default-lane", Value::Bool(sequencer::process::is_default_lane_slot(slot))),
            (
                "instance-name",
                slot.instance_name
                    .clone()
                    .map(Value::String)
                    .unwrap_or(Value::Nil),
            ),
            (
                "target",
                Value::String(
                    def.map(|def| process_ports_label(&def.ports))
                        .unwrap_or_default(),
                ),
            ),
            ("ports", process_slot_ports_value(slot, def)),
            ("inlets", list_value(inlet_values)),
        ])
    }))
}

pub(crate) fn build_all_track_process_slots_value(
    state: &Arc<SequencerState>,
    track_count: usize,
) -> Value {
    list_value((0..track_count).map(|track| build_process_slots_value(state, track)))
}

/// Out ports get cable ids `(track * TRACK_STRIDE + slot-index) * PORT_STRIDE
/// + ordinal`. The patch-port machinery keys cable sources by a single number
/// across the whole buffer layout, and two expanded tracks show two
/// patchbays at once, so the id must be unique per track as well as per
/// port (a slot-only id drew track 3's cables from track 2's ports).
pub(crate) const LANE_PATCH_PORT_STRIDE: usize = 16;
pub(crate) const LANE_PATCH_TRACK_STRIDE: usize = 4096;

pub(crate) fn lane_patch_port_id(track: usize, slot_index: usize, ordinal: usize) -> usize {
    (track * LANE_PATCH_TRACK_STRIDE + slot_index) * LANE_PATCH_PORT_STRIDE + ordinal
}

/// `SEQ.track-lane-patch`: per track, one entry per composed chain slot in
/// fire order, with the cable-level view the lane patchbay draws
/// (docs/default-process-lanes-spec.md, patchbay). Each slot lists its
/// connectable out ports with every reader (the primary binding, then the
/// fan-out entries on that port, each resolved to a chain index), its
/// wireable in ports (lane inlets, gate inlets, and any inlet a reader entry
/// on this track already targets) with the cable ids writing into them, and
/// its mappable ports for the target chip.
pub(crate) fn build_track_lane_patch_value(state: &Arc<SequencerState>, track: usize) -> Value {
    let Some(chain) = state.composed_track_process_chain(track) else {
        return list_value(Vec::<Value>::new());
    };
    let published = state.published_process_authoring();
    let def_for = |slot: &sequencer::process::TrackProcessSlot| {
        published.defs.iter().find(|def| def.name == slot.class_name)
    };
    // Wiring stays within a layer, as the scheduler resolves it
    // (`resolve_process_inlet_target`): a project slot drives project slots.
    let resolve = |writer: &sequencer::process::TrackProcessSlot,
                   target: &sequencer::process::ParamTarget|
     -> Option<(usize, String)> {
        let sequencer::process::ParamTarget::ProcessInlet {
            process,
            inlet,
            instance_id,
        } = target
        else {
            return None;
        };
        let index = chain.slots.iter().position(|slot| {
            slot.project_layer == writer.project_layer
                && slot.class_name == *process
                && instance_id.is_none_or(|id| slot.instance_id == id)
        })?;
        Some((index, inlet.clone()))
    };

    struct Reader {
        slot_index: usize,
        inlet: String,
        source: &'static str,
        fanout_index: Option<usize>,
    }
    let mut out_ports: Vec<Vec<(String, usize, bool, Vec<Reader>)>> = Vec::new();
    let mut in_writers: std::collections::BTreeMap<(usize, String), Vec<usize>> =
        std::collections::BTreeMap::new();
    for (slot_index, slot) in chain.slots.iter().enumerate() {
        let mut entries = Vec::new();
        let connectable = def_for(slot)
            .map(|def| {
                def.ports
                    .iter()
                    .filter(|port| port.is_connectable())
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        for (ordinal, port) in connectable.iter().enumerate() {
            let port_id = lane_patch_port_id(track, slot_index, ordinal);
            let mut readers = Vec::new();
            // A new cable fills the primary binding when nothing holds it;
            // otherwise it becomes a fan-out entry on the port.
            let primary_free = slot.unbound_ports.contains(&port.name)
                || !matches!(slot.bindings.get(&port.name), Some(Some(_)));
            if !slot.unbound_ports.contains(&port.name) {
                if let Some(Some(target)) = slot.bindings.get(&port.name) {
                    if let Some((index, inlet)) = resolve(slot, target) {
                        readers.push(Reader {
                            slot_index: index,
                            inlet,
                            source: "primary",
                            fanout_index: None,
                        });
                    }
                }
            }
            let fanout = slot.fanout.get(&port.name).map(Vec::as_slice).unwrap_or(&[]);
            for (fanout_index, entry) in fanout.iter().enumerate() {
                if let Some((index, inlet)) = resolve(slot, &entry.target) {
                    readers.push(Reader {
                        slot_index: index,
                        inlet,
                        source: "fanout",
                        fanout_index: Some(fanout_index),
                    });
                }
            }
            for reader in &readers {
                in_writers
                    .entry((reader.slot_index, reader.inlet.clone()))
                    .or_default()
                    .push(port_id);
            }
            entries.push((port.name.clone(), port_id, primary_free, readers));
        }
        out_ports.push(entries);
    }

    list_value(chain.slots.iter().enumerate().map(|(slot_index, slot)| {
        let def = def_for(slot);
        let in_ports = def
            .map(|def| {
                def.inlets
                    .iter()
                    .filter(|inlet| {
                        inlet.lane
                            || matches!(inlet.kind, sequencer::process::ProcessInletKind::Gate)
                            || in_writers.contains_key(&(slot_index, inlet.name.clone()))
                    })
                    .enumerate()
                    .map(|(ordinal, inlet)| {
                        let writers = in_writers
                            .get(&(slot_index, inlet.name.clone()))
                            .map(Vec::as_slice)
                            .unwrap_or(&[]);
                        map_value([
                            ("name", Value::String(inlet.name.clone())),
                            ("ordinal", Value::Number(ordinal as f64)),
                            ("lane", Value::Bool(inlet.lane)),
                            ("kind", Value::String(process_inlet_kind_name(&inlet.kind).to_string())),
                            (
                                "writers",
                                list_value(writers.iter().map(|id| Value::Number(*id as f64))),
                            ),
                        ])
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let param_ports = def
            .map(|def| {
                def.ports
                    .iter()
                    .filter(|port| port.is_mappable())
                    .map(|port| process_port_value(slot, port))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let display_name = slot
            .instance_name
            .clone()
            .unwrap_or_else(|| slot.class_name.clone());
        map_value([
            ("slot-index", Value::Number(slot_index as f64)),
            ("instance-id", Value::Number(slot.instance_id.0 as f64)),
            ("name", Value::String(display_name)),
            ("class", Value::String(slot.class_name.clone())),
            ("project", Value::Bool(slot.project_layer)),
            ("enabled", Value::Bool(slot.enabled)),
            ("default-lane", Value::Bool(sequencer::process::is_default_lane_slot(slot))),
            (
                "out-ports",
                list_value(out_ports[slot_index].iter().enumerate().map(
                    |(ordinal, (name, port_id, primary_free, readers))| {
                        map_value([
                            ("name", Value::String(name.clone())),
                            ("ordinal", Value::Number(ordinal as f64)),
                            ("port-id", Value::Number(*port_id as f64)),
                            ("primary-free", Value::Bool(*primary_free)),
                            (
                                "readers",
                                list_value(readers.iter().map(|reader| {
                                    map_value([
                                        ("slot-index", Value::Number(reader.slot_index as f64)),
                                        (
                                            "instance-id",
                                            Value::Number(
                                                chain.slots[reader.slot_index].instance_id.0 as f64,
                                            ),
                                        ),
                                        ("inlet", Value::String(reader.inlet.clone())),
                                        ("source", Value::String(reader.source.to_string())),
                                        (
                                            "fanout-index",
                                            reader
                                                .fanout_index
                                                .map(|index| Value::Number(index as f64))
                                                .unwrap_or(Value::Nil),
                                        ),
                                    ])
                                })),
                            ),
                        ])
                    },
                )),
            ),
            ("in-ports", list_value(in_ports)),
            ("param-ports", list_value(param_ports)),
        ])
    }))
}

pub(crate) fn build_all_track_lane_patch_value(
    state: &Arc<SequencerState>,
    track_count: usize,
) -> Value {
    list_value((0..track_count).map(|track| build_track_lane_patch_value(state, track)))
}

pub(crate) fn build_process_library_value(state: &Arc<SequencerState>) -> Value {
    let published = state.published_process_authoring();
    list_value(published.defs.iter().map(|def| {
        map_value([
            ("name", Value::String(def.name.clone())),
            ("label", Value::String(def.name.clone())),
            ("doc", Value::String(def.doc.clone().unwrap_or_default())),
            (
                "source-path",
                Value::String(def.source_path.clone().unwrap_or_default()),
            ),
            ("target", Value::String(process_ports_label(&def.ports))),
            (
                "ports",
                list_value(def.ports.iter().map(|port| {
                    map_value([
                        ("name", Value::String(port.name.clone())),
                        (
                            "label",
                            Value::String(
                                if port.name == sequencer::process::DEFAULT_PROCESS_PORT {
                                    "default".to_string()
                                } else {
                                    port.name.clone()
                                },
                            ),
                        ),
                        (
                            "hint",
                            Value::String(process_target_hint_label(port.target.as_ref())),
                        ),
                        ("mappable", Value::Bool(port.is_mappable())),
                        ("connectable", Value::Bool(port.is_connectable())),
                        (
                            "target-kind",
                            Value::String(process_target_kind_label(port.effective_target_kind())),
                        ),
                    ])
                })),
            ),
            (
                "lane-count",
                Value::Number(def.inlets.iter().filter(|inlet| inlet.lane).count() as f64),
            ),
        ])
    }))
}

/// `SEQ.track-process-scopes`: per track, one entry per composed chain slot
/// with that slot's state history on this track (project slots have
/// per-track runtime state, so the same lane scopes differently per track).
/// `values` is the first declared state cell's history, `cells` the rest.
pub(crate) fn build_track_process_scopes_value(state: &Arc<SequencerState>) -> Value {
    let scopes = state.process_scope_values();
    let published = state.published_process_authoring();
    let tracks = (0..state.active_track_count()).map(|track| {
        let Some(chain) = state.composed_track_process_chain(track) else {
            return list_value(std::iter::empty());
        };
        let entries = chain.slots.iter().filter_map(|slot| {
            let runtime_id =
                sequencer::process::track_process_slot_runtime_id(slot, track).0;
            let cells = scopes.get(&runtime_id)?;
            let def = published.defs.iter().find(|def| def.name == slot.class_name);
            let primary = def
                .and_then(|def| def.state.first().map(|cell| cell.name.clone()))
                .or_else(|| cells.keys().next().cloned())?;
            let values = cells.get(&primary).cloned().unwrap_or_default();
            let current = values.last().copied().unwrap_or(0.0);
            Some(map_value([
                ("instance-id", Value::Number(slot.instance_id.0 as f64)),
                ("state", Value::String(primary)),
                ("current", Value::Number(current as f64)),
                (
                    "values",
                    list_value(values.iter().map(|value| Value::Number(*value as f64))),
                ),
                ("cells", process_scope_cells_value(cells)),
            ]))
        });
        list_value(entries.collect::<Vec<_>>())
    });
    list_value(tracks.collect::<Vec<_>>())
}

/// Descriptor indices of `track`'s instrument params that an *enabled*
/// process slot currently writes to through a bound OUT port or a fan-out
/// entry. Drives the panel's amber "process-mapped" tint and gates the
/// effective-value overlay so a stale feed entry never draws on an unmapped
/// control. Hint-only (unbound) targets resolve in the scheduler and are not
/// counted here.
pub(crate) fn process_bound_instrument_params(
    state: &SequencerState,
    descriptor: &sequencer::effects::EffectDescriptor,
    track: usize,
) -> HashSet<usize> {
    let mut bound = HashSet::new();
    let Some(chain) = state.composed_track_process_chain(track) else {
        return bound;
    };
    let mut note = |target: &sequencer::process::ParamTarget| {
        if let sequencer::process::ParamTarget::InstrumentParam { param, .. } = target {
            if let Ok(Some(idx)) =
                sequencer::process::process_param_index_by_tag_or_name(descriptor, param)
            {
                bound.insert(idx);
            }
        }
    };
    for slot in chain.slots.iter().filter(|slot| slot.enabled) {
        for target in slot.bindings.values().flatten() {
            note(target);
        }
        for entry in slot.fanout.values().flatten() {
            note(&entry.target);
        }
    }
    bound
}

/// Bus ids an enabled process slot on `track` binds or fans out to.
pub(crate) fn process_bound_bus_sends(state: &SequencerState, track: usize) -> HashSet<u64> {
    let mut bound = HashSet::new();
    let Some(chain) = state.composed_track_process_chain(track) else {
        return bound;
    };
    for slot in chain.slots.iter().filter(|slot| slot.enabled) {
        let targets = slot
            .bindings
            .values()
            .flatten()
            .chain(slot.fanout.values().flatten().map(|entry| &entry.target));
        for target in targets {
            if let sequencer::process::ParamTarget::BusSend { bus } = target {
                bound.insert(*bus);
            }
        }
    }
    bound
}

/// Every numeric state cell's history of one process instance, by cell
/// name: `{:snap (0 -1 ...) :root (...)}`.
fn process_scope_cells_value(cells: &HashMap<String, Vec<f32>>) -> Value {
    let mut map = HashMap::new();
    for (name, values) in cells {
        map.insert(
            name.clone(),
            value_cell(list_value(values.iter().map(|value| Value::Number(*value as f64)))),
        );
    }
    Value::Map(map)
}

/// `SEQ.process-scope-cells`: one `{:runtime-id :cells}` entry per process
/// instance that has fired, for scopes a track chain does not reach — a
/// graph node's patch slots, whose runtime id is their instance id.
pub(crate) fn build_process_scope_cells_value(state: &Arc<SequencerState>) -> Value {
    let scopes = state.process_scope_values();
    let mut ids = scopes.keys().copied().collect::<Vec<_>>();
    ids.sort_unstable();
    list_value(ids.into_iter().map(|id| {
        map_value([
            ("runtime-id", Value::Number(id as f64)),
            ("cells", process_scope_cells_value(&scopes[&id])),
        ])
    }))
}

pub(crate) fn sync_process_scope_state(
    rt: &mut Runtime,
    state: &Arc<SequencerState>,
    track_scopes: bool,
    scope_cells: bool,
) {
    if track_scopes {
        rt.set_reactive(
            "SEQ",
            "track-process-scopes",
            build_track_process_scopes_value(state),
        );
    }
    if scope_cells {
        rt.set_reactive("SEQ", "process-scope-cells", build_process_scope_cells_value(state));
    }
}

pub(crate) fn sync_process_chain_state(
    rt: &mut Runtime,
    state: &Arc<SequencerState>,
    track_count: usize,
    current_track: usize,
) {
    rt.set_reactive(
        "SEQ", "track-process-lane-values",
        build_all_track_process_lane_values(state, track_count),
    );
    rt.set_reactive(
        "SEQ",
        "track-process-lanes",
        build_all_track_process_lanes_value(state, track_count),
    );
    rt.set_reactive(
        "SEQ",
        "process-lanes",
        build_process_lanes_value(state, current_track),
    );
    rt.set_reactive(
        "SEQ",
        "process-slots",
        build_process_slots_value(state, current_track),
    );
    rt.set_reactive(
        "SEQ",
        "track-process-slots",
        build_all_track_process_slots_value(state, track_count),
    );
    rt.set_reactive(
        "SEQ",
        "track-lane-patch",
        build_all_track_lane_patch_value(state, track_count),
    );
    rt.set_reactive("SEQ", "process-library", build_process_library_value(state));
}
