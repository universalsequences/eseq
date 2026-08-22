use super::*;

/// The panel header's sound-binding label (takes spec 16.6): the bound
/// sound's identity only — the patch name, or the binding label
/// (`Take 2 · bars 0–2` / `Pattern 2 (scene)`) when no palette entry
/// resolves. Deliberately *not* `App::sound_binding_badge`, which appends
/// the reverse referent index ("— used by Scene 1, Take 2, …"): that list
/// grows without bound and carries nothing the header needs.
fn sound_binding_label(app: &app::App, track: usize) -> Option<String> {
    let target = app.palette_target_or_binding(track, None);
    app.sound_palette_entries(track, target)
        .into_iter()
        .find(|entry| entry.is_current)
        .map(|entry| entry.name)
        .or_else(|| app.track_binding_label(track))
}

pub(crate) fn build_sampler_panel_value(
    app: &app::App,
    track: usize,
    selected: &Arc<Mutex<HashSet<usize>>>,
) -> Value {
    use std::collections::HashMap;

    fn is_mod_param(name: &str) -> bool {
        name.starts_with("mod ")
    }

    fn is_generated_host_mod_param(name: &str) -> bool {
        name.starts_with("__host_mod__")
    }

    fn is_hidden_dgen_mod_param(name: &str) -> bool {
        name.starts_with("__dgen_mod_active__")
    }

    fn is_source_param(node_param_idx: u32) -> bool {
        // u32::MAX marks host-only controls such as sampler slicing; it is not
        // a packed voice-modulator source index.
        node_param_idx != u32::MAX
            && sequencer::instruments::voice_modulator::is_source_param(node_param_idx)
    }

    fn rename_source_param(name: &str) -> String {
        sequencer::instruments::voice_modulator::source_param_display_name(name)
    }

    app.publish_sampler_analysis_runtime(track);

    let sel = selected.lock().unwrap();
    let plock_step = sel.iter().copied().min();
    let slot = &app.state.pattern.instrument_slots[track];
    let desc = app
        .graph
        .instrument_descriptors
        .get(track)
        .cloned()
        .unwrap_or_else(sequencer::effects::EffectDescriptor::builtin_sampler);

    // Look up the pre-registered SampleBuffer and pass its Value map directly
    // to the Lisp side, so the waveform widget can use it without re-loading.
    let sampler_path = app.sampler_path_for_track(track);
    let registered_sample = sampler_path.as_ref().and_then(|path| {
        match load_waveform_sample(path) {
            Ok(sample) => Some(sample),
            Err(error) => {
                eprintln!(
                    "waveform: failed to register sample {}: {error}",
                    path.display()
                );
                None
            }
        }
    });
    let buffer_value = registered_sample.as_ref().map(|s| s.to_value());
    let sample_duration = registered_sample
        .as_ref()
        .map(|s| s.duration_seconds)
        .unwrap_or(1.0);

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

    let mut modulation_targets: HashMap<usize, Vec<UiModMetadata>> = HashMap::new();
    for target in desc
        .instrument_modulation_targets
        .iter()
        .filter_map(|target| {
            let depth_desc = desc.params.get(target.depth_param_idx)?;
            let source_default = if let Some(source_param_idx) = target.source_param_idx {
                if source_param_idx < slot.num_params.load(Ordering::Relaxed) as usize {
                    slot.defaults.get(source_param_idx)
                } else {
                    desc.params.get(source_param_idx)?.default
                }
            } else {
                target.modulator_slot as f32
            };
            let depth_default =
                if target.depth_param_idx < slot.num_params.load(Ordering::Relaxed) as usize {
                    slot.defaults.get(target.depth_param_idx)
                } else {
                    depth_desc.default
                };
            let source_current = target
                .source_param_idx
                .and_then(|source_param_idx| {
                    plock_step.and_then(|step| slot.plocks.get(step, source_param_idx))
                })
                .unwrap_or(source_default);
            let depth_current = plock_step
                .and_then(|step| slot.plocks.get(step, target.depth_param_idx))
                .unwrap_or(depth_default);
            let (depth_min, depth_max) = sampler_modulation_depth_display_range(depth_desc, target);
            Some((
                target.base_param_idx,
                UiModMetadata {
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
                        instrument_param_value_field(track, source_param_idx, &source_desc.name)
                    }),
                    depth_value: depth_desc.stored_to_user(depth_current),
                    depth_value_field: Some(instrument_param_value_field(
                        track,
                        target.depth_param_idx,
                        &depth_desc.name,
                    )),
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

    let mut synth_params: Vec<Rc<RefCell<Value>>> = Vec::new();
    let mut mod_params: Vec<Rc<RefCell<Value>>> = Vec::new();
    let mut source_params_by_slot: HashMap<usize, Vec<Rc<RefCell<Value>>>> = HashMap::new();
    let mut source_type_param_by_slot: HashMap<usize, Rc<RefCell<Value>>> = HashMap::new();
    let visible_source_indices: std::collections::HashSet<usize> =
        selected_voice_mod_source_indices(&desc, slot, plock_step)
            .into_iter()
            .collect();
    let base_note = f32::from_bits(
        app.state.pattern.instrument_base_note_offsets[track].load(Ordering::Relaxed),
    );
    {
        let mut pmap: HashMap<String, Rc<RefCell<Value>>> = HashMap::new();
        pmap.insert(
            "name".to_string(),
            Rc::new(RefCell::new(Value::String("base".to_string()))),
        );
        pmap.insert(
            "control".to_string(),
            Rc::new(RefCell::new(Value::String("base-note".to_string()))),
        );
        pmap.insert(
            "value".to_string(),
            Rc::new(RefCell::new(Value::Number(base_note as f64))),
        );
        pmap.insert(
            "min".to_string(),
            Rc::new(RefCell::new(Value::Number(-48.0))),
        );
        pmap.insert(
            "max".to_string(),
            Rc::new(RefCell::new(Value::Number(48.0))),
        );
        insert_string_prop(
            &mut pmap,
            "value-field",
            instrument_base_note_value_field(track),
        );
        synth_params.push(Rc::new(RefCell::new(Value::Map(pmap))));
    }
    for (param_idx, pdesc) in desc.params.iter().enumerate() {
        let default_val = if param_idx < slot.num_params.load(Ordering::Relaxed) as usize {
            slot.defaults.get(param_idx)
        } else {
            pdesc.default
        };
        let current_val = plock_step
            .and_then(|step| slot.plocks.get(step, param_idx))
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
            "control".to_string(),
            Rc::new(RefCell::new(Value::String("param".to_string()))),
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
            sequencer::effects::ParamKind::Boolean => {
                pmap.insert(
                    "boolean".to_string(),
                    Rc::new(RefCell::new(Value::Bool(true))),
                );
                if param_supports_value_binding(pdesc) {
                    insert_string_prop(
                        &mut pmap,
                        "value-field",
                        instrument_param_value_field(track, param_idx, &pdesc.name),
                    );
                }
            }
            sequencer::effects::ParamKind::Enum { labels } => {
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
                        instrument_param_value_field(track, param_idx, &pdesc.name),
                    );
                }
            }
            sequencer::effects::ParamKind::Continuous { .. } => {
                if param_supports_value_binding(pdesc) {
                    insert_string_prop(
                        &mut pmap,
                        "value-field",
                        instrument_param_value_field(track, param_idx, &pdesc.name),
                    );
                }
            }
        }
        if is_generated_host_mod_param(&pdesc.name) || is_hidden_dgen_mod_param(&pdesc.name) {
            continue;
        }
        if is_source_param(pdesc.node_param_idx) {
            if let Some(Value::String(name)) = pmap.get("name").map(|v| v.borrow().clone()) {
                pmap.insert(
                    "name".to_string(),
                    Rc::new(RefCell::new(Value::String(rename_source_param(&name)))),
                );
            }
            if let Some(slot_number) = sequencer::instruments::voice_modulator::slot_from_param_name(&pdesc.name)
            {
                let param_value = Rc::new(RefCell::new(Value::Map(pmap)));
                if sequencer::instruments::voice_modulator::source_type_name_from_param_name(&pdesc.name)
                    == Some("source")
                {
                    source_type_param_by_slot.insert(slot_number, param_value);
                } else if visible_source_indices.contains(&param_idx) {
                    source_params_by_slot
                        .entry(slot_number)
                        .or_default()
                        .push(param_value);
                }
            }
        } else if is_mod_param(&pdesc.name) {
            if let Some(Value::String(name)) = pmap.get("name").map(|v| v.borrow().clone()) {
                pmap.insert(
                    "name".to_string(),
                    Rc::new(RefCell::new(Value::String(
                        name.strip_prefix("mod ").unwrap_or(&name).to_string(),
                    ))),
                );
            }
            mod_params.push(Rc::new(RefCell::new(Value::Map(pmap))));
        } else {
            if let Some(targets) = modulation_targets.get(&param_idx) {
                insert_mod_metadata(&mut pmap, targets);
            }
            synth_params.push(Rc::new(RefCell::new(Value::Map(pmap))));
        }
    }

    let mut source_sections: Vec<Rc<RefCell<Value>>> = Vec::new();
    let mut source_names: Vec<Rc<RefCell<Value>>> = Vec::new();
    for slot_number in 1..=sequencer::instruments::voice_modulator::SLOT_COUNT {
        let section_name = sequencer::instruments::voice_modulator::modulator_slot_label(slot_number, "");
        let params = source_params_by_slot
            .remove(&slot_number)
            .unwrap_or_default();
        let source_param = source_type_param_by_slot.remove(&slot_number);
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
            Rc::new(RefCell::new(Value::List(params))),
        );
        source_sections.push(Rc::new(RefCell::new(Value::Map(section_map))));
    }

    let mut panel_map: HashMap<String, Rc<RefCell<Value>>> = HashMap::new();
    panel_map.insert(
        "type".to_string(),
        Rc::new(RefCell::new(Value::String("sampler".to_string()))),
    );
    panel_map.insert(
        "track".to_string(),
        Rc::new(RefCell::new(Value::Number(track as f64))),
    );
    if let Some(buf_val) = buffer_value {
        panel_map.insert("buffer".to_string(), Rc::new(RefCell::new(buf_val)));
    }
    let buffer_id = app.graph.track_buffer_ids.get(track).copied().unwrap_or(-1);
    let sensitivity_idx = sequencer::instruments::sampler::SLOT_PARAM_SLICE_SENSITIVITY;
    let sensitivity_default = if sensitivity_idx < slot.num_params.load(Ordering::Relaxed) as usize {
        slot.defaults.get(sensitivity_idx)
    } else {
        desc.params[sensitivity_idx].default
    };
    let sensitivity = plock_step
        .and_then(|step| slot.plocks.get(step, sensitivity_idx))
        .unwrap_or(sensitivity_default);
    let slice_values = app
        .sample_analysis
        .cache()
        .table(buffer_id)
        .map(|table| {
            table
                .slice_starts(sensitivity)
                .map(|frame| {
                    Rc::new(RefCell::new(Value::Number(
                        frame as f64 / table.sample_rate.max(1) as f64,
                    )))
                })
                .collect()
        })
        .unwrap_or_default();
    let analysis_entry = app.sample_analysis.cache().get(buffer_id);
    let mut analysis_status = "none".to_string();
    let mut analysis_message = String::new();
    let mut onset_values: Vec<Rc<RefCell<Value>>> = Vec::new();
    if let Some(entry) = analysis_entry {
        match entry.as_ref() {
            sequencer::analysis::AnalysisEntry::Pending => {
                analysis_status = "pending".to_string();
                analysis_message = "Analyzing...".to_string();
            }
            sequencer::analysis::AnalysisEntry::Ready(result) => {
                analysis_status = "ready".to_string();
                analysis_message = format!("{:.1} BPM", result.bpm);
                panel_map.insert(
                    "analysis-bpm".to_string(),
                    Rc::new(RefCell::new(Value::Number(result.bpm as f64))),
                );
                panel_map.insert(
                    "analysis-confidence".to_string(),
                    Rc::new(RefCell::new(Value::Number(result.bpm_confidence as f64))),
                );
                if let Some(frame) = result.downbeat_frame {
                    let seconds = frame as f64 / app.graph.sample_rate.max(1) as f64;
                    panel_map.insert(
                        "downbeat-time".to_string(),
                        Rc::new(RefCell::new(Value::Number(seconds))),
                    );
                }
                onset_values = result
                    .onsets_frames
                    .iter()
                    .map(|frame| {
                        Rc::new(RefCell::new(Value::Number(
                            *frame as f64 / app.graph.sample_rate.max(1) as f64,
                        )))
                    })
                    .collect();
            }
            sequencer::analysis::AnalysisEntry::Failed(error) => {
                analysis_status = "failed".to_string();
                analysis_message = error.clone();
            }
        }
    }
    panel_map.insert(
        "analysis-status".to_string(),
        Rc::new(RefCell::new(Value::String(analysis_status))),
    );
    panel_map.insert(
        "analysis-message".to_string(),
        Rc::new(RefCell::new(Value::String(analysis_message))),
    );
    panel_map.insert(
        "onsets".to_string(),
        Rc::new(RefCell::new(Value::List(onset_values))),
    );
    panel_map.insert(
        "slices".to_string(),
        Rc::new(RefCell::new(Value::List(slice_values))),
    );
    panel_map.insert(
        "params".to_string(),
        Rc::new(RefCell::new(Value::List(synth_params.clone()))),
    );
    panel_map.insert(
        "synth".to_string(),
        Rc::new(RefCell::new(Value::List(synth_params))),
    );
    panel_map.insert(
        "mod".to_string(),
        Rc::new(RefCell::new(Value::List(mod_params))),
    );
    panel_map.insert(
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
    panel_map.insert(
        "source-names".to_string(),
        Rc::new(RefCell::new(Value::List(source_names))),
    );
    panel_map.insert(
        "sources".to_string(),
        Rc::new(RefCell::new(Value::List(source_sections))),
    );
    // Start/end as seconds for the waveform selection overlay.
    // Raw stored values are 0.0-1.0 normalized; multiply by duration.
    let start_raw = plock_step
        .and_then(|step| slot.plocks.get(step, 2))
        .unwrap_or_else(|| slot.defaults.get(2));
    let end_raw = plock_step
        .and_then(|step| slot.plocks.get(step, 3))
        .unwrap_or_else(|| slot.defaults.get(3));
    panel_map.insert(
        "start-time".to_string(),
        Rc::new(RefCell::new(Value::Number(
            (start_raw as f64) * sample_duration,
        ))),
    );
    insert_string_prop(
        &mut panel_map,
        "start-time-field",
        sampler_selection_time_field(track, "start"),
    );
    panel_map.insert(
        "end-time".to_string(),
        Rc::new(RefCell::new(Value::Number(
            (end_raw as f64) * sample_duration,
        ))),
    );
    insert_string_prop(
        &mut panel_map,
        "end-time-field",
        sampler_selection_time_field(track, "end"),
    );
    panel_map.insert(
        "duration".to_string(),
        Rc::new(RefCell::new(Value::Number(sample_duration))),
    );
    // Sound-binding badge (takes spec 16.6) — sampler tracks are the common
    // take-recording case, so they carry the badge too.
    panel_map.insert(
        "sound-binding".to_string(),
        Rc::new(RefCell::new(match sound_binding_label(app, track) {
            Some(label) => Value::String(label),
            None => Value::Nil,
        })),
    );

    Value::List(vec![Rc::new(RefCell::new(Value::Map(panel_map)))])
}

pub(crate) fn build_instrument_panel_value(
    app: &app::App,
    track: usize,
    selected: &Arc<Mutex<HashSet<usize>>>,
) -> Value {
    use std::collections::HashMap;

    if app.graph.track_instrument_types.get(track)
        == Some(&sequencer::sequencer::InstrumentType::Rack)
    {
        return build_rack_panel_value(app, track, selected);
    }
    if app.is_sampler_track(track) {
        return build_sampler_panel_value(app, track, selected);
    }
    let Some(desc) = app.graph.instrument_descriptors.get(track) else {
        return Value::List(vec![]);
    };
    if desc.params.is_empty() && desc.tensor_params.is_empty() {
        return Value::List(vec![]);
    }

    let sel = selected.lock().unwrap();
    let plock_step = sel.iter().copied().min();
    let slot = &app.state.pattern.instrument_slots[track];
    let base_note_default = f32::from_bits(
        app.state.pattern.instrument_base_note_offsets[track].load(Ordering::Relaxed),
    );
    let base_note_current = base_note_default;

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

    fn push_param(
        out: &mut Vec<Rc<RefCell<Value>>>,
        name: String,
        control: &str,
        idx: Option<usize>,
        value: f32,
        min: f32,
        max: f32,
        options: Option<&Vec<String>>,
        value_field: Option<String>,
        mod_targets: Option<&Vec<UiModMetadata>>,
        ui_metadata: Option<&sequencer::effects::ParamUiMetadata>,
        key_locks: &[(u8, f32)],
    ) {
        let is_boolean_name = name == "enabled" || name == "sync";
        let mut pmap: HashMap<String, Rc<RefCell<Value>>> = HashMap::new();
        pmap.insert(
            "name".to_string(),
            Rc::new(RefCell::new(Value::String(name))),
        );
        pmap.insert(
            "control".to_string(),
            Rc::new(RefCell::new(Value::String(control.to_string()))),
        );
        if let Some(idx) = idx {
            pmap.insert(
                "idx".to_string(),
                Rc::new(RefCell::new(Value::Number(idx as f64))),
            );
        }
        pmap.insert(
            "value".to_string(),
            Rc::new(RefCell::new(Value::Number(value as f64))),
        );
        pmap.insert(
            "min".to_string(),
            Rc::new(RefCell::new(Value::Number(min as f64))),
        );
        pmap.insert(
            "max".to_string(),
            Rc::new(RefCell::new(Value::Number(max as f64))),
        );
        if let Some(labels) = options {
            let selected = labels
                .get(value.round() as usize)
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
        if options.is_none() && is_boolean_name {
            pmap.insert(
                "boolean".to_string(),
                Rc::new(RefCell::new(Value::Bool(true))),
            );
        }
        if let Some(value_field) = value_field {
            insert_string_prop(&mut pmap, "value-field", value_field);
        }
        if !key_locks.is_empty() {
            let rows = key_locks
                .iter()
                .map(|(note, value)| {
                    let mut row = HashMap::new();
                    row.insert(
                        "note".to_string(),
                        Rc::new(RefCell::new(Value::Number(*note as f64))),
                    );
                    row.insert(
                        "value".to_string(),
                        Rc::new(RefCell::new(Value::Number(*value as f64))),
                    );
                    Rc::new(RefCell::new(Value::Map(row)))
                })
                .collect();
            pmap.insert(
                "key-locks".to_string(),
                Rc::new(RefCell::new(Value::List(rows))),
            );
        }
        if let Some(targets) = mod_targets {
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
        insert_param_ui_metadata(&mut pmap, ui_metadata);
        out.push(Rc::new(RefCell::new(Value::Map(pmap))));
    }

    fn is_mod_param(name: &str) -> bool {
        name.starts_with("mod ")
    }

    fn is_generated_host_mod_param(name: &str) -> bool {
        name.starts_with("__host_mod__")
    }

    fn is_hidden_dgen_mod_param(name: &str) -> bool {
        name.starts_with("__dgen_mod_active__")
    }

    fn is_source_param(node_param_idx: u32) -> bool {
        // u32::MAX marks host-only controls; it is not a packed
        // voice-modulator source index.
        node_param_idx != u32::MAX
            && sequencer::instruments::voice_modulator::is_source_param(node_param_idx)
    }

    fn rename_source_param(name: &str) -> String {
        sequencer::instruments::voice_modulator::source_param_display_name(name)
    }

    let source_actual = selected_voice_mod_source_indices(desc, slot, plock_step);
    let slot_num_params = slot.num_params.load(Ordering::Relaxed) as usize;
    let mut key_locks_by_param = vec![Vec::<(u8, f32)>::new(); desc.params.len()];
    for note in 0..sequencer::effects::MAX_MIDI_NOTES {
        let note = note as u8;
        if !slot.key_locks.note_has_any_lock(note, slot_num_params) {
            continue;
        }
        for (param_idx, pdesc) in desc.params.iter().enumerate().take(slot_num_params) {
            let Some(value) = slot.key_locks.get(note, param_idx) else {
                continue;
            };
            if slot.key_locks.get_id(note, param_idx) != slot.param_node_id(param_idx) {
                continue;
            }
            if let Some(rows) = key_locks_by_param.get_mut(param_idx) {
                rows.push((note, pdesc.stored_to_user(value)));
            }
        }
    }
    let key_lock_assignments = app
        .state
        .reconcile_key_lock_variant_registry_for_track(track);
    let key_lock_note_variants = key_lock_assignments
        .iter()
        .enumerate()
        .filter_map(|(note, assignment)| {
            let assignment = assignment.as_ref()?;
            let mut map = HashMap::new();
            map.insert(
                "note".to_string(),
                Rc::new(RefCell::new(Value::Number(note as f64))),
            );
            map.insert(
                "label".to_string(),
                Rc::new(RefCell::new(Value::String(assignment.label.clone()))),
            );
            map.insert(
                "count".to_string(),
                Rc::new(RefCell::new(Value::Number(assignment.param_count as f64))),
            );
            map.insert(
                "color-r".to_string(),
                Rc::new(RefCell::new(Value::Number(assignment.color[0] as f64))),
            );
            map.insert(
                "color-g".to_string(),
                Rc::new(RefCell::new(Value::Number(assignment.color[1] as f64))),
            );
            map.insert(
                "color-b".to_string(),
                Rc::new(RefCell::new(Value::Number(assignment.color[2] as f64))),
            );
            Some(Rc::new(RefCell::new(Value::Map(map))))
        })
        .collect::<Vec<_>>();
    let mut key_lock_variant_items = Vec::new();
    let mut def_map = HashMap::new();
    def_map.insert(
        "kind".to_string(),
        Rc::new(RefCell::new(Value::String("def".to_string()))),
    );
    def_map.insert(
        "label".to_string(),
        Rc::new(RefCell::new(Value::String("def".to_string()))),
    );
    def_map.insert(
        "display".to_string(),
        Rc::new(RefCell::new(Value::String("base".to_string()))),
    );
    def_map.insert(
        "count".to_string(),
        Rc::new(RefCell::new(Value::Number(0.0))),
    );
    def_map.insert(
        "color-r".to_string(),
        Rc::new(RefCell::new(Value::Number(0.545_098_07))),
    );
    def_map.insert(
        "color-g".to_string(),
        Rc::new(RefCell::new(Value::Number(0.545_098_07))),
    );
    def_map.insert(
        "color-b".to_string(),
        Rc::new(RefCell::new(Value::Number(0.588_235_3))),
    );
    key_lock_variant_items.push(Rc::new(RefCell::new(Value::Map(def_map))));
    for entry in app.state.key_lock_variant_registry_snapshot(track).entries {
        let mut map = HashMap::new();
        map.insert(
            "kind".to_string(),
            Rc::new(RefCell::new(Value::String("variant".to_string()))),
        );
        map.insert(
            "label".to_string(),
            Rc::new(RefCell::new(Value::String(entry.label.clone()))),
        );
        map.insert(
            "display".to_string(),
            Rc::new(RefCell::new(Value::String(
                entry.name.clone().unwrap_or_else(|| entry.label.clone()),
            ))),
        );
        map.insert(
            "count".to_string(),
            Rc::new(RefCell::new(Value::Number(entry.key.param_count() as f64))),
        );
        map.insert(
            "color-r".to_string(),
            Rc::new(RefCell::new(Value::Number(entry.color[0] as f64))),
        );
        map.insert(
            "color-g".to_string(),
            Rc::new(RefCell::new(Value::Number(entry.color[1] as f64))),
        );
        map.insert(
            "color-b".to_string(),
            Rc::new(RefCell::new(Value::Number(entry.color[2] as f64))),
        );
        key_lock_variant_items.push(Rc::new(RefCell::new(Value::Map(map))));
    }

    let mut synth_params: Vec<Rc<RefCell<Value>>> = Vec::new();
    let mut mod_params: Vec<Rc<RefCell<Value>>> = Vec::new();
    let mut modulation_targets: HashMap<usize, Vec<UiModMetadata>> = HashMap::new();
    for target in desc
        .instrument_modulation_targets
        .iter()
        .filter_map(|target| {
            let depth_desc = desc.params.get(target.depth_param_idx)?;
            let source_default = if let Some(source_param_idx) = target.source_param_idx {
                if source_param_idx < slot.num_params.load(Ordering::Relaxed) as usize {
                    slot.defaults.get(source_param_idx)
                } else {
                    desc.params.get(source_param_idx)?.default
                }
            } else {
                target.modulator_slot as f32
            };
            let depth_default =
                if target.depth_param_idx < slot.num_params.load(Ordering::Relaxed) as usize {
                    slot.defaults.get(target.depth_param_idx)
                } else {
                    depth_desc.default
                };
            let source_current = target
                .source_param_idx
                .and_then(|source_param_idx| {
                    plock_step.and_then(|step| slot.plocks.get(step, source_param_idx))
                })
                .unwrap_or(source_default);
            let depth_current = plock_step
                .and_then(|step| slot.plocks.get(step, target.depth_param_idx))
                .unwrap_or(depth_default);
            let (depth_min, depth_max) = instrument_modulation_depth_display_range(target);
            Some((
                target.base_param_idx,
                UiModMetadata {
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
                        instrument_param_value_field(track, source_param_idx, &source_desc.name)
                    }),
                    depth_value: depth_desc.stored_to_user(depth_current),
                    depth_value_field: Some(instrument_param_value_field(
                        track,
                        target.depth_param_idx,
                        &depth_desc.name,
                    )),
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
    push_param(
        &mut synth_params,
        "base_note".to_string(),
        "base-note",
        None,
        base_note_current,
        -48.0,
        48.0,
        None,
        Some(instrument_base_note_value_field(track)),
        None,
        None,
        &[],
    );

    for (param_idx, pdesc) in desc.params.iter().enumerate() {
        let default_val = if param_idx < slot.num_params.load(Ordering::Relaxed) as usize {
            slot.defaults.get(param_idx)
        } else {
            pdesc.default
        };
        let current_val = plock_step
            .and_then(|step| slot.plocks.get(step, param_idx))
            .unwrap_or(default_val);
        let options = match &pdesc.kind {
            sequencer::effects::ParamKind::Enum { labels } => Some(labels),
            _ => None,
        };
        if is_source_param(pdesc.node_param_idx)
            || is_generated_host_mod_param(&pdesc.name)
            || is_hidden_dgen_mod_param(&pdesc.name)
        {
            continue;
        }
        if is_mod_param(&pdesc.name) {
            let mod_name = pdesc
                .name
                .strip_prefix("mod ")
                .unwrap_or(&pdesc.name)
                .to_string();
            push_param(
                &mut mod_params,
                mod_name,
                "param",
                Some(param_idx),
                pdesc.stored_to_user(current_val),
                pdesc.stored_to_user(pdesc.min),
                pdesc.stored_to_user(pdesc.max),
                options,
                Some(instrument_param_value_field(track, param_idx, &pdesc.name)),
                None,
                None,
                key_locks_by_param
                    .get(param_idx)
                    .map(Vec::as_slice)
                    .unwrap_or(&[]),
            );
        } else {
            push_param(
                &mut synth_params,
                pdesc.name.clone(),
                "param",
                Some(param_idx),
                pdesc.stored_to_user(current_val),
                pdesc.stored_to_user(pdesc.min),
                pdesc.stored_to_user(pdesc.max),
                options,
                Some(instrument_param_value_field(track, param_idx, &pdesc.name)),
                modulation_targets.get(&param_idx),
                pdesc.ui_metadata.as_ref(),
                key_locks_by_param
                    .get(param_idx)
                    .map(Vec::as_slice)
                    .unwrap_or(&[]),
            );
        }
    }

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
            let default_val = if param_idx < slot.num_params.load(Ordering::Relaxed) as usize {
                slot.defaults.get(param_idx)
            } else {
                pdesc.default
            };
            let current_val = plock_step
                .and_then(|step| slot.plocks.get(step, param_idx))
                .unwrap_or(default_val);
            let options = match &pdesc.kind {
                sequencer::effects::ParamKind::Enum { labels } => Some(labels),
                _ => None,
            };
            push_param(
                &mut params,
                rename_source_param(&pdesc.name),
                "param",
                Some(param_idx),
                pdesc.stored_to_user(current_val),
                pdesc.stored_to_user(pdesc.min),
                pdesc.stored_to_user(pdesc.max),
                options,
                Some(instrument_param_value_field(track, param_idx, &pdesc.name)),
                None,
                None,
                key_locks_by_param
                    .get(param_idx)
                    .map(Vec::as_slice)
                    .unwrap_or(&[]),
            );
            if sequencer::instruments::voice_modulator::source_type_name_from_param_name(&pdesc.name)
                == Some("source")
            {
                source_param = params.pop();
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
            Rc::new(RefCell::new(Value::List(params))),
        );
        source_sections.push(Rc::new(RefCell::new(Value::Map(section_map))));
    }

    let mut tensor_params: Vec<Rc<RefCell<Value>>> = Vec::new();
    for (tensor_idx, tensor_desc) in desc.tensor_params.iter().enumerate() {
        let values = slot
            .tensor_params
            .resolved_values(plock_step, tensor_idx)
            .unwrap_or_else(|| tensor_desc.default.clone());
        let value_list = values
            .into_iter()
            .map(|value| Rc::new(RefCell::new(Value::Number(value as f64))))
            .collect();
        let mut tensor_map: HashMap<String, Rc<RefCell<Value>>> = HashMap::new();
        tensor_map.insert(
            "idx".to_string(),
            Rc::new(RefCell::new(Value::Number(tensor_idx as f64))),
        );
        tensor_map.insert(
            "name".to_string(),
            Rc::new(RefCell::new(Value::String(tensor_desc.name.clone()))),
        );
        tensor_map.insert(
            "rows".to_string(),
            Rc::new(RefCell::new(Value::Number(tensor_desc.rows() as f64))),
        );
        tensor_map.insert(
            "cols".to_string(),
            Rc::new(RefCell::new(Value::Number(tensor_desc.cols() as f64))),
        );
        tensor_map.insert(
            "min".to_string(),
            Rc::new(RefCell::new(Value::Number(tensor_desc.min as f64))),
        );
        tensor_map.insert(
            "max".to_string(),
            Rc::new(RefCell::new(Value::Number(tensor_desc.max as f64))),
        );
        tensor_map.insert(
            "value-field".to_string(),
            Rc::new(RefCell::new(Value::String(instrument_tensor_value_field(
                track,
                tensor_idx,
                &tensor_desc.name,
            )))),
        );
        tensor_map.insert(
            "value".to_string(),
            Rc::new(RefCell::new(Value::List(value_list))),
        );
        tensor_params.push(Rc::new(RefCell::new(Value::Map(tensor_map))));
    }

    let mut panel_map: HashMap<String, Rc<RefCell<Value>>> = HashMap::new();
    let instrument_type = app
        .graph
        .track_instrument_types
        .get(track)
        .copied()
        .unwrap_or(sequencer::sequencer::InstrumentType::Custom);
    let instrument_name = current_custom_instrument_name(app, track).unwrap_or_else(|| {
        if instrument_type == sequencer::sequencer::InstrumentType::Modulator {
            "Modulator".to_string()
        } else {
            "Instrument".to_string()
        }
    });
    let instrument_type_name = match instrument_type {
        sequencer::sequencer::InstrumentType::Sampler => "sampler",
        sequencer::sequencer::InstrumentType::Custom => "custom",
        sequencer::sequencer::InstrumentType::Modulator => "modulator",
        sequencer::sequencer::InstrumentType::Rack => "rack",
    };
    panel_map.insert(
        "type".to_string(),
        Rc::new(RefCell::new(Value::String(
            instrument_type_name.to_string(),
        ))),
    );
    panel_map.insert(
        "track".to_string(),
        Rc::new(RefCell::new(Value::Number(track as f64))),
    );
    panel_map.insert(
        "phase-field".to_string(),
        Rc::new(RefCell::new(Value::String(modulator_phase_field(track)))),
    );
    panel_map.insert(
        "level-field".to_string(),
        Rc::new(RefCell::new(Value::String(modulator_level_field(track)))),
    );
    panel_map.insert(
        "name".to_string(),
        Rc::new(RefCell::new(Value::String(instrument_name.clone()))),
    );
    panel_map.insert(
        "display-name".to_string(),
        Rc::new(RefCell::new(Value::String(instrument_display_name(
            &instrument_name,
        )))),
    );
    // Sound-binding badge (takes spec 16.6): the bound Patch's identity —
    // "Patch A". Rides the panel map rather than a per-track reactive list
    // because the FX strip is driven entirely by `inst` — a panel-scope
    // SEQ.* read breaks the *fx* buffer's evaluation.
    panel_map.insert(
        "sound-binding".to_string(),
        Rc::new(RefCell::new(match sound_binding_label(app, track) {
            Some(label) => Value::String(label),
            None => Value::Nil,
        })),
    );
    panel_map.insert(
        "synth".to_string(),
        Rc::new(RefCell::new(Value::List(synth_params))),
    );
    panel_map.insert(
        "mod".to_string(),
        Rc::new(RefCell::new(Value::List(mod_params))),
    );
    panel_map.insert(
        "tensors".to_string(),
        Rc::new(RefCell::new(Value::List(tensor_params))),
    );
    panel_map.insert(
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
    panel_map.insert(
        "source-names".to_string(),
        Rc::new(RefCell::new(Value::List(source_names))),
    );
    panel_map.insert(
        "sources".to_string(),
        Rc::new(RefCell::new(Value::List(source_sections))),
    );
    panel_map.insert(
        "key-lock-note-variants".to_string(),
        Rc::new(RefCell::new(Value::List(key_lock_note_variants))),
    );
    panel_map.insert(
        "key-lock-variants".to_string(),
        Rc::new(RefCell::new(Value::List(key_lock_variant_items))),
    );

    Value::List(vec![Rc::new(RefCell::new(Value::Map(panel_map)))])
}
