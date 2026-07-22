use super::*;

/// Build a Lisp Value::List of effect slot maps for a track.
/// Each slot is a map: {:name "Filter" :params ({:name "cutoff" :value 1000 :min 20 :max 20000} ...)}
pub(crate) fn build_effects_value(
    state: &Arc<SequencerState>,
    track: usize,
    descriptors: &[Vec<sequencer::effects::EffectDescriptor>],
    selected: &Arc<Mutex<HashSet<usize>>>,
) -> Value {
    use sequencer::effects::{ParamKind, SyncDivision};
    use std::collections::HashMap;
    let Some(track_descs) = descriptors.get(track) else {
        return Value::List(vec![]);
    };
    let chain = &state.pattern.effect_chains[track];
    let sel = selected.lock().unwrap();
    // If steps are selected, show p-lock value from first selected step
    let plock_step = sel.iter().copied().min();

    struct UiModMetadata {
        source_param_idx: Option<usize>,
        depth_param_idx: usize,
        source_slot: f32,
        source_value_field: Option<String>,
        depth_value: f32,
        depth_value_field: Option<String>,
        depth_min: f32,
        depth_max: f32,
        depth_unit: Option<String>,
    }

    fn is_generated_host_mod_param(name: &str) -> bool {
        name.starts_with("__host_mod__")
    }

    fn is_hidden_dgen_mod_param(name: &str) -> bool {
        name.starts_with("__dgen_mod_active__")
    }

    fn is_source_param(node_param_idx: u32) -> bool {
        sequencer::voice_modulator::is_source_param(node_param_idx)
    }

    fn rename_source_param(name: &str) -> String {
        sequencer::voice_modulator::source_param_display_name(name)
    }

    fn insert_mod_metadata(
        pmap: &mut HashMap<String, Rc<RefCell<Value>>>,
        targets: &[UiModMetadata],
    ) {
        pmap.insert(
            "modulatable".to_string(),
            Rc::new(RefCell::new(Value::Bool(true))),
        );
        let target_values = targets
            .iter()
            .map(|meta| {
                let mut target = HashMap::new();
                if let Some(source_param_idx) = meta.source_param_idx {
                    target.insert(
                        "source-idx".to_string(),
                        Rc::new(RefCell::new(Value::Number(source_param_idx as f64))),
                    );
                }
                target.insert(
                    "depth-idx".to_string(),
                    Rc::new(RefCell::new(Value::Number(meta.depth_param_idx as f64))),
                );
                target.insert(
                    "source-slot".to_string(),
                    Rc::new(RefCell::new(Value::Number(meta.source_slot as f64))),
                );
                if let Some(field) = &meta.source_value_field {
                    target.insert(
                        "source-value-field".to_string(),
                        Rc::new(RefCell::new(Value::String(field.clone()))),
                    );
                }
                target.insert(
                    "depth".to_string(),
                    Rc::new(RefCell::new(Value::Number(meta.depth_value as f64))),
                );
                if let Some(field) = &meta.depth_value_field {
                    target.insert(
                        "depth-value-field".to_string(),
                        Rc::new(RefCell::new(Value::String(field.clone()))),
                    );
                }
                target.insert(
                    "depth-min".to_string(),
                    Rc::new(RefCell::new(Value::Number(meta.depth_min as f64))),
                );
                target.insert(
                    "depth-max".to_string(),
                    Rc::new(RefCell::new(Value::Number(meta.depth_max as f64))),
                );
                if let Some(unit) = &meta.depth_unit {
                    target.insert(
                        "depth-unit".to_string(),
                        Rc::new(RefCell::new(Value::String(unit.clone()))),
                    );
                }
                Rc::new(RefCell::new(Value::Map(target)))
            })
            .collect();
        pmap.insert(
            "mod-targets".to_string(),
            Rc::new(RefCell::new(Value::List(target_values))),
        );
    }

    let slots: Vec<Rc<RefCell<Value>>> = track_descs
        .iter()
        .enumerate()
        .map(|(slot_idx, desc)| {
            let mut slot_map: HashMap<String, Rc<RefCell<Value>>> = HashMap::new();

            slot_map.insert(
                "name".to_string(),
                Rc::new(RefCell::new(Value::String(desc.name.clone()))),
            );

            slot_map.insert(
                "slot-idx".to_string(),
                Rc::new(RefCell::new(Value::Number(slot_idx as f64))),
            );
            slot_map.insert(
                "track-idx".to_string(),
                Rc::new(RefCell::new(Value::Number(track as f64))),
            );
            slot_map.insert(
                "builtin".to_string(),
                Rc::new(RefCell::new(Value::Bool(
                    sequencer::effects::EffectDescriptor::builtin_insert(&desc.name).is_some()
                        || sequencer::effects::conv_reverb::is_dgen_builtin(&desc.name),
                ))),
            );

            let slot = chain.get(slot_idx);

            // Convolution Reverb: surface the current IR's display name for the
            // panel label (keyed by the live node id).
            if sequencer::effects::conv_reverb::is_dgen_builtin(&desc.name) {
                let node_id = slot
                    .map(|s| s.node_id.load(Ordering::Relaxed) as i32)
                    .unwrap_or(0);
                let ir_name = sequencer::effects::conv_reverb::ir_name_for(node_id)
                    .unwrap_or_else(|| "No IR".to_string());
                slot_map.insert(
                    "ir-name".to_string(),
                    Rc::new(RefCell::new(Value::String(ir_name))),
                );
            }
            let mut modulation_targets: HashMap<usize, Vec<UiModMetadata>> = HashMap::new();
            for target in desc
                .instrument_modulation_targets
                .iter()
                .filter_map(|target| {
                    let depth_desc = desc.params.get(target.depth_param_idx)?;
                    let source_default = if let Some(source_param_idx) = target.source_param_idx {
                        if let Some(slot) = slot {
                            if source_param_idx < slot.num_params.load(Ordering::Relaxed) as usize {
                                slot.defaults.get(source_param_idx)
                            } else {
                                desc.params.get(source_param_idx)?.default
                            }
                        } else {
                            desc.params.get(source_param_idx)?.default
                        }
                    } else {
                        target.modulator_slot as f32
                    };
                    let depth_default = if let Some(slot) = slot {
                        if target.depth_param_idx < slot.num_params.load(Ordering::Relaxed) as usize
                        {
                            slot.defaults.get(target.depth_param_idx)
                        } else {
                            depth_desc.default
                        }
                    } else {
                        depth_desc.default
                    };
                    let source_current = target
                        .source_param_idx
                        .and_then(|source_param_idx| {
                            plock_step.and_then(|step| {
                                slot.and_then(|slot| slot.plocks.get(step, source_param_idx))
                            })
                        })
                        .unwrap_or(source_default);
                    let depth_current = plock_step
                        .and_then(|step| {
                            slot.and_then(|slot| slot.plocks.get(step, target.depth_param_idx))
                        })
                        .unwrap_or(depth_default);
                    Some((
                        target.base_param_idx,
                        UiModMetadata {
                            source_param_idx: target.source_param_idx,
                            depth_param_idx: target.depth_param_idx,
                            source_slot: target
                                .source_param_idx
                                .and_then(|source_param_idx| {
                                    desc.params.get(source_param_idx).map(|source_desc| {
                                        source_desc.stored_to_user(source_current)
                                    })
                                })
                                .unwrap_or(source_current),
                            source_value_field: target.source_param_idx.map(|source_param_idx| {
                                let source_desc = &desc.params[source_param_idx];
                                track_effect_param_value_field(
                                    track,
                                    slot_idx,
                                    source_param_idx,
                                    &source_desc.name,
                                )
                            }),
                            depth_value: depth_desc.stored_to_user(depth_current),
                            depth_value_field: Some(track_effect_param_value_field(
                                track,
                                slot_idx,
                                target.depth_param_idx,
                                &depth_desc.name,
                            )),
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

            let modulation_routing_params = modulation_routing_param_indices(desc);

            let params: Vec<Rc<RefCell<Value>>> = desc
                .params
                .iter()
                .enumerate()
                .filter_map(|(param_idx, pdesc)| {
                    if (is_source_param(pdesc.node_param_idx)
                        && !matches!(
                            pdesc.host_control,
                            Some(sequencer::effects::HostControl::FxSidechain { .. })
                        ))
                        || modulation_routing_params.contains(&param_idx)
                        || is_generated_host_mod_param(&pdesc.name)
                        || is_hidden_dgen_mod_param(&pdesc.name)
                    {
                        return None;
                    }
                    let delay_synced = if desc.name == "Delay" {
                        chain
                            .get(slot_idx)
                            .map(|s| s.defaults.get(1) > 0.5)
                            .unwrap_or(false)
                    } else {
                        false
                    };
                    let default_val = chain
                        .get(slot_idx)
                        .map(|s| {
                            if param_idx < s.num_params.load(Ordering::Relaxed) as usize {
                                s.defaults.get(param_idx)
                            } else {
                                pdesc.default
                            }
                        })
                        .unwrap_or(pdesc.default);
                    // Show p-lock value if steps are selected, fall back to default
                    let current_val = plock_step
                        .and_then(|step| chain.get(slot_idx)?.plocks.get(step, param_idx))
                        .unwrap_or(default_val);

                    let mut pmap: HashMap<String, Rc<RefCell<Value>>> = HashMap::new();
                    pmap.insert(
                        "name".to_string(),
                        Rc::new(RefCell::new(Value::String(pdesc.name.clone()))),
                    );
                    pmap.insert(
                        "idx".to_string(),
                        Rc::new(RefCell::new(Value::Number(param_idx as f64))),
                    );
                    pmap.insert(
                        "value".to_string(),
                        Rc::new(RefCell::new(Value::Number(current_val as f64))),
                    );
                    pmap.insert(
                        "min".to_string(),
                        Rc::new(RefCell::new(Value::Number(pdesc.min as f64))),
                    );
                    pmap.insert(
                        "max".to_string(),
                        Rc::new(RefCell::new(Value::Number(pdesc.max as f64))),
                    );
                    match &pdesc.kind {
                        ParamKind::Boolean => {
                            pmap.insert(
                                "boolean".to_string(),
                                Rc::new(RefCell::new(Value::Bool(true))),
                            );
                            if param_supports_value_binding(pdesc) {
                                insert_string_prop(
                                    &mut pmap,
                                    "value-field",
                                    track_effect_param_value_field(
                                        track,
                                        slot_idx,
                                        param_idx,
                                        &pdesc.name,
                                    ),
                                );
                            }
                        }
                        ParamKind::Enum { labels } => {
                            let selected = labels
                                .get(current_val.round() as usize)
                                .cloned()
                                .unwrap_or_default();
                            let option_values = labels
                                .iter()
                                .cloned()
                                .map(|label| Rc::new(RefCell::new(Value::String(label))))
                                .collect();
                            pmap.insert(
                                "text-value".to_string(),
                                Rc::new(RefCell::new(Value::String(selected))),
                            );
                            pmap.insert(
                                "options".to_string(),
                                Rc::new(RefCell::new(Value::List(option_values))),
                            );
                            if param_supports_value_binding(pdesc) {
                                insert_string_prop(
                                    &mut pmap,
                                    "value-field",
                                    track_effect_param_value_field(
                                        track,
                                        slot_idx,
                                        param_idx,
                                        &pdesc.name,
                                    ),
                                );
                            }
                        }
                        ParamKind::Continuous { .. } => {
                            if desc.name == "Delay" && param_idx == 2 && delay_synced {
                                let labels: Vec<String> = SyncDivision::ALL
                                    .iter()
                                    .map(|d| d.label().to_string())
                                    .collect();
                                let selected_idx = (current_val.round() as usize)
                                    .min(labels.len().saturating_sub(1));
                                let selected =
                                    labels.get(selected_idx).cloned().unwrap_or_default();
                                let option_values = labels
                                    .into_iter()
                                    .map(|label| Rc::new(RefCell::new(Value::String(label))))
                                    .collect();
                                pmap.insert(
                                    "text-value".to_string(),
                                    Rc::new(RefCell::new(Value::String(selected))),
                                );
                                pmap.insert(
                                    "options".to_string(),
                                    Rc::new(RefCell::new(Value::List(option_values))),
                                );
                                pmap.insert(
                                    "min".to_string(),
                                    Rc::new(RefCell::new(Value::Number(0.0))),
                                );
                                pmap.insert(
                                    "max".to_string(),
                                    Rc::new(RefCell::new(Value::Number(
                                        (SyncDivision::ALL.len() - 1) as f64,
                                    ))),
                                );
                            }
                            if param_supports_value_binding(pdesc) {
                                insert_string_prop(
                                    &mut pmap,
                                    "value-field",
                                    track_effect_param_value_field(
                                        track,
                                        slot_idx,
                                        param_idx,
                                        &pdesc.name,
                                    ),
                                );
                            }
                        }
                    }
                    if let Some(targets) = modulation_targets.get(&param_idx) {
                        insert_mod_metadata(&mut pmap, targets);
                    }
                    insert_param_ui_metadata(&mut pmap, pdesc.ui_metadata.as_ref());
                    Some(Rc::new(RefCell::new(Value::Map(pmap))))
                })
                .collect();

            let source_actual =
                selected_voice_mod_source_indices_for_optional_slot(desc, slot, plock_step);
            let mut source_sections: Vec<Rc<RefCell<Value>>> = Vec::new();
            let mut source_names: Vec<Rc<RefCell<Value>>> = Vec::new();
            for slot_number in 1..=sequencer::voice_modulator::SLOT_COUNT {
                let section_name =
                    sequencer::voice_modulator::modulator_slot_label(slot_number, "");
                let mut section_params: Vec<Rc<RefCell<Value>>> = Vec::new();
                let mut source_param: Option<Rc<RefCell<Value>>> = None;
                for &param_idx in &source_actual {
                    let Some(pdesc) = desc.params.get(param_idx) else {
                        continue;
                    };
                    if sequencer::voice_modulator::slot_from_param_name(&pdesc.name)
                        != Some(slot_number)
                    {
                        continue;
                    }
                    let default_val = slot
                        .map(|slot| {
                            if param_idx < slot.num_params.load(Ordering::Relaxed) as usize {
                                slot.defaults.get(param_idx)
                            } else {
                                pdesc.default
                            }
                        })
                        .unwrap_or(pdesc.default);
                    let current_val = plock_step
                        .and_then(|step| slot.and_then(|slot| slot.plocks.get(step, param_idx)))
                        .unwrap_or(default_val);
                    let mut pmap: HashMap<String, Rc<RefCell<Value>>> = HashMap::new();
                    pmap.insert(
                        "name".to_string(),
                        Rc::new(RefCell::new(Value::String(rename_source_param(
                            &pdesc.name,
                        )))),
                    );
                    pmap.insert(
                        "idx".to_string(),
                        Rc::new(RefCell::new(Value::Number(param_idx as f64))),
                    );
                    pmap.insert(
                        "value".to_string(),
                        Rc::new(RefCell::new(Value::Number(
                            pdesc.stored_to_user(current_val) as f64,
                        ))),
                    );
                    pmap.insert(
                        "min".to_string(),
                        Rc::new(RefCell::new(Value::Number(
                            pdesc.stored_to_user(pdesc.min) as f64
                        ))),
                    );
                    pmap.insert(
                        "max".to_string(),
                        Rc::new(RefCell::new(Value::Number(
                            pdesc.stored_to_user(pdesc.max) as f64
                        ))),
                    );
                    match &pdesc.kind {
                        ParamKind::Boolean => {
                            pmap.insert(
                                "boolean".to_string(),
                                Rc::new(RefCell::new(Value::Bool(true))),
                            );
                        }
                        ParamKind::Enum { labels } => {
                            let selected = labels
                                .get(current_val.round() as usize)
                                .cloned()
                                .unwrap_or_default();
                            let option_values = labels
                                .iter()
                                .cloned()
                                .map(|label| Rc::new(RefCell::new(Value::String(label))))
                                .collect();
                            pmap.insert(
                                "text-value".to_string(),
                                Rc::new(RefCell::new(Value::String(selected))),
                            );
                            pmap.insert(
                                "options".to_string(),
                                Rc::new(RefCell::new(Value::List(option_values))),
                            );
                        }
                        ParamKind::Continuous { .. } => {}
                    }
                    insert_string_prop(
                        &mut pmap,
                        "value-field",
                        track_effect_param_value_field(track, slot_idx, param_idx, &pdesc.name),
                    );
                    let param_value = Rc::new(RefCell::new(Value::Map(pmap)));
                    if sequencer::voice_modulator::source_type_name_from_param_name(&pdesc.name)
                        == Some("source")
                    {
                        source_param = Some(param_value);
                    } else {
                        section_params.push(param_value);
                    }
                }
                source_names.push(Rc::new(RefCell::new(Value::String(section_name.clone()))));
                let mut section_map: HashMap<String, Rc<RefCell<Value>>> = HashMap::new();
                section_map.insert(
                    "name".to_string(),
                    Rc::new(RefCell::new(Value::String(section_name))),
                );
                section_map.insert(
                    "slot".to_string(),
                    Rc::new(RefCell::new(Value::Number(slot_number as f64))),
                );
                if let Some(source_param) = source_param {
                    section_map.insert("source-param".to_string(), source_param);
                }
                section_map.insert(
                    "params".to_string(),
                    Rc::new(RefCell::new(Value::List(section_params))),
                );
                source_sections.push(Rc::new(RefCell::new(Value::Map(section_map))));
            }

            slot_map.insert(
                "params".to_string(),
                Rc::new(RefCell::new(Value::List(params))),
            );
            slot_map.insert(
                "modulators".to_string(),
                Rc::new(RefCell::new(Value::List(
                    desc.instrument_modulators
                        .iter()
                        .map(|modulator| {
                            let mut map: HashMap<String, Rc<RefCell<Value>>> = HashMap::new();
                            map.insert(
                                "slot".to_string(),
                                Rc::new(RefCell::new(Value::Number(modulator.slot as f64))),
                            );
                            map.insert(
                                "label".to_string(),
                                Rc::new(RefCell::new(Value::String(modulator.label.clone()))),
                            );
                            Rc::new(RefCell::new(Value::Map(map)))
                        })
                        .collect(),
                ))),
            );
            slot_map.insert(
                "source-names".to_string(),
                Rc::new(RefCell::new(Value::List(source_names))),
            );
            slot_map.insert(
                "sources".to_string(),
                Rc::new(RefCell::new(Value::List(source_sections))),
            );

            Rc::new(RefCell::new(Value::Map(slot_map)))
        })
        .collect();

    Value::List(slots)
}

pub(crate) fn build_bus_effects_value(app: &app::App) -> Value {
    build_bus_effects_value_for_selection(app, None)
}

pub(crate) fn build_bus_effects_value_for_selection(
    app: &app::App,
    selected: Option<&Arc<Mutex<HashSet<usize>>>>,
) -> Value {
    use sequencer::effects::{ParamKind, SyncDivision};
    use std::collections::HashMap;

    let plock_step = selected.and_then(|selected| selected.lock().unwrap().iter().copied().min());

    struct UiModMetadata {
        source_param_idx: Option<usize>,
        depth_param_idx: usize,
        source_slot: f32,
        source_value_field: Option<String>,
        depth_value: f32,
        depth_value_field: Option<String>,
        depth_min: f32,
        depth_max: f32,
        depth_unit: Option<String>,
    }

    fn is_generated_host_mod_param(name: &str) -> bool {
        name.starts_with("__host_mod__")
    }

    fn is_hidden_dgen_mod_param(name: &str) -> bool {
        name.starts_with("__dgen_mod_active__")
    }

    fn is_source_param(node_param_idx: u32) -> bool {
        sequencer::voice_modulator::is_source_param(node_param_idx)
    }

    fn rename_source_param(name: &str) -> String {
        sequencer::voice_modulator::source_param_display_name(name)
    }

    fn bus_slot_param_stored_value(
        slot: Option<&sequencer::effects::EffectSlotSnapshot>,
        desc: &sequencer::effects::EffectDescriptor,
        param_idx: usize,
        plock_step: Option<usize>,
    ) -> f32 {
        let Some(pdesc) = desc.params.get(param_idx) else {
            return 0.0;
        };
        slot.and_then(|slot| {
            plock_step
                .and_then(|step| {
                    slot.plocks
                        .get(step)
                        .and_then(|step_plocks| step_plocks.get(param_idx))
                        .copied()
                        .flatten()
                })
                .or_else(|| {
                    if param_idx < slot.num_params as usize {
                        slot.defaults.get(param_idx).copied()
                    } else {
                        None
                    }
                })
        })
        .unwrap_or(pdesc.default)
    }

    fn selected_bus_voice_mod_source_indices(
        desc: &sequencer::effects::EffectDescriptor,
        slot: Option<&sequencer::effects::EffectSlotSnapshot>,
        plock_step: Option<usize>,
    ) -> Vec<usize> {
        sequencer::voice_modulator::selected_source_param_indices(&desc.params, |idx, param| {
            slot.map(|_| bus_slot_param_stored_value(slot, desc, idx, plock_step))
                .unwrap_or(param.default)
        })
    }

    fn insert_mod_metadata(
        pmap: &mut HashMap<String, Rc<RefCell<Value>>>,
        targets: &[UiModMetadata],
    ) {
        pmap.insert(
            "modulatable".to_string(),
            Rc::new(RefCell::new(Value::Bool(true))),
        );
        let target_values = targets
            .iter()
            .map(|meta| {
                let mut target = HashMap::new();
                if let Some(source_param_idx) = meta.source_param_idx {
                    target.insert(
                        "source-idx".to_string(),
                        Rc::new(RefCell::new(Value::Number(source_param_idx as f64))),
                    );
                }
                target.insert(
                    "depth-idx".to_string(),
                    Rc::new(RefCell::new(Value::Number(meta.depth_param_idx as f64))),
                );
                target.insert(
                    "source-slot".to_string(),
                    Rc::new(RefCell::new(Value::Number(meta.source_slot as f64))),
                );
                if let Some(field) = &meta.source_value_field {
                    target.insert(
                        "source-value-field".to_string(),
                        Rc::new(RefCell::new(Value::String(field.clone()))),
                    );
                }
                target.insert(
                    "depth".to_string(),
                    Rc::new(RefCell::new(Value::Number(meta.depth_value as f64))),
                );
                if let Some(field) = &meta.depth_value_field {
                    target.insert(
                        "depth-value-field".to_string(),
                        Rc::new(RefCell::new(Value::String(field.clone()))),
                    );
                }
                target.insert(
                    "depth-min".to_string(),
                    Rc::new(RefCell::new(Value::Number(meta.depth_min as f64))),
                );
                target.insert(
                    "depth-max".to_string(),
                    Rc::new(RefCell::new(Value::Number(meta.depth_max as f64))),
                );
                if let Some(unit) = &meta.depth_unit {
                    target.insert(
                        "depth-unit".to_string(),
                        Rc::new(RefCell::new(Value::String(unit.clone()))),
                    );
                }
                Rc::new(RefCell::new(Value::Map(target)))
            })
            .collect();
        pmap.insert(
            "mod-targets".to_string(),
            Rc::new(RefCell::new(Value::List(target_values))),
        );
    }

    let buses: Vec<Rc<RefCell<Value>>> = app
        .buses
        .iter()
        .enumerate()
        .map(|(bus_idx, bus)| {
            let slots: Vec<Rc<RefCell<Value>>> = bus
                .effect_descriptors
                .iter()
                .enumerate()
                .map(|(slot_idx, desc)| {
                    let mut slot_map: HashMap<String, Rc<RefCell<Value>>> = HashMap::new();
                    slot_map.insert(
                        "name".to_string(),
                        Rc::new(RefCell::new(Value::String(desc.name.clone()))),
                    );
                    slot_map.insert(
                        "slot-idx".to_string(),
                        Rc::new(RefCell::new(Value::Number(slot_idx as f64))),
                    );
                    slot_map.insert(
                        "bus-idx".to_string(),
                        Rc::new(RefCell::new(Value::Number(bus_idx as f64))),
                    );
                    slot_map.insert(
                        "bus-fx".to_string(),
                        Rc::new(RefCell::new(Value::Bool(true))),
                    );
                    slot_map.insert(
                        "builtin".to_string(),
                        Rc::new(RefCell::new(Value::Bool(
                            sequencer::effects::EffectDescriptor::builtin_insert(&desc.name)
                                .is_some()
                                || sequencer::effects::conv_reverb::is_dgen_builtin(&desc.name),
                        ))),
                    );
                    // Convolution Reverb: surface the current IR name for the label.
                    if sequencer::effects::conv_reverb::is_dgen_builtin(&desc.name) {
                        let node_id = bus
                            .effect_slots
                            .get(slot_idx)
                            .map(|s| s.node_id as i32)
                            .unwrap_or(0);
                        let ir_name = sequencer::effects::conv_reverb::ir_name_for(node_id)
                            .unwrap_or_else(|| "No IR".to_string());
                        slot_map.insert(
                            "ir-name".to_string(),
                            Rc::new(RefCell::new(Value::String(ir_name))),
                        );
                    }

                    let slot = bus.effect_slots.get(slot_idx);
                    let mut modulation_targets: HashMap<usize, Vec<UiModMetadata>> = HashMap::new();
                    for target in desc
                        .instrument_modulation_targets
                        .iter()
                        .filter_map(|target| {
                            let depth_desc = desc.params.get(target.depth_param_idx)?;
                            let source_current =
                                if let Some(source_param_idx) = target.source_param_idx {
                                    bus_slot_param_stored_value(
                                        slot,
                                        desc,
                                        source_param_idx,
                                        plock_step,
                                    )
                                } else {
                                    target.modulator_slot as f32
                                };
                            let depth_current = bus_slot_param_stored_value(
                                slot,
                                desc,
                                target.depth_param_idx,
                                plock_step,
                            );
                            Some((
                                target.base_param_idx,
                                UiModMetadata {
                                    source_param_idx: target.source_param_idx,
                                    depth_param_idx: target.depth_param_idx,
                                    source_slot: target
                                        .source_param_idx
                                        .and_then(|source_param_idx| {
                                            desc.params.get(source_param_idx).map(|source_desc| {
                                                source_desc.stored_to_user(source_current)
                                            })
                                        })
                                        .unwrap_or(source_current),
                                    source_value_field: target.source_param_idx.map(
                                        |source_param_idx| {
                                            let source_desc = &desc.params[source_param_idx];
                                            bus_effect_param_value_field(
                                                bus_idx,
                                                slot_idx,
                                                source_param_idx,
                                                &source_desc.name,
                                            )
                                        },
                                    ),
                                    depth_value: depth_desc.stored_to_user(depth_current),
                                    depth_value_field: Some(bus_effect_param_value_field(
                                        bus_idx,
                                        slot_idx,
                                        target.depth_param_idx,
                                        &depth_desc.name,
                                    )),
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

                    let modulation_routing_params = modulation_routing_param_indices(desc);

                    let params: Vec<Rc<RefCell<Value>>> = desc
                        .params
                        .iter()
                        .enumerate()
                        .filter_map(|(param_idx, pdesc)| {
                            if (is_source_param(pdesc.node_param_idx)
                                && !matches!(
                                    pdesc.host_control,
                                    Some(sequencer::effects::HostControl::FxSidechain { .. })
                                ))
                                || modulation_routing_params.contains(&param_idx)
                                || is_generated_host_mod_param(&pdesc.name)
                                || is_hidden_dgen_mod_param(&pdesc.name)
                            {
                                return None;
                            }
                            let current_val =
                                bus_slot_param_stored_value(slot, desc, param_idx, plock_step);
                            let delay_synced = if desc.name == "Delay" {
                                slot.map(|slot| slot.defaults.get(1).copied().unwrap_or(0.0) > 0.5)
                                    .unwrap_or(false)
                            } else {
                                false
                            };
                            let mut pmap: HashMap<String, Rc<RefCell<Value>>> = HashMap::new();
                            pmap.insert(
                                "name".to_string(),
                                Rc::new(RefCell::new(Value::String(pdesc.name.clone()))),
                            );
                            pmap.insert(
                                "idx".to_string(),
                                Rc::new(RefCell::new(Value::Number(param_idx as f64))),
                            );
                            pmap.insert(
                                "value".to_string(),
                                Rc::new(RefCell::new(Value::Number(current_val as f64))),
                            );
                            pmap.insert(
                                "min".to_string(),
                                Rc::new(RefCell::new(Value::Number(pdesc.min as f64))),
                            );
                            pmap.insert(
                                "max".to_string(),
                                Rc::new(RefCell::new(Value::Number(pdesc.max as f64))),
                            );
                            match &pdesc.kind {
                                ParamKind::Boolean => {
                                    pmap.insert(
                                        "boolean".to_string(),
                                        Rc::new(RefCell::new(Value::Bool(true))),
                                    );
                                    if param_supports_value_binding(pdesc) {
                                        insert_string_prop(
                                            &mut pmap,
                                            "value-field",
                                            bus_effect_param_value_field(
                                                bus_idx,
                                                slot_idx,
                                                param_idx,
                                                &pdesc.name,
                                            ),
                                        );
                                    }
                                }
                                ParamKind::Enum { labels } => {
                                    let selected = labels
                                        .get(current_val.round() as usize)
                                        .cloned()
                                        .unwrap_or_default();
                                    let option_values = labels
                                        .iter()
                                        .cloned()
                                        .map(|label| Rc::new(RefCell::new(Value::String(label))))
                                        .collect();
                                    pmap.insert(
                                        "text-value".to_string(),
                                        Rc::new(RefCell::new(Value::String(selected))),
                                    );
                                    pmap.insert(
                                        "options".to_string(),
                                        Rc::new(RefCell::new(Value::List(option_values))),
                                    );
                                    if param_supports_value_binding(pdesc) {
                                        insert_string_prop(
                                            &mut pmap,
                                            "value-field",
                                            bus_effect_param_value_field(
                                                bus_idx,
                                                slot_idx,
                                                param_idx,
                                                &pdesc.name,
                                            ),
                                        );
                                    }
                                }
                                ParamKind::Continuous { .. } => {
                                    if desc.name == "Delay" && param_idx == 2 && delay_synced {
                                        let labels: Vec<String> = SyncDivision::ALL
                                            .iter()
                                            .map(|d| d.label().to_string())
                                            .collect();
                                        let selected_idx = (current_val.round() as usize)
                                            .min(labels.len().saturating_sub(1));
                                        let selected =
                                            labels.get(selected_idx).cloned().unwrap_or_default();
                                        let option_values = labels
                                            .into_iter()
                                            .map(|label| {
                                                Rc::new(RefCell::new(Value::String(label)))
                                            })
                                            .collect();
                                        pmap.insert(
                                            "text-value".to_string(),
                                            Rc::new(RefCell::new(Value::String(selected))),
                                        );
                                        pmap.insert(
                                            "options".to_string(),
                                            Rc::new(RefCell::new(Value::List(option_values))),
                                        );
                                        pmap.insert(
                                            "min".to_string(),
                                            Rc::new(RefCell::new(Value::Number(0.0))),
                                        );
                                        pmap.insert(
                                            "max".to_string(),
                                            Rc::new(RefCell::new(Value::Number(
                                                (SyncDivision::ALL.len() - 1) as f64,
                                            ))),
                                        );
                                    }
                                    if param_supports_value_binding(pdesc) {
                                        insert_string_prop(
                                            &mut pmap,
                                            "value-field",
                                            bus_effect_param_value_field(
                                                bus_idx,
                                                slot_idx,
                                                param_idx,
                                                &pdesc.name,
                                            ),
                                        );
                                    }
                                }
                            }
                            if let Some(targets) = modulation_targets.get(&param_idx) {
                                insert_mod_metadata(&mut pmap, targets);
                            }
                            insert_param_ui_metadata(&mut pmap, pdesc.ui_metadata.as_ref());
                            Some(Rc::new(RefCell::new(Value::Map(pmap))))
                        })
                        .collect();

                    let source_actual =
                        selected_bus_voice_mod_source_indices(desc, slot, plock_step);
                    let mut source_sections: Vec<Rc<RefCell<Value>>> = Vec::new();
                    let mut source_names: Vec<Rc<RefCell<Value>>> = Vec::new();
                    for slot_number in 1..=sequencer::voice_modulator::SLOT_COUNT {
                        let section_name =
                            sequencer::voice_modulator::modulator_slot_label(slot_number, "");
                        let mut section_params: Vec<Rc<RefCell<Value>>> = Vec::new();
                        let mut source_param: Option<Rc<RefCell<Value>>> = None;
                        for &param_idx in &source_actual {
                            let Some(pdesc) = desc.params.get(param_idx) else {
                                continue;
                            };
                            if sequencer::voice_modulator::slot_from_param_name(&pdesc.name)
                                != Some(slot_number)
                            {
                                continue;
                            }
                            let current_val =
                                bus_slot_param_stored_value(slot, desc, param_idx, plock_step);
                            let mut pmap: HashMap<String, Rc<RefCell<Value>>> = HashMap::new();
                            pmap.insert(
                                "name".to_string(),
                                Rc::new(RefCell::new(Value::String(rename_source_param(
                                    &pdesc.name,
                                )))),
                            );
                            pmap.insert(
                                "idx".to_string(),
                                Rc::new(RefCell::new(Value::Number(param_idx as f64))),
                            );
                            pmap.insert(
                                "value".to_string(),
                                Rc::new(RefCell::new(Value::Number(
                                    pdesc.stored_to_user(current_val) as f64,
                                ))),
                            );
                            pmap.insert(
                                "min".to_string(),
                                Rc::new(RefCell::new(Value::Number(
                                    pdesc.stored_to_user(pdesc.min) as f64,
                                ))),
                            );
                            pmap.insert(
                                "max".to_string(),
                                Rc::new(RefCell::new(Value::Number(
                                    pdesc.stored_to_user(pdesc.max) as f64,
                                ))),
                            );
                            match &pdesc.kind {
                                ParamKind::Boolean => {
                                    pmap.insert(
                                        "boolean".to_string(),
                                        Rc::new(RefCell::new(Value::Bool(true))),
                                    );
                                }
                                ParamKind::Enum { labels } => {
                                    let selected = labels
                                        .get(current_val.round() as usize)
                                        .cloned()
                                        .unwrap_or_default();
                                    let option_values = labels
                                        .iter()
                                        .filter(|label| {
                                            !(is_source_param(pdesc.node_param_idx)
                                                && label.as_str() == "env")
                                        })
                                        .cloned()
                                        .map(|label| Rc::new(RefCell::new(Value::String(label))))
                                        .collect();
                                    pmap.insert(
                                        "text-value".to_string(),
                                        Rc::new(RefCell::new(Value::String(selected))),
                                    );
                                    pmap.insert(
                                        "options".to_string(),
                                        Rc::new(RefCell::new(Value::List(option_values))),
                                    );
                                }
                                ParamKind::Continuous { .. } => {}
                            }
                            insert_string_prop(
                                &mut pmap,
                                "value-field",
                                bus_effect_param_value_field(
                                    bus_idx,
                                    slot_idx,
                                    param_idx,
                                    &pdesc.name,
                                ),
                            );
                            let param_value = Rc::new(RefCell::new(Value::Map(pmap)));
                            if sequencer::voice_modulator::source_type_name_from_param_name(
                                &pdesc.name,
                            ) == Some("source")
                            {
                                source_param = Some(param_value);
                            } else {
                                section_params.push(param_value);
                            }
                        }
                        source_names
                            .push(Rc::new(RefCell::new(Value::String(section_name.clone()))));
                        let mut section_map: HashMap<String, Rc<RefCell<Value>>> = HashMap::new();
                        section_map.insert(
                            "name".to_string(),
                            Rc::new(RefCell::new(Value::String(section_name))),
                        );
                        section_map.insert(
                            "slot".to_string(),
                            Rc::new(RefCell::new(Value::Number(slot_number as f64))),
                        );
                        if let Some(source_param) = source_param {
                            section_map.insert("source-param".to_string(), source_param);
                        }
                        section_map.insert(
                            "params".to_string(),
                            Rc::new(RefCell::new(Value::List(section_params))),
                        );
                        source_sections.push(Rc::new(RefCell::new(Value::Map(section_map))));
                    }

                    slot_map.insert(
                        "params".to_string(),
                        Rc::new(RefCell::new(Value::List(params))),
                    );
                    slot_map.insert(
                        "modulators".to_string(),
                        Rc::new(RefCell::new(Value::List(
                            desc.instrument_modulators
                                .iter()
                                .map(|modulator| {
                                    let mut map: HashMap<String, Rc<RefCell<Value>>> =
                                        HashMap::new();
                                    map.insert(
                                        "slot".to_string(),
                                        Rc::new(RefCell::new(Value::Number(modulator.slot as f64))),
                                    );
                                    map.insert(
                                        "label".to_string(),
                                        Rc::new(RefCell::new(Value::String(
                                            modulator.label.clone(),
                                        ))),
                                    );
                                    Rc::new(RefCell::new(Value::Map(map)))
                                })
                                .collect(),
                        ))),
                    );
                    slot_map.insert(
                        "source-names".to_string(),
                        Rc::new(RefCell::new(Value::List(source_names))),
                    );
                    slot_map.insert(
                        "sources".to_string(),
                        Rc::new(RefCell::new(Value::List(source_sections))),
                    );
                    Rc::new(RefCell::new(Value::Map(slot_map)))
                })
                .collect();
            Rc::new(RefCell::new(Value::List(slots)))
        })
        .collect();

    Value::List(buses)
}

pub(crate) fn build_midi_effects_value(
    state: &Arc<SequencerState>,
    track: usize,
    selected: &Arc<Mutex<HashSet<usize>>>,
) -> Value {
    use sequencer::effects::{EffectDescriptor, ParamKind};
    use std::collections::HashMap;

    let descriptors = sequencer::lisp_host::load_midi_fx_descriptors();
    let descriptor_for = |name: &str| -> Option<EffectDescriptor> {
        descriptors
            .iter()
            .find(|desc| desc.name.eq_ignore_ascii_case(name))
            .cloned()
    };
    let Some(track_params) = state.pattern.track_params.get(track) else {
        return Value::List(vec![]);
    };
    let chain = track_params.midi_fx_chain();
    let sel = selected.lock().unwrap();
    let plock_step = sel.iter().copied().min();

    let slots: Vec<Rc<RefCell<Value>>> = chain
        .iter()
        .enumerate()
        .filter_map(|(slot_idx, name)| {
            let desc = descriptor_for(name)?;
            let slot = state
                .pattern
                .midi_fx_slots
                .get(track)
                .and_then(|slots| slots.get(slot_idx));
            let mut slot_map: HashMap<String, Rc<RefCell<Value>>> = HashMap::new();
            slot_map.insert(
                "name".to_string(),
                Rc::new(RefCell::new(Value::String(desc.name.clone()))),
            );
            slot_map.insert(
                "slot-idx".to_string(),
                Rc::new(RefCell::new(Value::Number(slot_idx as f64))),
            );
            slot_map.insert(
                "midi-fx".to_string(),
                Rc::new(RefCell::new(Value::Bool(true))),
            );

            let params: Vec<Rc<RefCell<Value>>> = desc
                .params
                .iter()
                .enumerate()
                .map(|(param_idx, pdesc)| {
                    let default_val = slot
                        .map(|s| {
                            if param_idx < s.num_params.load(Ordering::Relaxed) as usize {
                                s.defaults.get(param_idx)
                            } else {
                                pdesc.default
                            }
                        })
                        .unwrap_or(pdesc.default);
                    let current_val = plock_step
                        .and_then(|step| slot.and_then(|s| s.plocks.get(step, param_idx)))
                        .unwrap_or(default_val);
                    let mut pmap: HashMap<String, Rc<RefCell<Value>>> = HashMap::new();
                    pmap.insert(
                        "name".to_string(),
                        Rc::new(RefCell::new(Value::String(pdesc.name.clone()))),
                    );
                    pmap.insert(
                        "idx".to_string(),
                        Rc::new(RefCell::new(Value::Number(param_idx as f64))),
                    );
                    pmap.insert(
                        "value".to_string(),
                        Rc::new(RefCell::new(Value::Number(current_val as f64))),
                    );
                    pmap.insert(
                        "min".to_string(),
                        Rc::new(RefCell::new(Value::Number(pdesc.min as f64))),
                    );
                    pmap.insert(
                        "max".to_string(),
                        Rc::new(RefCell::new(Value::Number(pdesc.max as f64))),
                    );
                    match &pdesc.kind {
                        ParamKind::Boolean => {
                            pmap.insert(
                                "boolean".to_string(),
                                Rc::new(RefCell::new(Value::Bool(true))),
                            );
                            if param_supports_value_binding(pdesc) {
                                insert_string_prop(
                                    &mut pmap,
                                    "value-field",
                                    midi_fx_param_value_field(
                                        track,
                                        slot_idx,
                                        param_idx,
                                        &pdesc.name,
                                    ),
                                );
                            }
                        }
                        ParamKind::Enum { labels } => {
                            let selected = labels
                                .get(current_val.round() as usize)
                                .cloned()
                                .unwrap_or_default();
                            let option_values = labels
                                .iter()
                                .cloned()
                                .map(|label| Rc::new(RefCell::new(Value::String(label))))
                                .collect();
                            pmap.insert(
                                "text-value".to_string(),
                                Rc::new(RefCell::new(Value::String(selected))),
                            );
                            pmap.insert(
                                "options".to_string(),
                                Rc::new(RefCell::new(Value::List(option_values))),
                            );
                            if param_supports_value_binding(pdesc) {
                                insert_string_prop(
                                    &mut pmap,
                                    "value-field",
                                    midi_fx_param_value_field(
                                        track,
                                        slot_idx,
                                        param_idx,
                                        &pdesc.name,
                                    ),
                                );
                            }
                        }
                        ParamKind::Continuous { .. } => {
                            if param_supports_value_binding(pdesc) {
                                insert_string_prop(
                                    &mut pmap,
                                    "value-field",
                                    midi_fx_param_value_field(
                                        track,
                                        slot_idx,
                                        param_idx,
                                        &pdesc.name,
                                    ),
                                );
                            }
                        }
                    }
                    Rc::new(RefCell::new(Value::Map(pmap)))
                })
                .collect();
            slot_map.insert(
                "params".to_string(),
                Rc::new(RefCell::new(Value::List(params))),
            );
            Some(Rc::new(RefCell::new(Value::Map(slot_map))))
        })
        .collect();

    Value::List(slots)
}
