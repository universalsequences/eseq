use super::*;

pub(super) fn insert_rack_param_target(
    pmap: &mut HashMap<String, Rc<RefCell<Value>>>,
    track: usize,
    slot_idx: usize,
) {
    pmap.insert(
        "rack-track".to_string(),
        value_cell(Value::Number(track as f64)),
    );
    pmap.insert(
        "rack-slot".to_string(),
        value_cell(Value::Number(slot_idx as f64)),
    );
}

pub(super) struct RackUiModMetadata {
    source_param_idx: Option<usize>,
    depth_param_idx: usize,
    source_slot: f32,
    source_value_field: Option<String>,
    depth_value: f32,
    depth_value_field: String,
    depth_min: f32,
    depth_max: f32,
    depth_unit: Option<String>,
}

pub(super) fn insert_rack_mod_metadata(
    pmap: &mut HashMap<String, Rc<RefCell<Value>>>,
    targets: &[RackUiModMetadata],
) {
    pmap.insert("modulatable".to_string(), value_cell(Value::Bool(true)));
    let target_values = targets
        .iter()
        .map(|meta| {
            let mut target = HashMap::new();
            if let Some(source_param_idx) = meta.source_param_idx {
                target.insert(
                    "source-idx".to_string(),
                    value_cell(Value::Number(source_param_idx as f64)),
                );
            }
            target.insert(
                "depth-idx".to_string(),
                value_cell(Value::Number(meta.depth_param_idx as f64)),
            );
            target.insert(
                "source-slot".to_string(),
                value_cell(Value::Number(meta.source_slot as f64)),
            );
            if let Some(field) = &meta.source_value_field {
                insert_string_prop(&mut target, "source-value-field", field);
            }
            target.insert(
                "depth".to_string(),
                value_cell(Value::Number(meta.depth_value as f64)),
            );
            insert_string_prop(&mut target, "depth-value-field", &meta.depth_value_field);
            target.insert(
                "depth-min".to_string(),
                value_cell(Value::Number(meta.depth_min as f64)),
            );
            target.insert(
                "depth-max".to_string(),
                value_cell(Value::Number(meta.depth_max as f64)),
            );
            if let Some(unit) = &meta.depth_unit {
                insert_string_prop(&mut target, "depth-unit", unit);
            }
            value_cell(Value::Map(target))
        })
        .collect();
    pmap.insert(
        "mod-targets".to_string(),
        value_cell(Value::List(target_values)),
    );
}

pub(super) fn rack_slot_param_map(
    track: usize,
    slot_idx: usize,
    name: String,
    control: &str,
    idx: Option<usize>,
    value_field: String,
    value: f32,
    min: f32,
    max: f32,
    options: Option<&Vec<String>>,
    mod_targets: Option<&Vec<RackUiModMetadata>>,
    ui_metadata: Option<&sequencer::effects::ParamUiMetadata>,
) -> Rc<RefCell<Value>> {
    let mut pmap: HashMap<String, Rc<RefCell<Value>>> = HashMap::new();
    insert_string_prop(&mut pmap, "name", name.clone());
    insert_string_prop(&mut pmap, "control", control);
    if let Some(idx) = idx {
        pmap.insert("idx".to_string(), value_cell(Value::Number(idx as f64)));
    }
    insert_string_prop(&mut pmap, "value-field", value_field);
    pmap.insert("value".to_string(), value_cell(Value::Number(value as f64)));
    pmap.insert("min".to_string(), value_cell(Value::Number(min as f64)));
    pmap.insert("max".to_string(), value_cell(Value::Number(max as f64)));
    if let Some(labels) = options {
        let selected = labels
            .get(value.round() as usize)
            .cloned()
            .unwrap_or_default();
        insert_string_prop(&mut pmap, "text-value", selected);
        pmap.insert(
            "options".to_string(),
            value_cell(Value::List(
                labels
                    .iter()
                    .cloned()
                    .map(|label| value_cell(Value::String(label)))
                    .collect(),
            )),
        );
    } else if name == "enabled" || name == "sync" {
        pmap.insert("boolean".to_string(), value_cell(Value::Bool(true)));
    }
    if let Some(targets) = mod_targets {
        insert_rack_mod_metadata(&mut pmap, targets);
    }
    insert_param_ui_metadata(&mut pmap, ui_metadata);
    insert_rack_param_target(&mut pmap, track, slot_idx);
    Rc::new(RefCell::new(Value::Map(pmap)))
}

pub(super) fn rack_slot_param_value(
    rack: &sequencer::sequencer::RackTrackSnapshot,
    slot_idx: usize,
    slot: &sequencer::sequencer::RackSlotSnapshot,
    desc: &sequencer::effects::EffectDescriptor,
    param_idx: usize,
    selected_step: Option<usize>,
) -> f32 {
    if let Some(step) = selected_step {
        if let Some(value) = slot
            .instrument_slot
            .plocks
            .get(step)
            .and_then(|step_plocks| step_plocks.get(param_idx))
            .copied()
            .flatten()
        {
            return value;
        }
    }
    if let Some(value) = rack_macro_mapped_value(rack, selected_step, |target| {
        matches!(
            target,
            sequencer::sequencer::RackMacroTarget::SlotInstrumentParam {
                slot,
                param_index,
                ..
            } if *slot == slot_idx && *param_index == param_idx
        )
    }) {
        return value;
    }
    slot.instrument_slot
        .defaults
        .get(param_idx)
        .copied()
        .unwrap_or_else(|| {
            desc.params
                .get(param_idx)
                .map(|param| param.default)
                .unwrap_or_default()
        })
}

pub(super) fn rack_macro_mapped_value(
    rack: &sequencer::sequencer::RackTrackSnapshot,
    selected_step: Option<usize>,
    target_matches: impl Fn(&sequencer::sequencer::RackMacroTarget) -> bool,
) -> Option<f32> {
    rack.macros.iter().find_map(|rack_macro| {
        rack_macro.mappings.iter().find_map(|mapping| {
            if !target_matches(&mapping.target) {
                return None;
            }
            let macro_value = selected_step
                .map(|step| rack_macro.value_at(step))
                .unwrap_or(rack_macro.value);
            let curved = match mapping.curve {
                sequencer::sequencer::RackMacroCurve::Linear => macro_value,
                sequencer::sequencer::RackMacroCurve::Exp => macro_value * macro_value,
                sequencer::sequencer::RackMacroCurve::Log => macro_value.sqrt(),
            };
            Some(mapping.range_min + (mapping.range_max - mapping.range_min) * curved)
        })
    })
}

pub(super) fn selected_rack_slot_voice_mod_source_indices(
    rack: &sequencer::sequencer::RackTrackSnapshot,
    slot_idx: usize,
    desc: &sequencer::effects::EffectDescriptor,
    slot: &sequencer::sequencer::RackSlotSnapshot,
    selected_step: Option<usize>,
) -> Vec<usize> {
    sequencer::instruments::voice_modulator::selected_source_param_indices(&desc.params, |idx, _| {
        rack_slot_param_value(rack, slot_idx, slot, desc, idx, selected_step)
    })
}

pub(super) fn build_selected_rack_slot_instrument_value(
    app: &app::App,
    rack: &sequencer::sequencer::RackTrackSnapshot,
    track: usize,
    slot_idx: usize,
    slot: &sequencer::sequencer::RackSlotSnapshot,
    selected_step: Option<usize>,
) -> Option<Rc<RefCell<Value>>> {
    let desc = app.rack_slot_instrument_descriptor(slot)?;
    let raw_name = rack_slot_raw_name(app, slot_idx, slot);
    let mut synth_params: Vec<Rc<RefCell<Value>>> = Vec::new();
    let mut mod_params: Vec<Rc<RefCell<Value>>> = Vec::new();
    let mut modulation_targets: HashMap<usize, Vec<RackUiModMetadata>> = HashMap::new();
    let use_sampler_depth_units =
        slot.instrument_type == sequencer::sequencer::InstrumentType::Sampler;

    for target in desc
        .instrument_modulation_targets
        .iter()
        .filter_map(|target| {
            let depth_desc = desc.params.get(target.depth_param_idx)?;
            let source_current = target
                .source_param_idx
                .map(|source_param_idx| {
                    rack_slot_param_value(
                        rack,
                        slot_idx,
                        slot,
                        &desc,
                        source_param_idx,
                        selected_step,
                    )
                })
                .unwrap_or(target.modulator_slot as f32);
            let depth_current = rack_slot_param_value(
                rack,
                slot_idx,
                slot,
                &desc,
                target.depth_param_idx,
                selected_step,
            );
            let (depth_min, depth_max) = if use_sampler_depth_units {
                sampler_modulation_depth_display_range(depth_desc, target)
            } else {
                instrument_modulation_depth_display_range(target)
            };
            Some((
                target.base_param_idx,
                RackUiModMetadata {
                    source_param_idx: target.source_param_idx,
                    depth_param_idx: target.depth_param_idx,
                    source_slot: target
                        .source_param_idx
                        .and_then(|source_param_idx| {
                            desc.params
                                .get(source_param_idx)
                                .map(|source_desc| source_desc.stored_to_user(source_current))
                        })
                        .unwrap_or(source_current),
                    source_value_field: target.source_param_idx.map(|source_param_idx| {
                        let source_desc = &desc.params[source_param_idx];
                        rack_slot_instrument_param_value_field(
                            track,
                            slot_idx,
                            source_param_idx,
                            &source_desc.name,
                        )
                    }),
                    depth_value: depth_desc.stored_to_user(depth_current),
                    depth_value_field: rack_slot_instrument_param_value_field(
                        track,
                        slot_idx,
                        target.depth_param_idx,
                        &depth_desc.name,
                    ),
                    depth_min,
                    depth_max,
                    depth_unit: target.depth_unit.clone(),
                },
            ))
        })
    {
        modulation_targets
            .entry(target.0)
            .or_default()
            .push(target.1);
    }

    synth_params.push(rack_slot_param_map(
        track,
        slot_idx,
        "base_note".to_string(),
        "base-note",
        None,
        rack_slot_value_field(
            track,
            slot_idx,
            sequencer::sequencer::RackSlotParam::BaseNote,
        ),
        slot.param_value_at_step(
            sequencer::sequencer::RackSlotParam::BaseNote,
            selected_step.unwrap_or(usize::MAX),
        ),
        -48.0,
        48.0,
        None,
        None,
        None,
    ));

    for (param_idx, pdesc) in desc.params.iter().enumerate() {
        if sequencer::instruments::voice_modulator::is_source_param(pdesc.node_param_idx)
            || pdesc.name.starts_with("__host_mod__")
            || pdesc.name.starts_with("__dgen_mod_active__")
        {
            continue;
        }
        let current = rack_slot_param_value(rack, slot_idx, slot, &desc, param_idx, selected_step);
        let options = match &pdesc.kind {
            sequencer::effects::ParamKind::Enum { labels } => Some(labels),
            _ => None,
        };
        if pdesc.name.starts_with("mod ") {
            mod_params.push(rack_slot_param_map(
                track,
                slot_idx,
                pdesc
                    .name
                    .strip_prefix("mod ")
                    .unwrap_or(&pdesc.name)
                    .to_string(),
                "param",
                Some(param_idx),
                rack_slot_instrument_param_value_field(track, slot_idx, param_idx, &pdesc.name),
                pdesc.stored_to_user(current),
                pdesc.stored_to_user(pdesc.min),
                pdesc.stored_to_user(pdesc.max),
                options,
                None,
                None,
            ));
        } else {
            synth_params.push(rack_slot_param_map(
                track,
                slot_idx,
                pdesc.name.clone(),
                "param",
                Some(param_idx),
                rack_slot_instrument_param_value_field(track, slot_idx, param_idx, &pdesc.name),
                pdesc.stored_to_user(current),
                pdesc.stored_to_user(pdesc.min),
                pdesc.stored_to_user(pdesc.max),
                options,
                modulation_targets.get(&param_idx),
                pdesc.ui_metadata.as_ref(),
            ));
        }
    }

    let source_actual =
        selected_rack_slot_voice_mod_source_indices(rack, slot_idx, &desc, slot, selected_step);
    let mut source_sections: Vec<Rc<RefCell<Value>>> = Vec::new();
    let mut source_names: Vec<Rc<RefCell<Value>>> = Vec::new();
    for slot_number in 1..=sequencer::instruments::voice_modulator::SLOT_COUNT {
        let section_name = sequencer::instruments::voice_modulator::modulator_slot_label(slot_number, "");
        let mut params: Vec<Rc<RefCell<Value>>> = Vec::new();
        let mut source_param: Option<Rc<RefCell<Value>>> = None;
        for &param_idx in &source_actual {
            let Some(pdesc) = desc.params.get(param_idx) else {
                continue;
            };
            if sequencer::instruments::voice_modulator::slot_from_param_name(&pdesc.name) != Some(slot_number) {
                continue;
            }
            let current =
                rack_slot_param_value(rack, slot_idx, slot, &desc, param_idx, selected_step);
            let options = match &pdesc.kind {
                sequencer::effects::ParamKind::Enum { labels } => Some(labels),
                _ => None,
            };
            let param = rack_slot_param_map(
                track,
                slot_idx,
                sequencer::instruments::voice_modulator::source_param_display_name(&pdesc.name),
                "param",
                Some(param_idx),
                rack_slot_instrument_param_value_field(track, slot_idx, param_idx, &pdesc.name),
                pdesc.stored_to_user(current),
                pdesc.stored_to_user(pdesc.min),
                pdesc.stored_to_user(pdesc.max),
                options,
                None,
                None,
            );
            if sequencer::instruments::voice_modulator::source_type_name_from_param_name(&pdesc.name)
                == Some("source")
            {
                source_param = Some(param);
            } else {
                params.push(param);
            }
        }
        source_names.push(value_cell(Value::String(section_name.clone())));
        let mut section_map: HashMap<String, Rc<RefCell<Value>>> = HashMap::new();
        insert_string_prop(&mut section_map, "name", section_name);
        section_map.insert(
            "slot".to_string(),
            value_cell(Value::Number(slot_number as f64)),
        );
        if let Some(source_param) = source_param {
            section_map.insert("source-param".to_string(), source_param);
        }
        section_map.insert("params".to_string(), value_cell(Value::List(params)));
        source_sections.push(value_cell(Value::Map(section_map)));
    }

    let mut panel_map: HashMap<String, Rc<RefCell<Value>>> = HashMap::new();
    insert_string_prop(&mut panel_map, "type", rack_slot_type_name(slot));
    panel_map.insert("track".to_string(), value_cell(Value::Number(track as f64)));
    panel_map.insert(
        "rack-track".to_string(),
        value_cell(Value::Number(track as f64)),
    );
    panel_map.insert(
        "rack-slot".to_string(),
        value_cell(Value::Number(slot_idx as f64)),
    );
    insert_string_prop(&mut panel_map, "name", raw_name.clone());
    insert_string_prop(
        &mut panel_map,
        "display-name",
        instrument_display_name(&raw_name),
    );
    panel_map.insert(
        "synth".to_string(),
        value_cell(Value::List(synth_params.clone())),
    );
    panel_map.insert("params".to_string(), value_cell(Value::List(synth_params)));
    panel_map.insert("mod".to_string(), value_cell(Value::List(mod_params)));
    panel_map.insert(
        "modulators".to_string(),
        value_cell(Value::List(
            desc.instrument_modulators
                .iter()
                .map(|modulator| {
                    let mut map: HashMap<String, Rc<RefCell<Value>>> = HashMap::new();
                    map.insert(
                        "slot".to_string(),
                        value_cell(Value::Number(modulator.slot as f64)),
                    );
                    insert_string_prop(&mut map, "label", modulator.label.clone());
                    Rc::new(RefCell::new(Value::Map(map)))
                })
                .collect(),
        )),
    );
    panel_map.insert(
        "source-names".to_string(),
        value_cell(Value::List(source_names)),
    );
    panel_map.insert(
        "sources".to_string(),
        value_cell(Value::List(source_sections)),
    );
    panel_map.insert(
        "phase-field".to_string(),
        value_cell(Value::String(modulator_phase_field(track))),
    );
    panel_map.insert(
        "level-field".to_string(),
        value_cell(Value::String(modulator_level_field(track))),
    );

    if slot.instrument_type == sequencer::sequencer::InstrumentType::Sampler {
        let (buffer_id, sample_name, _) = slot
            .sample_id
            .clone()
            .unwrap_or_else(|| (-1, raw_name.clone(), app.graph.sample_rate.max(1)));
        let sampler_path = app
            .sample_buffer_path_registry
            .get(&buffer_id)
            .cloned()
            .or_else(|| app.sample_path_registry.get(&sample_name).cloned());
        let registered_sample = sampler_path.as_ref().and_then(|path| {
            let key = path.display().to_string();
            eseqlisp::audio::sample::get_registered_sample(&key).or_else(|| {
                match eseqlisp::audio::sample::SampleBuffer::load_wav(path) {
                    Ok(sample) => {
                        sample.register();
                        eseqlisp::audio::sample::get_registered_sample(&key)
                    }
                    Err(error) => {
                        eprintln!(
                            "rack waveform: failed to register sample {}: {error}",
                            path.display()
                        );
                        None
                    }
                }
            })
        });
        let sample_duration = registered_sample
            .as_ref()
            .map(|sample| sample.duration_seconds)
            .unwrap_or(1.0);
        if let Some(buffer_value) = registered_sample.as_ref().map(|sample| sample.to_value()) {
            panel_map.insert("buffer".to_string(), value_cell(buffer_value));
        }
        let start_raw = rack_slot_param_value(rack, slot_idx, slot, &desc, 2, selected_step);
        let end_raw = rack_slot_param_value(rack, slot_idx, slot, &desc, 3, selected_step);
        panel_map.insert(
            "start-time".to_string(),
            value_cell(Value::Number(start_raw as f64 * sample_duration)),
        );
        insert_string_prop(
            &mut panel_map,
            "start-time-field",
            rack_slot_sampler_selection_time_field(track, slot_idx, "start"),
        );
        panel_map.insert(
            "end-time".to_string(),
            value_cell(Value::Number(end_raw as f64 * sample_duration)),
        );
        insert_string_prop(
            &mut panel_map,
            "end-time-field",
            rack_slot_sampler_selection_time_field(track, slot_idx, "end"),
        );
        panel_map.insert(
            "duration".to_string(),
            value_cell(Value::Number(sample_duration)),
        );
    }

    Some(Rc::new(RefCell::new(Value::Map(panel_map))))
}

pub(super) fn rack_effect_param_value(
    rack: &sequencer::sequencer::RackTrackSnapshot,
    rack_slot: usize,
    effect_slot: usize,
    snapshot: &sequencer::effects::EffectSlotSnapshot,
    descriptor: &sequencer::effects::EffectDescriptor,
    param_idx: usize,
    selected_step: Option<usize>,
) -> f32 {
    let fallback = descriptor.params[param_idx].default;
    if let Some(step) = selected_step {
        if let Some(value) = snapshot
            .plocks
            .get(step)
            .and_then(|step_plocks| step_plocks.get(param_idx))
            .copied()
            .flatten()
        {
            return value;
        }
    }
    rack_macro_mapped_value(rack, selected_step, |target| {
        matches!(
            target,
            sequencer::sequencer::RackMacroTarget::SlotEffectParam {
                slot,
                effect_slot: target_effect_slot,
                param_index,
                ..
            } if *slot == rack_slot
                && *target_effect_slot == effect_slot
                && *param_index == param_idx
        )
    })
    .unwrap_or_else(|| {
        snapshot
            .defaults
            .get(param_idx)
            .copied()
            .unwrap_or(fallback)
    })
}

pub(super) fn build_rack_slot_effect_value(
    rack: &sequencer::sequencer::RackTrackSnapshot,
    track: usize,
    rack_slot: usize,
    effect_slot: usize,
    descriptor: &sequencer::effects::EffectDescriptor,
    snapshot: &sequencer::effects::EffectSlotSnapshot,
    selected_step: Option<usize>,
) -> Rc<RefCell<Value>> {
    let mut modulation_targets: HashMap<usize, Vec<RackUiModMetadata>> = HashMap::new();
    for target in descriptor
        .instrument_modulation_targets
        .iter()
        .filter_map(|target| {
            let depth_desc = descriptor.params.get(target.depth_param_idx)?;
            let source_current = target
                .source_param_idx
                .map(|source_idx| {
                    rack_effect_param_value(
                        rack,
                        rack_slot,
                        effect_slot,
                        snapshot,
                        descriptor,
                        source_idx,
                        selected_step,
                    )
                })
                .unwrap_or(target.modulator_slot as f32);
            let depth_current = rack_effect_param_value(
                rack,
                rack_slot,
                effect_slot,
                snapshot,
                descriptor,
                target.depth_param_idx,
                selected_step,
            );
            Some((
                target.base_param_idx,
                RackUiModMetadata {
                    source_param_idx: target.source_param_idx,
                    depth_param_idx: target.depth_param_idx,
                    source_slot: target
                        .source_param_idx
                        .and_then(|source_idx| {
                            descriptor
                                .params
                                .get(source_idx)
                                .map(|source| source.stored_to_user(source_current))
                        })
                        .unwrap_or(source_current),
                    source_value_field: target.source_param_idx.map(|source_idx| {
                        let source = &descriptor.params[source_idx];
                        rack_slot_effect_param_value_field(
                            track,
                            rack_slot,
                            effect_slot,
                            source_idx,
                            &source.name,
                        )
                    }),
                    depth_value: depth_desc.stored_to_user(depth_current),
                    depth_value_field: rack_slot_effect_param_value_field(
                        track,
                        rack_slot,
                        effect_slot,
                        target.depth_param_idx,
                        &depth_desc.name,
                    ),
                    depth_min: target.depth_min,
                    depth_max: target.depth_max,
                    depth_unit: target.depth_unit.clone(),
                },
            ))
        })
    {
        modulation_targets
            .entry(target.0)
            .or_default()
            .push(target.1);
    }

    let routing_params = modulation_routing_param_indices(descriptor);
    let params = descriptor
        .params
        .iter()
        .enumerate()
        .filter_map(|(param_idx, param)| {
            // Host-routed sidechain params (Compressor `sidechain`, Filterbank
            // `fm source` / `am source`) carry node_param_idx == u32::MAX, which
            // is_source_param also matches. Keep them the way the track chain
            // does (effects_panel.rs) so custom built-in panels that require
            // them still resolve on rack tracks.
            if (sequencer::instruments::voice_modulator::is_source_param(param.node_param_idx)
                && !matches!(
                    param.host_control,
                    Some(sequencer::effects::HostControl::FxSidechain { .. })
                ))
                || routing_params.contains(&param_idx)
                || param.name.starts_with("__host_mod__")
                || param.name.starts_with("__dgen_mod_active__")
            {
                return None;
            }
            let current = rack_effect_param_value(
                rack,
                rack_slot,
                effect_slot,
                snapshot,
                descriptor,
                param_idx,
                selected_step,
            );
            let options = match &param.kind {
                sequencer::effects::ParamKind::Enum { labels } => Some(labels),
                _ => None,
            };
            let value = rack_slot_param_map(
                track,
                rack_slot,
                param.name.clone(),
                "param",
                Some(param_idx),
                rack_slot_effect_param_value_field(
                    track,
                    rack_slot,
                    effect_slot,
                    param_idx,
                    &param.name,
                ),
                current,
                param.min,
                param.max,
                options,
                modulation_targets.get(&param_idx),
                param.ui_metadata.as_ref(),
            );
            if matches!(param.kind, sequencer::effects::ParamKind::Boolean) {
                if let Value::Map(map) = &mut *value.borrow_mut() {
                    map.insert("boolean".to_string(), value_cell(Value::Bool(true)));
                }
            }
            Some(value)
        })
        .collect::<Vec<_>>();

    let source_actual =
        sequencer::instruments::voice_modulator::selected_source_param_indices(&descriptor.params, |idx, _| {
            rack_effect_param_value(
                rack,
                rack_slot,
                effect_slot,
                snapshot,
                descriptor,
                idx,
                selected_step,
            )
        });
    let mut source_sections = Vec::new();
    let mut source_names = Vec::new();
    for slot_number in 1..=sequencer::instruments::voice_modulator::SLOT_COUNT {
        let section_name = sequencer::instruments::voice_modulator::modulator_slot_label(slot_number, "");
        let mut section_params = Vec::new();
        let mut source_param = None;
        for &param_idx in &source_actual {
            let Some(param) = descriptor.params.get(param_idx) else {
                continue;
            };
            if sequencer::instruments::voice_modulator::slot_from_param_name(&param.name) != Some(slot_number) {
                continue;
            }
            let current = rack_effect_param_value(
                rack,
                rack_slot,
                effect_slot,
                snapshot,
                descriptor,
                param_idx,
                selected_step,
            );
            let options = match &param.kind {
                sequencer::effects::ParamKind::Enum { labels } => Some(labels),
                _ => None,
            };
            let value = rack_slot_param_map(
                track,
                rack_slot,
                sequencer::instruments::voice_modulator::source_param_display_name(&param.name),
                "param",
                Some(param_idx),
                rack_slot_effect_param_value_field(
                    track,
                    rack_slot,
                    effect_slot,
                    param_idx,
                    &param.name,
                ),
                param.stored_to_user(current),
                param.stored_to_user(param.min),
                param.stored_to_user(param.max),
                options,
                None,
                None,
            );
            if sequencer::instruments::voice_modulator::source_type_name_from_param_name(&param.name)
                == Some("source")
            {
                source_param = Some(value);
            } else {
                section_params.push(value);
            }
        }
        source_names.push(value_cell(Value::String(section_name.clone())));
        let mut section = HashMap::new();
        insert_string_prop(&mut section, "name", section_name);
        section.insert(
            "slot".to_string(),
            value_cell(Value::Number(slot_number as f64)),
        );
        if let Some(source_param) = source_param {
            section.insert("source-param".to_string(), source_param);
        }
        section.insert(
            "params".to_string(),
            value_cell(Value::List(section_params)),
        );
        source_sections.push(value_cell(Value::Map(section)));
    }

    let mut effect = HashMap::new();
    effect.insert(
        "slot-idx".to_string(),
        value_cell(Value::Number(effect_slot as f64)),
    );
    insert_string_prop(&mut effect, "name", descriptor.name.clone());
    effect.insert(
        "track-idx".to_string(),
        value_cell(Value::Number(track as f64)),
    );
    effect.insert(
        "rack-slot".to_string(),
        value_cell(Value::Number(rack_slot as f64)),
    );
    effect.insert("rack-fx".to_string(), value_cell(Value::Bool(true)));
    effect.insert(
        "builtin".to_string(),
        value_cell(Value::Bool(
            sequencer::effects::EffectDescriptor::builtin_insert(&descriptor.name).is_some()
                || sequencer::effects::dgen_builtin::contains(&descriptor.name),
        )),
    );
    if descriptor.name == sequencer::effects::filter_table::NAME {
        let node_id = snapshot.node_id as i32;
        insert_string_prop(
            &mut effect,
            "table-name",
            sequencer::effects::filter_table::table_name_for(node_id)
                .unwrap_or_else(|| "No table".to_string()),
        );
        effect.insert(
            "table-options".to_string(),
            value_cell(Value::List(
                sequencer::effects::filter_table_asset::list_asset_stems()
                    .into_iter()
                    .map(|stem| value_cell(Value::String(stem)))
                    .collect(),
            )),
        );
        if let Some(mode_label) = sequencer::effects::filter_table::table_ref_for(node_id)
            .and_then(|reference| {
                sequencer::effects::filter_table::decode_table_ref(&reference).1
            })
            .map(|mode| mode.label().to_string())
        {
            insert_string_prop(&mut effect, "table-mode", mode_label);
        }
        insert_string_prop(
            &mut effect,
            "table-engine",
            sequencer::effects::filter_table::engine_for(node_id)
                .display_name()
                .to_string(),
        );
        if sequencer::effects::filter_table::prepared_table_for(node_id).is_some() {
            insert_string_prop(
                &mut effect,
                "table-data-key",
                sequencer::effects::filter_table::visualization_key(node_id),
            );
        }
    }
    effect.insert("params".to_string(), value_cell(Value::List(params)));
    effect.insert(
        "modulators".to_string(),
        value_cell(Value::List(
            descriptor
                .instrument_modulators
                .iter()
                .map(|modulator| {
                    let mut map = HashMap::new();
                    map.insert(
                        "slot".to_string(),
                        value_cell(Value::Number(modulator.slot as f64)),
                    );
                    insert_string_prop(&mut map, "label", modulator.label.clone());
                    value_cell(Value::Map(map))
                })
                .collect(),
        )),
    );
    effect.insert(
        "source-names".to_string(),
        value_cell(Value::List(source_names)),
    );
    effect.insert(
        "sources".to_string(),
        value_cell(Value::List(source_sections)),
    );
    value_cell(Value::Map(effect))
}

pub(super) fn rack_macro_mapping_display_metadata(
    app: &app::App,
    rack: &sequencer::sequencer::RackTrackSnapshot,
    mapping: &sequencer::sequencer::RackMacroMapping,
) -> (String, String, f32, f32, f32, f32, f32, u8, String) {
    let (slot_idx, descriptor, param_idx) = match &mapping.target {
        sequencer::sequencer::RackMacroTarget::SlotInstrumentParam {
            slot, param_index, ..
        } => (
            *slot,
            rack.slots
                .get(*slot)
                .and_then(|slot| app.rack_slot_instrument_descriptor(slot)),
            *param_index,
        ),
        sequencer::sequencer::RackMacroTarget::SlotEffectParam {
            slot,
            effect_slot,
            param_index,
            ..
        } => (
            *slot,
            rack.slots
                .get(*slot)
                .and_then(|slot| slot.effect_descriptors.get(*effect_slot))
                .cloned(),
            *param_index,
        ),
        sequencer::sequencer::RackMacroTarget::SlotParam { slot, param } => {
            return (
                format!("Layer {}", slot + 1),
                param.clone(),
                mapping.range_min,
                mapping.range_max,
                mapping.range_min,
                mapping.range_max,
                1.0,
                2,
                String::new(),
            );
        }
    };
    let Some(descriptor) = descriptor else {
        return (
            format!("Layer {}", slot_idx + 1),
            match &mapping.target {
                sequencer::sequencer::RackMacroTarget::SlotInstrumentParam { param, .. }
                | sequencer::sequencer::RackMacroTarget::SlotEffectParam { param, .. } => {
                    param.clone()
                }
                sequencer::sequencer::RackMacroTarget::SlotParam { param, .. } => param.clone(),
            },
            mapping.range_min,
            mapping.range_max,
            mapping.range_min,
            mapping.range_max,
            1.0,
            2,
            String::new(),
        );
    };
    let Some(param) = descriptor.params.get(param_idx) else {
        return (
            format!("Layer {} · {}", slot_idx + 1, descriptor.name),
            "missing parameter".to_string(),
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
        sequencer::effects::ParamKind::Continuous { unit } => (
            if unit.as_deref() == Some("%") { 1 } else { 2 },
            unit.clone().unwrap_or_default(),
        ),
    };
    (
        format!("Layer {} · {}", slot_idx + 1, descriptor.name),
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

pub(super) fn build_rack_panel_value(
    app: &app::App,
    track: usize,
    selected: &Arc<Mutex<HashSet<usize>>>,
) -> Value {
    let rack = app
        .state
        .pattern
        .rack_tracks
        .lock()
        .unwrap()
        .get(track)
        .cloned()
        .flatten();
    let Some(mut rack) = rack else {
        return Value::List(vec![]);
    };
    for rack_macro in &mut rack.macros {
        if let Some(value) = app.effective_rack_macro_value(track, rack_macro.id, None) {
            rack_macro.value = value;
        }
    }

    let routing_name = match rack.routing {
        sequencer::sequencer::RackRouting::Broadcast => "broadcast",
    };
    let selected_slot = app.selected_rack_slot_index_for_rack(track, &rack);
    let selected_step = selected_plock_step(selected);
    let slots: Vec<Rc<RefCell<Value>>> = rack
        .slots
        .iter()
        .enumerate()
        .map(|(slot_idx, slot)| {
            let slot_type = rack_slot_type_name(slot);
            let raw_name = rack_slot_raw_name(app, slot_idx, slot);
            let mut slot_map: HashMap<String, Rc<RefCell<Value>>> = HashMap::new();
            slot_map.insert(
                "idx".to_string(),
                value_cell(Value::Number(slot_idx as f64)),
            );
            slot_map.insert("track".to_string(), value_cell(Value::Number(track as f64)));
            insert_string_prop(&mut slot_map, "type", slot_type);
            insert_string_prop(&mut slot_map, "name", raw_name.clone());
            insert_string_prop(
                &mut slot_map,
                "display-name",
                instrument_display_name(&raw_name),
            );
            slot_map.insert(
                "choke-group".to_string(),
                value_cell(Value::Number(slot.choke_group.unwrap_or(0) as f64)),
            );
            slot_map.insert(
                "base-note".to_string(),
                value_cell(Value::Number(slot.param_value_at_step(
                    sequencer::sequencer::RackSlotParam::BaseNote,
                    selected_step.unwrap_or(usize::MAX),
                ) as f64)),
            );
            insert_string_prop(
                &mut slot_map,
                "base-note-field",
                rack_slot_value_field(
                    track,
                    slot_idx,
                    sequencer::sequencer::RackSlotParam::BaseNote,
                ),
            );
            slot_map.insert(
                "base-note-min".to_string(),
                value_cell(Value::Number(-48.0)),
            );
            slot_map.insert("base-note-max".to_string(), value_cell(Value::Number(48.0)));
            slot_map.insert(
                "gain".to_string(),
                value_cell(Value::Number(slot.param_value_at_step(
                    sequencer::sequencer::RackSlotParam::Gain,
                    selected_step.unwrap_or(usize::MAX),
                ) as f64)),
            );
            insert_string_prop(
                &mut slot_map,
                "gain-field",
                rack_slot_value_field(track, slot_idx, sequencer::sequencer::RackSlotParam::Gain),
            );
            slot_map.insert("gain-min".to_string(), value_cell(Value::Number(0.0)));
            slot_map.insert("gain-max".to_string(), value_cell(Value::Number(2.0)));
            slot_map.insert(
                "pan".to_string(),
                value_cell(Value::Number(slot.param_value_at_step(
                    sequencer::sequencer::RackSlotParam::Pan,
                    selected_step.unwrap_or(usize::MAX),
                ) as f64)),
            );
            insert_string_prop(
                &mut slot_map,
                "pan-field",
                rack_slot_value_field(track, slot_idx, sequencer::sequencer::RackSlotParam::Pan),
            );
            slot_map.insert("pan-min".to_string(), value_cell(Value::Number(-1.0)));
            slot_map.insert("pan-max".to_string(), value_cell(Value::Number(1.0)));
            slot_map.insert(
                "mute".to_string(),
                value_cell(Value::Bool(
                    slot.param_value_at_step(
                        sequencer::sequencer::RackSlotParam::Mute,
                        selected_step.unwrap_or(usize::MAX),
                    ) > 0.5,
                )),
            );
            insert_string_prop(
                &mut slot_map,
                "mute-field",
                rack_slot_value_field(track, slot_idx, sequencer::sequencer::RackSlotParam::Mute),
            );
            slot_map.insert(
                "solo".to_string(),
                value_cell(Value::Bool(
                    slot.param_value_at_step(
                        sequencer::sequencer::RackSlotParam::Solo,
                        selected_step.unwrap_or(usize::MAX),
                    ) > 0.5,
                )),
            );
            insert_string_prop(
                &mut slot_map,
                "solo-field",
                rack_slot_value_field(track, slot_idx, sequencer::sequencer::RackSlotParam::Solo),
            );
            slot_map.insert(
                "max-polyphony".to_string(),
                value_cell(Value::Number(slot.param_value_at_step(
                    sequencer::sequencer::RackSlotParam::MaxPolyphony,
                    selected_step.unwrap_or(usize::MAX),
                ) as f64)),
            );
            insert_string_prop(
                &mut slot_map,
                "max-polyphony-field",
                rack_slot_value_field(
                    track,
                    slot_idx,
                    sequencer::sequencer::RackSlotParam::MaxPolyphony,
                ),
            );
            slot_map.insert(
                "max-polyphony-min".to_string(),
                value_cell(Value::Number(1.0)),
            );
            slot_map.insert(
                "max-polyphony-max".to_string(),
                value_cell(Value::Number(64.0)),
            );
            slot_map.insert(
                "selected".to_string(),
                value_cell(Value::Bool(Some(slot_idx) == selected_slot)),
            );
            let effects = slot
                .effect_descriptors
                .iter()
                .zip(&slot.effect_slots)
                .enumerate()
                .filter_map(|(effect_idx, (descriptor, snapshot))| {
                    (snapshot.node_id != 0).then(|| {
                        build_rack_slot_effect_value(
                            &rack,
                            track,
                            slot_idx,
                            effect_idx,
                            descriptor,
                            snapshot,
                            selected_step,
                        )
                    })
                });
            let effects = effects.collect::<Vec<_>>();
            slot_map.insert(
                "effect-count".to_string(),
                value_cell(Value::Number(effects.len() as f64)),
            );
            slot_map.insert(
                "processing-cost".to_string(),
                // A stable, inspectable work-unit estimate: one unit per
                // available voice plus one per post-voice slot effect.
                value_cell(Value::Number((slot.max_polyphony + effects.len()) as f64)),
            );
            slot_map.insert("effects".to_string(), value_cell(Value::List(effects)));
            Rc::new(RefCell::new(Value::Map(slot_map)))
        })
        .collect();
    let selected_instrument = selected_slot.and_then(|slot_idx| {
        rack.slots.get(slot_idx).and_then(|slot| {
            build_selected_rack_slot_instrument_value(
                app,
                &rack,
                track,
                slot_idx,
                slot,
                selected_step,
            )
        })
    });

    let mut panel_map: HashMap<String, Rc<RefCell<Value>>> = HashMap::new();
    insert_string_prop(&mut panel_map, "type", "rack");
    panel_map.insert("track".to_string(), value_cell(Value::Number(track as f64)));
    panel_map.insert(
        "selected-slot".to_string(),
        value_cell(Value::Number(
            selected_slot
                .map(|slot_idx| slot_idx as f64)
                .unwrap_or(-1.0),
        )),
    );
    insert_string_prop(
        &mut panel_map,
        "name",
        app.tracks.get(track).cloned().unwrap_or_default(),
    );
    insert_string_prop(
        &mut panel_map,
        "display-name",
        app.tracks
            .get(track)
            .map(|name| instrument_display_name(name))
            .unwrap_or_else(|| "Rack".to_string()),
    );
    insert_string_prop(&mut panel_map, "routing", routing_name);
    let macros = rack
        .macros
        .iter()
        .map(|rack_macro| {
            let mut map = HashMap::new();
            map.insert(
                "id".to_string(),
                value_cell(Value::Number(rack_macro.id.index() as f64)),
            );
            insert_string_prop(&mut map, "key", &rack_macro.id.stable_key());
            insert_string_prop(&mut map, "name", &rack_macro.name);
            insert_string_prop(&mut map, "scope", "rack");
            map.insert(
                "value".to_string(),
                value_cell(Value::Number(
                    app.effective_rack_macro_value(track, rack_macro.id, selected_step)
                        .unwrap_or(rack_macro.value) as f64,
                )),
            );
            insert_string_prop(
                &mut map,
                "value-field",
                rack_macro_value_field(track, rack_macro.id.index()),
            );
            insert_string_prop(
                &mut map,
                "plock-active-field",
                rack_macro_plock_active_field(track, rack_macro.id.index()),
            );
            insert_string_prop(
                &mut map,
                "plock-default-field",
                rack_macro_plock_default_field(track, rack_macro.id.index()),
            );
            map.insert(
                "mapping-count".to_string(),
                value_cell(Value::Number(rack_macro.mappings.len() as f64)),
            );
            let mappings = rack_macro
                .mappings
                .iter()
                .enumerate()
                .map(|(mapping_idx, mapping)| {
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
                    ) = rack_macro_mapping_display_metadata(app, &rack, mapping);
                    let mut target = HashMap::new();
                    target.insert(
                        "mapping-idx".to_string(),
                        value_cell(Value::Number(mapping_idx as f64)),
                    );
                    target.insert(
                        "min".to_string(),
                        value_cell(Value::Number(mapping.range_min as f64)),
                    );
                    target.insert(
                        "max".to_string(),
                        value_cell(Value::Number(mapping.range_max as f64)),
                    );
                    insert_string_prop(&mut target, "path-label", path_label);
                    insert_string_prop(&mut target, "param-label", param_label);
                    target.insert(
                        "display-min".to_string(),
                        value_cell(Value::Number(display_min as f64)),
                    );
                    target.insert(
                        "display-max".to_string(),
                        value_cell(Value::Number(display_max as f64)),
                    );
                    target.insert(
                        "domain-min".to_string(),
                        value_cell(Value::Number(domain_min as f64)),
                    );
                    target.insert(
                        "domain-max".to_string(),
                        value_cell(Value::Number(domain_max as f64)),
                    );
                    target.insert(
                        "display-scale".to_string(),
                        value_cell(Value::Number(display_scale as f64)),
                    );
                    target.insert(
                        "display-decimals".to_string(),
                        value_cell(Value::Number(display_decimals as f64)),
                    );
                    insert_string_prop(&mut target, "display-unit", display_unit);
                    insert_string_prop(
                        &mut target,
                        "curve",
                        match mapping.curve {
                            sequencer::sequencer::RackMacroCurve::Linear => "linear",
                            sequencer::sequencer::RackMacroCurve::Exp => "exp",
                            sequencer::sequencer::RackMacroCurve::Log => "log",
                        },
                    );
                    target.insert("suspended".to_string(), value_cell(Value::Bool(false)));
                    match &mapping.target {
                        sequencer::sequencer::RackMacroTarget::SlotParam { slot, param } => {
                            insert_string_prop(&mut target, "kind", "rack-slot");
                            target.insert(
                                "rack-slot".to_string(),
                                value_cell(Value::Number(*slot as f64)),
                            );
                            insert_string_prop(&mut target, "param", param);
                        }
                        sequencer::sequencer::RackMacroTarget::SlotInstrumentParam {
                            slot,
                            param,
                            param_index,
                        } => {
                            insert_string_prop(&mut target, "kind", "rack-slot-instrument");
                            target.insert(
                                "rack-slot".to_string(),
                                value_cell(Value::Number(*slot as f64)),
                            );
                            target.insert(
                                "param-idx".to_string(),
                                value_cell(Value::Number(*param_index as f64)),
                            );
                            insert_string_prop(&mut target, "param", param);
                        }
                        sequencer::sequencer::RackMacroTarget::SlotEffectParam {
                            slot,
                            effect_slot,
                            param,
                            param_index,
                        } => {
                            insert_string_prop(&mut target, "kind", "rack-slot-effect");
                            target.insert(
                                "rack-slot".to_string(),
                                value_cell(Value::Number(*slot as f64)),
                            );
                            target.insert(
                                "effect-slot".to_string(),
                                value_cell(Value::Number(*effect_slot as f64)),
                            );
                            target.insert(
                                "param-idx".to_string(),
                                value_cell(Value::Number(*param_index as f64)),
                            );
                            insert_string_prop(&mut target, "param", param);
                        }
                    }
                    Rc::new(RefCell::new(Value::Map(target)))
                })
                .collect();
            map.insert("mappings".to_string(), value_cell(Value::List(mappings)));
            Rc::new(RefCell::new(Value::Map(map)))
        })
        .collect();
    panel_map.insert("macros".to_string(), value_cell(Value::List(macros)));
    panel_map.insert(
        "slots".to_string(),
        Rc::new(RefCell::new(Value::List(slots))),
    );
    panel_map.insert(
        "processing-cost".to_string(),
        value_cell(Value::Number(
            rack.slots
                .iter()
                .map(|slot| {
                    slot.max_polyphony
                        + slot
                            .effect_slots
                            .iter()
                            .filter(|effect| effect.node_id != 0)
                            .count()
                })
                .sum::<usize>() as f64,
        )),
    );
    if let Some(selected_instrument) = selected_instrument {
        panel_map.insert("selected-instrument".to_string(), selected_instrument);
    }
    Value::List(vec![Rc::new(RefCell::new(Value::Map(panel_map)))])
}
