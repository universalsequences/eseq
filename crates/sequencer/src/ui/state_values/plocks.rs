use super::*;

pub(super) fn plock_entry(
    step: usize,
    target: &str,
    group: &str,
    name: &str,
    value: f32,
    default: f32,
    min: f32,
    max: f32,
    slot_idx: Option<usize>,
    param_idx: Option<usize>,
    options: Option<Vec<String>>,
) -> Rc<RefCell<Value>> {
    plock_entry_with_label(
        &format!("S{}", step + 1),
        step,
        target,
        group,
        name,
        value,
        default,
        min,
        max,
        slot_idx,
        param_idx,
        options,
        None,
        None,
        None,
    )
}

pub(super) fn rack_effect_plock_entry(
    step: usize,
    rack_slot: usize,
    effect_slot: usize,
    effect_name: &str,
    param_name: &str,
    value: f32,
    default: f32,
    min: f32,
    max: f32,
    param_idx: usize,
    options: Option<Vec<String>>,
) -> Rc<RefCell<Value>> {
    let entry = plock_entry(
        step,
        "rack-effect",
        effect_name,
        param_name,
        value,
        default,
        min,
        max,
        Some(effect_slot),
        Some(param_idx),
        options,
    );
    if let Value::Map(map) = &mut *entry.borrow_mut() {
        map.insert(
            "rack-slot".to_string(),
            Rc::new(RefCell::new(Value::Number(rack_slot as f64))),
        );
    }
    entry
}

pub(super) fn plock_entry_with_label(
    label: &str,
    step: usize,
    target: &str,
    group: &str,
    name: &str,
    value: f32,
    default: f32,
    min: f32,
    max: f32,
    slot_idx: Option<usize>,
    param_idx: Option<usize>,
    options: Option<Vec<String>>,
    source: Option<&str>,
    target_track: Option<usize>,
    network_id: Option<u64>,
) -> Rc<RefCell<Value>> {
    use std::collections::HashMap;

    let mut map: HashMap<String, Rc<RefCell<Value>>> = HashMap::new();
    map.insert(
        "label".to_string(),
        Rc::new(RefCell::new(Value::String(label.to_string()))),
    );
    map.insert(
        "step".to_string(),
        Rc::new(RefCell::new(Value::Number((step + 1) as f64))),
    );
    map.insert(
        "step-idx".to_string(),
        Rc::new(RefCell::new(Value::Number(step as f64))),
    );
    map.insert(
        "target".to_string(),
        Rc::new(RefCell::new(Value::String(target.to_string()))),
    );
    map.insert(
        "group".to_string(),
        Rc::new(RefCell::new(Value::String(group.to_string()))),
    );
    map.insert(
        "name".to_string(),
        Rc::new(RefCell::new(Value::String(name.to_string()))),
    );
    map.insert(
        "value".to_string(),
        Rc::new(RefCell::new(Value::Number(value as f64))),
    );
    map.insert(
        "default".to_string(),
        Rc::new(RefCell::new(Value::Number(default as f64))),
    );
    map.insert(
        "min".to_string(),
        Rc::new(RefCell::new(Value::Number(min as f64))),
    );
    map.insert(
        "max".to_string(),
        Rc::new(RefCell::new(Value::Number(max as f64))),
    );
    if let Some(slot_idx) = slot_idx {
        map.insert(
            "slot-idx".to_string(),
            Rc::new(RefCell::new(Value::Number(slot_idx as f64))),
        );
    }
    if let Some(param_idx) = param_idx {
        map.insert(
            "param-idx".to_string(),
            Rc::new(RefCell::new(Value::Number(param_idx as f64))),
        );
    }
    if let Some(source) = source {
        map.insert(
            "source".to_string(),
            Rc::new(RefCell::new(Value::String(source.to_string()))),
        );
        if source == "neuron" {
            map.insert(
                "neuron-idx".to_string(),
                Rc::new(RefCell::new(Value::Number(step as f64))),
            );
        }
    }
    if let Some(target_track) = target_track {
        map.insert(
            "target-track".to_string(),
            Rc::new(RefCell::new(Value::Number(target_track as f64))),
        );
    }
    if let Some(network_id) = network_id {
        map.insert(
            "network-id".to_string(),
            Rc::new(RefCell::new(Value::Number(network_id as f64))),
        );
    }
    if let Some(options) = options {
        let selected = options
            .get(value.round().max(0.0) as usize)
            .cloned()
            .unwrap_or_default();
        let default_text = options
            .get(default.round().max(0.0) as usize)
            .cloned()
            .unwrap_or_default();
        map.insert(
            "text-value".to_string(),
            Rc::new(RefCell::new(Value::String(selected))),
        );
        map.insert(
            "default-text".to_string(),
            Rc::new(RefCell::new(Value::String(default_text))),
        );
        map.insert(
            "options".to_string(),
            Rc::new(RefCell::new(Value::List(
                options
                    .into_iter()
                    .map(|label| Rc::new(RefCell::new(Value::String(label))))
                    .collect(),
            ))),
        );
    } else {
        map.insert(
            "text-value".to_string(),
            Rc::new(RefCell::new(Value::String(format!("{value:.2}")))),
        );
        map.insert(
            "default-text".to_string(),
            Rc::new(RefCell::new(Value::String(format!("{default:.2}")))),
        );
    }
    map.insert(
        "domain".to_string(),
        Rc::new(RefCell::new(Value::String(
            plock_entry_domain(target).to_string(),
        ))),
    );
    Rc::new(RefCell::new(Value::Map(map)))
}

pub(super) fn plock_entry_domain(target: &str) -> &'static str {
    match target {
        "instrument"
        | "instrument-tensor"
        | "rack-macro"
        | "rack-slot-param"
        | "rack-slot-instrument"
        | "rack-slot-instrument-tensor" => "inst",
        "effect" | "effect-tensor" | "bus-effect" | "rack-effect" => "fx",
        "neural-instrument" | "neural-effect" => "neural",
        _ => "seq",
    }
}

pub(super) fn plock_param_options(kind: &sequencer::effects::ParamKind) -> Option<Vec<String>> {
    match kind {
        sequencer::effects::ParamKind::Enum { labels } => Some(labels.clone()),
        sequencer::effects::ParamKind::Boolean => Some(vec!["off".to_string(), "on".to_string()]),
        sequencer::effects::ParamKind::Continuous { .. } => None,
    }
}

pub(super) fn preview_plock_entry(
    label: &str,
    target: &str,
    group: &str,
    name: &str,
    value: f32,
    default: f32,
    min: f32,
    max: f32,
    slot_idx: Option<usize>,
    param_idx: Option<usize>,
    options: Option<Vec<String>>,
) -> Rc<RefCell<Value>> {
    let entry = plock_entry_with_label(
        label,
        0,
        target,
        group,
        name,
        value,
        default,
        min,
        max,
        slot_idx,
        param_idx,
        options,
        Some("preview"),
        None,
        None,
    );
    {
        let mut value = entry.borrow_mut();
        if let Value::Map(map) = &mut *value {
            map.insert("preview".to_string(), value_cell(Value::Bool(true)));
            map.insert(
                "preview-label".to_string(),
                value_cell(Value::String(label.to_string())),
            );
        }
    }
    entry
}

pub(super) fn tensor_cell_label(desc: &sequencer::effects::TensorParamDescriptor, cell_idx: usize) -> String {
    let rows = desc.rows();
    let cols = desc.cols();
    if rows > 1 && cols > 0 {
        let row = cell_idx / cols;
        let col = cell_idx % cols;
        format!("{} {}:{}", desc.name, row + 1, col + 1)
    } else {
        format!("{} {}", desc.name, cell_idx + 1)
    }
}

pub(super) fn live_tensor_default_cell(
    slot: &sequencer::effects::EffectSlotState,
    desc: &sequencer::effects::TensorParamDescriptor,
    tensor_idx: usize,
    cell_idx: usize,
) -> f32 {
    slot.tensor_params
        .default_values(tensor_idx)
        .and_then(|values| values.get(cell_idx).copied())
        .or_else(|| desc.default.get(cell_idx).copied())
        .unwrap_or_default()
}

pub(super) fn snapshot_tensor_default_cell(
    slot: &sequencer::effects::EffectSlotSnapshot,
    desc: &sequencer::effects::TensorParamDescriptor,
    tensor_idx: usize,
    cell_idx: usize,
) -> f32 {
    slot.tensor_default_values(tensor_idx)
        .and_then(|values| values.get(cell_idx).copied())
        .or_else(|| desc.default.get(cell_idx).copied())
        .unwrap_or_default()
}

pub(super) fn rack_slot_param_by_index(index: usize) -> Option<sequencer::sequencer::RackSlotParam> {
    sequencer::sequencer::RackSlotParam::ALL
        .iter()
        .copied()
        .find(|param| param.index() == index)
}

pub(super) fn rack_slot_param_bounds(param: sequencer::sequencer::RackSlotParam) -> (f32, f32) {
    match param {
        sequencer::sequencer::RackSlotParam::BaseNote => (-48.0, 48.0),
        sequencer::sequencer::RackSlotParam::Gain => (0.0, 2.0),
        sequencer::sequencer::RackSlotParam::Pan => (-1.0, 1.0),
        sequencer::sequencer::RackSlotParam::MaxPolyphony => (
            1.0,
            sequencer::sequencer::RackSlotParam::MaxPolyphony.clamp(f32::MAX),
        ),
        sequencer::sequencer::RackSlotParam::Mute | sequencer::sequencer::RackSlotParam::Solo => {
            (0.0, 1.0)
        }
    }
}

pub(super) fn rack_slot_param_options(param: sequencer::sequencer::RackSlotParam) -> Option<Vec<String>> {
    match param {
        sequencer::sequencer::RackSlotParam::Mute | sequencer::sequencer::RackSlotParam::Solo => {
            Some(vec!["off".to_string(), "on".to_string()])
        }
        _ => None,
    }
}

pub(super) fn build_track_plock_preview_row_for_variant_entry(
    app: &app::App,
    state: &Arc<SequencerState>,
    track: usize,
    label: &str,
    entry: &sequencer::plock_variants::PlockVariantEntry,
) -> Option<Rc<RefCell<Value>>> {
    let stored_value = f32::from_bits(entry.value_bits);
    match entry.domain {
        sequencer::plock_variants::PlockVariantDomain::TrackTimebase => {
            let default = state.pattern.track_params.get(track)?.get_timebase() as u32 as f32;
            Some(preview_plock_entry(
                label,
                "timebase",
                "track",
                "timebase",
                stored_value,
                default,
                0.0,
                (Timebase::ALL.len() - 1) as f32,
                None,
                None,
                Some(
                    Timebase::LABELS
                        .iter()
                        .map(|label| label.to_string())
                        .collect(),
                ),
            ))
        }
        sequencer::plock_variants::PlockVariantDomain::TrackSwing => {
            let default = state.pattern.track_params.get(track)?.get_swing();
            Some(preview_plock_entry(
                label,
                "swing",
                "track",
                "swing",
                stored_value,
                default,
                50.0,
                75.0,
                None,
                None,
                None,
            ))
        }
        sequencer::plock_variants::PlockVariantDomain::TrackSwingResolution => {
            let default = state
                .pattern
                .track_params
                .get(track)?
                .get_swing_resolution() as u32 as f32;
            Some(preview_plock_entry(
                label,
                "swing-resolution",
                "track",
                "swing res",
                stored_value,
                default,
                0.0,
                (SwingResolution::ALL.len() - 1) as f32,
                None,
                None,
                Some(
                    SwingResolution::LABELS
                        .iter()
                        .map(|label| label.to_string())
                        .collect(),
                ),
            ))
        }
        sequencer::plock_variants::PlockVariantDomain::MidiEffect => {
            let effect_name = state
                .pattern
                .track_params
                .get(track)?
                .midi_fx_chain()
                .get(entry.slot)?
                .clone();
            let desc = sequencer::lisp_host::load_midi_fx_descriptor(&effect_name)?;
            let param = desc.params.get(entry.param)?;
            let slot = state.pattern.midi_fx_slots.get(track)?.get(entry.slot)?;
            Some(preview_plock_entry(
                label,
                "midi-fx",
                &desc.name,
                &param.name,
                stored_value,
                slot.defaults.get(entry.param),
                param.min,
                param.max,
                Some(entry.slot),
                Some(entry.param),
                plock_param_options(&param.kind),
            ))
        }
        sequencer::plock_variants::PlockVariantDomain::MidiEffectTensor => {
            let cell_idx = entry.cell?;
            let effect_name = state
                .pattern
                .track_params
                .get(track)?
                .midi_fx_chain()
                .get(entry.slot)?
                .clone();
            let desc = sequencer::lisp_host::load_midi_fx_descriptor(&effect_name)?;
            let tensor = desc.tensor_params.get(entry.param)?;
            let slot = state.pattern.midi_fx_slots.get(track)?.get(entry.slot)?;
            Some(preview_plock_entry(
                label,
                "midi-fx-tensor",
                &desc.name,
                &tensor_cell_label(tensor, cell_idx),
                stored_value,
                live_tensor_default_cell(slot, tensor, entry.param, cell_idx),
                tensor.min,
                tensor.max,
                Some(entry.slot),
                Some(entry.param),
                None,
            ))
        }
        sequencer::plock_variants::PlockVariantDomain::Instrument => {
            let desc = app.graph.instrument_descriptors.get(track)?;
            let param = desc.params.get(entry.param)?;
            let slot = state.pattern.instrument_slots.get(track)?;
            Some(preview_plock_entry(
                label,
                "instrument",
                "inst",
                &param.name,
                param.stored_to_user(stored_value),
                param.stored_to_user(slot.defaults.get(entry.param)),
                param.stored_to_user(param.min),
                param.stored_to_user(param.max),
                None,
                Some(entry.param),
                plock_param_options(&param.kind),
            ))
        }
        sequencer::plock_variants::PlockVariantDomain::InstrumentTensor => {
            let cell_idx = entry.cell?;
            let desc = app.graph.instrument_descriptors.get(track)?;
            let tensor = desc.tensor_params.get(entry.param)?;
            let slot = state.pattern.instrument_slots.get(track)?;
            Some(preview_plock_entry(
                label,
                "instrument-tensor",
                "inst",
                &tensor_cell_label(tensor, cell_idx),
                stored_value,
                live_tensor_default_cell(slot, tensor, entry.param, cell_idx),
                tensor.min,
                tensor.max,
                None,
                Some(entry.param),
                None,
            ))
        }
        sequencer::plock_variants::PlockVariantDomain::Effect => {
            let desc = app
                .graph
                .effect_descriptors
                .get(track)
                .and_then(|descs| descs.get(entry.slot))?;
            let param = desc.params.get(entry.param)?;
            let slot = state
                .pattern
                .effect_chains
                .get(track)
                .and_then(|chain| chain.get(entry.slot))?;
            Some(preview_plock_entry(
                label,
                "effect",
                &desc.name,
                &param.name,
                stored_value,
                slot.defaults.get(entry.param),
                param.min,
                param.max,
                Some(entry.slot),
                Some(entry.param),
                plock_param_options(&param.kind),
            ))
        }
        sequencer::plock_variants::PlockVariantDomain::EffectTensor => {
            let cell_idx = entry.cell?;
            let desc = app
                .graph
                .effect_descriptors
                .get(track)
                .and_then(|descs| descs.get(entry.slot))?;
            let tensor = desc.tensor_params.get(entry.param)?;
            let slot = state
                .pattern
                .effect_chains
                .get(track)
                .and_then(|chain| chain.get(entry.slot))?;
            Some(preview_plock_entry(
                label,
                "effect-tensor",
                &desc.name,
                &tensor_cell_label(tensor, cell_idx),
                stored_value,
                live_tensor_default_cell(slot, tensor, entry.param, cell_idx),
                tensor.min,
                tensor.max,
                Some(entry.slot),
                Some(entry.param),
                None,
            ))
        }
        sequencer::plock_variants::PlockVariantDomain::RackMacro => {
            let rack = state
                .pattern
                .rack_tracks
                .lock()
                .unwrap()
                .get(track)
                .cloned()
                .flatten()?;
            let rack_macro = rack.macros.get(entry.param)?;
            Some(preview_plock_entry(
                label,
                "rack-macro",
                "rack",
                &rack_macro.name,
                stored_value,
                rack_macro.value,
                0.0,
                1.0,
                None,
                Some(entry.param),
                None,
            ))
        }
        sequencer::plock_variants::PlockVariantDomain::RackSlotParam => {
            let param = rack_slot_param_by_index(entry.param)?;
            let rack = state
                .pattern
                .rack_tracks
                .lock()
                .unwrap()
                .get(track)
                .cloned()
                .flatten()?;
            let slot = rack.slots.get(entry.slot)?;
            let (min, max) = rack_slot_param_bounds(param);
            Some(preview_plock_entry(
                label,
                "rack-slot-param",
                "rack",
                param.name(),
                param.clamp(stored_value),
                slot.param_default(param),
                min,
                max,
                Some(entry.slot),
                Some(entry.param),
                rack_slot_param_options(param),
            ))
        }
        sequencer::plock_variants::PlockVariantDomain::RackSlotInstrument => {
            let rack = state
                .pattern
                .rack_tracks
                .lock()
                .unwrap()
                .get(track)
                .cloned()
                .flatten()?;
            let slot = rack.slots.get(entry.slot)?;
            let desc = app.rack_slot_instrument_descriptor(slot)?;
            let param = desc.params.get(entry.param)?;
            Some(preview_plock_entry(
                label,
                "rack-slot-instrument",
                "rack",
                &param.name,
                param.stored_to_user(stored_value),
                param.stored_to_user(
                    slot.instrument_slot
                        .defaults
                        .get(entry.param)
                        .copied()
                        .unwrap_or(param.default),
                ),
                param.stored_to_user(param.min),
                param.stored_to_user(param.max),
                Some(entry.slot),
                Some(entry.param),
                plock_param_options(&param.kind),
            ))
        }
        sequencer::plock_variants::PlockVariantDomain::RackSlotInstrumentTensor => {
            let cell_idx = entry.cell?;
            let rack = state
                .pattern
                .rack_tracks
                .lock()
                .unwrap()
                .get(track)
                .cloned()
                .flatten()?;
            let slot = rack.slots.get(entry.slot)?;
            let desc = app.rack_slot_instrument_descriptor(slot)?;
            let tensor = desc.tensor_params.get(entry.param)?;
            Some(preview_plock_entry(
                label,
                "rack-slot-instrument-tensor",
                "rack",
                &tensor_cell_label(tensor, cell_idx),
                stored_value,
                snapshot_tensor_default_cell(&slot.instrument_slot, tensor, entry.param, cell_idx),
                tensor.min,
                tensor.max,
                Some(entry.slot),
                Some(entry.param),
                None,
            ))
        }
        sequencer::plock_variants::PlockVariantDomain::InstrumentKeyLock => None,
    }
}

pub(crate) fn build_track_plocks_value_for_variant_label(
    app: &app::App,
    state: &Arc<SequencerState>,
    track: usize,
    label: &str,
) -> Value {
    if label == "def" {
        return Value::List(vec![]);
    }
    let Some(assignment) = state
        .plock_variant_registry_snapshot(track)
        .assignment_for_label(label)
    else {
        return Value::List(vec![]);
    };
    let items = assignment
        .key
        .entries
        .iter()
        .filter_map(|entry| {
            build_track_plock_preview_row_for_variant_entry(
                app,
                state,
                track,
                &assignment.label,
                entry,
            )
        })
        .collect();
    Value::List(items)
}

pub(crate) fn build_track_plocks_value_with_neural_selection(
    app: &app::App,
    state: &Arc<SequencerState>,
    track: usize,
    selected: &Arc<Mutex<HashSet<usize>>>,
    selected_neural_neurons: Option<
        &std::collections::BTreeSet<sequencer::lisp_host::SelectedNeuralNeuron>,
    >,
) -> Value {
    let Some(selection) = selected_neural_neurons else {
        return build_track_plocks_value(app, state, track, selected);
    };
    let current_pattern = state.current_scene_index();
    if !selection
        .iter()
        .any(|selected| selected.pattern_idx == current_pattern)
    {
        return build_track_plocks_value(app, state, track, selected);
    }
    build_selected_neural_plocks_value(app, state, selection)
}

pub(super) fn build_selected_neural_plocks_value(
    app: &app::App,
    state: &Arc<SequencerState>,
    selection: &std::collections::BTreeSet<sequencer::lisp_host::SelectedNeuralNeuron>,
) -> Value {
    use sequencer::effects::ParamKind;

    let current_pattern = state.current_scene_index();
    let networks = state.current_neural_networks();
    let mut items = Vec::new();

    for selected in selection
        .iter()
        .filter(|selected| selected.pattern_idx == current_pattern)
    {
        let Some(network) = networks
            .iter()
            .find(|network| network.id == selected.network_id)
        else {
            continue;
        };
        let Some(neuron) = network.neurons.get(selected.neuron_idx) else {
            continue;
        };
        let label = format!("N{}", selected.neuron_idx + 1);

        for override_param in &neuron.output_overrides.instrument {
            let target_track = override_param.target_track;
            let Some(desc) = app.graph.instrument_descriptors.get(target_track) else {
                continue;
            };
            let Some(param) = desc.params.get(override_param.param_index) else {
                continue;
            };
            let Some(slot) = state.pattern.instrument_slots.get(target_track) else {
                continue;
            };
            if slot.param_node_id(override_param.param_index) != Some(override_param.param_id) {
                continue;
            }
            let options = match &param.kind {
                ParamKind::Enum { labels } => Some(labels.clone()),
                ParamKind::Boolean => Some(vec!["off".to_string(), "on".to_string()]),
                ParamKind::Continuous { .. } => None,
            };
            items.push(plock_entry_with_label(
                &label,
                selected.neuron_idx,
                "neural-instrument",
                &format!("T{} inst", target_track + 1),
                &param.name,
                param.stored_to_user(override_param.value),
                param.stored_to_user(param.default),
                param.stored_to_user(param.min),
                param.stored_to_user(param.max),
                None,
                Some(override_param.param_index),
                options,
                Some("neuron"),
                Some(target_track),
                Some(network.id),
            ));
        }

        for override_param in &neuron.output_overrides.effects {
            let target_track = override_param.target_track;
            let Some(desc) = app
                .graph
                .effect_descriptors
                .get(target_track)
                .and_then(|descs| descs.get(override_param.slot_index))
            else {
                continue;
            };
            let Some(param) = desc.params.get(override_param.param_index) else {
                continue;
            };
            let Some(slot) = state
                .pattern
                .effect_chains
                .get(target_track)
                .and_then(|chain| chain.get(override_param.slot_index))
            else {
                continue;
            };
            if slot.param_node_id(override_param.param_index) != Some(override_param.param_id) {
                continue;
            }
            let options = match &param.kind {
                ParamKind::Enum { labels } => Some(labels.clone()),
                ParamKind::Boolean => Some(vec!["off".to_string(), "on".to_string()]),
                ParamKind::Continuous { .. } => None,
            };
            items.push(plock_entry_with_label(
                &label,
                selected.neuron_idx,
                "neural-effect",
                &format!("T{} {}", target_track + 1, desc.name),
                &param.name,
                override_param.value,
                param.default,
                param.min,
                param.max,
                Some(override_param.slot_index),
                Some(override_param.param_index),
                options,
                Some("neuron"),
                Some(target_track),
                Some(network.id),
            ));
        }
    }

    Value::List(items)
}

/// Builds the `SEQ.track-plocks` rows for the selected step.
///
/// INVARIANT — one row per projected key. The `*plock-sync*` effect buffer
/// (`ui/effects/param-controls.lisp`) reduces these rows into per-param `SEQV`
/// scalars keyed by `(target, slot-idx, rack-slot, param-idx)` — or by
/// `target` alone for the rows that carry no param index (timebase, swing,
/// swing resolution). It writes `<key>-on` / `<key>-def` once per row and then
/// clears the keys that dropped out of the list, so two rows collapsing onto
/// the same key would make a control's displayed default depend on row order
/// and could leave a stale `-on` after one of them disappears. Every row
/// emitted below must therefore be unique in that tuple: each track-level lock
/// appears at most once, and the step-param / instrument / effect / rack-macro
/// / rack-effect rows are each emitted once per distinct parameter slot.
pub(crate) fn build_track_plocks_value(
    app: &app::App,
    state: &Arc<SequencerState>,
    track: usize,
    selected: &Arc<Mutex<HashSet<usize>>>,
) -> Value {
    use sequencer::effects::ParamKind;

    let Some(step) = selected_plock_step(selected) else {
        return Value::List(vec![]);
    };
    let mut items = Vec::new();

    let tp = &state.pattern.track_params[track];
    if let Some(timebase) = state.pattern.timebase_plocks[track].get(step) {
        items.push(plock_entry(
            step,
            "timebase",
            "track",
            "timebase",
            timebase as u32 as f32,
            tp.get_timebase() as u32 as f32,
            0.0,
            (Timebase::ALL.len() - 1) as f32,
            None,
            None,
            Some(
                Timebase::LABELS
                    .iter()
                    .map(|label| label.to_string())
                    .collect(),
            ),
        ));
    }
    if let Some(swing) = state.pattern.swing_plocks[track].get(step) {
        items.push(plock_entry(
            step,
            "swing",
            "track",
            "swing",
            swing,
            tp.get_swing(),
            50.0,
            75.0,
            None,
            None,
            None,
        ));
    }
    if let Some(resolution) = state.pattern.swing_resolution_plocks[track].get(step) {
        items.push(plock_entry(
            step,
            "swing-resolution",
            "track",
            "swing res",
            resolution as u32 as f32,
            tp.get_swing_resolution() as u32 as f32,
            0.0,
            (SwingResolution::ALL.len() - 1) as f32,
            None,
            None,
            Some(
                SwingResolution::LABELS
                    .iter()
                    .map(|label| label.to_string())
                    .collect(),
            ),
        ));
    }
    for param in StepParam::ALL {
        let value = state.pattern.step_data[track].get(step, param);
        if value.to_bits() == param.default_value().to_bits() {
            continue;
        }
        items.push(plock_entry(
            step,
            "step-param",
            "per step",
            param.short_label(),
            value,
            param.default_value(),
            param.min(),
            param.max(),
            None,
            Some(param.index()),
            None,
        ));
    }

    if let Some(desc) = app.graph.instrument_descriptors.get(track) {
        let slot = &state.pattern.instrument_slots[track];
        for (param_idx, param) in desc.params.iter().enumerate() {
            if let Some(value) = slot.plocks.get(step, param_idx) {
                let options = match &param.kind {
                    ParamKind::Enum { labels } => Some(labels.clone()),
                    ParamKind::Boolean => Some(vec!["off".to_string(), "on".to_string()]),
                    ParamKind::Continuous { .. } => None,
                };
                let continuous = options.is_none();
                let entry = plock_entry(
                    step,
                    "instrument",
                    "inst",
                    &param.name,
                    param.stored_to_user(value),
                    param.stored_to_user(slot.defaults.get(param_idx)),
                    param.stored_to_user(param.min),
                    param.stored_to_user(param.max),
                    None,
                    Some(param_idx),
                    options,
                );
                if continuous {
                    // Bind the LOCK readout to the per-param SEQV field the
                    // authoring syncs already maintain (it carries the
                    // displayed step's p-lock value). With the value bound,
                    // a knob drag repaints this row without republishing
                    // SEQ.track-plocks — which would rerun the whole *step*
                    // panel on every mouse move. `fx-plock-row-value` prefers
                    // :value-field and falls back to :value.
                    if let Value::Map(map) = &mut *entry.borrow_mut() {
                        map.insert(
                            "value-field".to_string(),
                            value_cell(Value::String(instrument_param_value_field(
                                track,
                                param_idx,
                                &param.name,
                            ))),
                        );
                    }
                }
                items.push(entry);
            }
        }
    }

    if let Some(descs) = app.graph.effect_descriptors.get(track) {
        for (slot_idx, desc) in descs.iter().enumerate() {
            let Some(slot) = state.pattern.effect_chains[track].get(slot_idx) else {
                continue;
            };
            for (param_idx, param) in desc.params.iter().enumerate() {
                if let Some(value) = slot.plocks.get(step, param_idx) {
                    let options = match &param.kind {
                        ParamKind::Enum { labels } => Some(labels.clone()),
                        ParamKind::Boolean => Some(vec!["off".to_string(), "on".to_string()]),
                        ParamKind::Continuous { .. } => None,
                    };
                    items.push(plock_entry(
                        step,
                        "effect",
                        &desc.name,
                        &param.name,
                        value,
                        slot.defaults.get(param_idx),
                        param.min,
                        param.max,
                        Some(slot_idx),
                        Some(param_idx),
                        options,
                    ));
                }
            }
        }
    }
    if let Some(Some(rack)) = state.pattern.rack_tracks.lock().unwrap().get(track) {
        for rack_macro in &rack.macros {
            if let Some(value) = rack_macro.plocks.get(step).copied().flatten() {
                let entry = plock_entry(
                    step,
                    "rack-macro",
                    "rack",
                    &rack_macro.name,
                    value,
                    rack_macro.value,
                    0.0,
                    1.0,
                    None,
                    Some(rack_macro.id.index()),
                    None,
                );
                if let Value::Map(map) = &mut *entry.borrow_mut() {
                    map.insert(
                        "value-field".to_string(),
                        value_cell(Value::String(rack_macro_value_field(
                            track,
                            rack_macro.id.index(),
                        ))),
                    );
                }
                items.push(entry);
            }
        }
        for (rack_slot_idx, rack_slot) in rack.slots.iter().enumerate() {
            for (effect_slot_idx, (descriptor, effect_slot)) in rack_slot
                .effect_descriptors
                .iter()
                .zip(&rack_slot.effect_slots)
                .enumerate()
            {
                if effect_slot.node_id == 0 {
                    continue;
                }
                for (param_idx, param) in descriptor.params.iter().enumerate() {
                    let Some(value) = effect_slot
                        .plocks
                        .get(step)
                        .and_then(|row| row.get(param_idx))
                        .copied()
                        .flatten()
                    else {
                        continue;
                    };
                    items.push(rack_effect_plock_entry(
                        step,
                        rack_slot_idx,
                        effect_slot_idx,
                        &descriptor.name,
                        &param.name,
                        value,
                        effect_slot
                            .defaults
                            .get(param_idx)
                            .copied()
                            .unwrap_or(param.default),
                        param.min,
                        param.max,
                        param_idx,
                        plock_param_options(&param.kind),
                    ));
                }
            }
        }
    }

    let midi_chain = tp.midi_fx_chain();
    for (slot_idx, slot) in state.pattern.midi_fx_slots[track].iter().enumerate() {
        let Some(desc) = midi_chain
            .get(slot_idx)
            .and_then(|name| sequencer::lisp_host::load_midi_fx_descriptor(name))
        else {
            continue;
        };
        for (param_idx, param) in desc.params.iter().enumerate() {
            if let Some(value) = slot.plocks.get(step, param_idx) {
                let options = match &param.kind {
                    ParamKind::Enum { labels } => Some(labels.clone()),
                    ParamKind::Boolean => Some(vec!["off".to_string(), "on".to_string()]),
                    ParamKind::Continuous { .. } => None,
                };
                items.push(plock_entry(
                    step,
                    "midi-fx",
                    &desc.name,
                    &param.name,
                    value,
                    slot.defaults.get(param_idx),
                    param.min,
                    param.max,
                    Some(slot_idx),
                    Some(param_idx),
                    options,
                ));
            }
        }
    }

    Value::List(items)
}

pub(crate) fn build_track_plock_variants_value(
    state: &Arc<SequencerState>,
    track: usize,
    selected: &Arc<Mutex<HashSet<usize>>>,
) -> Value {
    build_track_plock_variants_value_with_preview(state, track, selected, None)
}

pub(crate) fn build_track_plock_variants_value_with_preview(
    state: &Arc<SequencerState>,
    track: usize,
    selected: &Arc<Mutex<HashSet<usize>>>,
    preview_label: Option<&str>,
) -> Value {
    let registry = state.plock_variant_registry_snapshot(track);
    let selected_step = selected_plock_step(selected);
    let current_key = selected_step.and_then(|step| {
        sequencer::plock_variants::live_track_variant_key(state.as_ref(), track, step)
    });
    let preview_label = current_key.is_none().then_some(preview_label).flatten();
    let preview_is_def = preview_label.map_or(true, |label| label == "def");

    let mut items = Vec::with_capacity(registry.entries.len() + 1);
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
        "current".to_string(),
        Rc::new(RefCell::new(Value::Bool(
            current_key.is_none() && preview_is_def,
        ))),
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
    items.push(Rc::new(RefCell::new(Value::Map(def_map))));

    for entry in registry.entries {
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
            "current".to_string(),
            Rc::new(RefCell::new(Value::Bool(
                current_key.as_ref().is_some_and(|key| key == &entry.key)
                    || preview_label.is_some_and(|label| label == entry.label),
            ))),
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
        items.push(Rc::new(RefCell::new(Value::Map(map))));
    }

    Value::List(items)
}

pub(super) fn build_track_output_label(app: &app::App, tp: &sequencer::sequencer::TrackParams) -> Value {
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
}

pub(super) fn build_track_output_options(app: &app::App) -> Value {
    let mut labels = vec![
        Rc::new(RefCell::new(Value::String("main".to_string()))),
        Rc::new(RefCell::new(Value::String("sends only".to_string()))),
    ];
    labels.extend(
        app.buses
            .iter()
            .filter(|bus| bus.id != sequencer::sequencer::BusId::MIX)
            .map(|bus| Rc::new(RefCell::new(Value::String(bus.name.clone())))),
    );
    Value::List(labels)
}

pub(super) fn build_track_bus_sends(app: &app::App, _tp: &sequencer::sequencer::TrackParams) -> Value {
    use std::collections::HashMap;

    let items = app
        .buses
        .iter()
        .enumerate()
        .filter(|(_, bus)| bus.id != sequencer::sequencer::BusId::MIX)
        .map(|(bus_idx, bus)| {
            let mut map = HashMap::new();
            map.insert(
                "bus-idx".to_string(),
                Rc::new(RefCell::new(Value::Number(bus_idx as f64))),
            );
            map.insert(
                "name".to_string(),
                Rc::new(RefCell::new(Value::String(bus.name.clone()))),
            );
            Rc::new(RefCell::new(Value::Map(map)))
        })
        .collect();
    Value::List(items)
}

/// Build a Lisp Value::Map of track parameters for the current track.
pub(crate) fn build_track_params(state: &Arc<SequencerState>, track: usize) -> Value {
    use std::collections::HashMap;
    let tp = &state.pattern.track_params[track];
    let mut map: HashMap<String, Rc<RefCell<Value>>> = HashMap::new();
    map.insert(
        "gate".into(),
        Rc::new(RefCell::new(Value::Bool(tp.is_gate_on()))),
    );
    map.insert(
        "attack".into(),
        Rc::new(RefCell::new(Value::Number(tp.get_attack_ms() as f64))),
    );
    map.insert(
        "release".into(),
        Rc::new(RefCell::new(Value::Number(tp.get_release_ms() as f64))),
    );
    map.insert(
        "swing".into(),
        Rc::new(RefCell::new(Value::Number(tp.get_swing() as f64))),
    );
    map.insert(
        "swing-resolution".into(),
        Rc::new(RefCell::new(Value::String(
            tp.get_swing_resolution().label().to_string(),
        ))),
    );
    map.insert(
        "num-steps".into(),
        Rc::new(RefCell::new(Value::Number(tp.get_num_steps() as f64))),
    );
    map.insert(
        "volume".into(),
        Rc::new(RefCell::new(Value::Number(tp.get_volume() as f64))),
    );
    map.insert(
        "pan".into(),
        Rc::new(RefCell::new(Value::Number(tp.get_pan() as f64))),
    );
    map.insert(
        "mute".into(),
        Rc::new(RefCell::new(Value::Bool(tp.is_muted()))),
    );
    map.insert(
        "solo".into(),
        Rc::new(RefCell::new(Value::Bool(tp.is_solo()))),
    );
    map.insert(
        "timebase".into(),
        Rc::new(RefCell::new(Value::String(
            tp.get_timebase().label().to_string(),
        ))),
    );
    map.insert(
        "send".into(),
        Rc::new(RefCell::new(Value::Number(tp.get_send() as f64))),
    );
    map.insert(
        "poly".into(),
        Rc::new(RefCell::new(Value::Bool(tp.is_polyphonic()))),
    );
    map.insert(
        "max-polyphony".into(),
        Rc::new(RefCell::new(Value::Number(tp.get_max_polyphony() as f64))),
    );
    map.insert(
        "mute-group".into(),
        Rc::new(RefCell::new(Value::Number(tp.get_mute_group() as f64))),
    );
    Value::Map(map)
}

/// Build a Lisp Value::List of bools indicating which steps have any p-locks on the given track.
pub(crate) fn build_step_has_plocks(
    state: &Arc<SequencerState>,
    track: usize,
    descriptors: &[Vec<sequencer::effects::EffectDescriptor>],
) -> Value {
    let mask = track_step_plock_mask(state, track, descriptors);
    build_step_has_plocks_from_mask(&mask)
}

pub(crate) fn build_step_has_plocks_from_mask(mask: &[u64; MAX_STEPS / 64]) -> Value {
    let items: Vec<Rc<RefCell<Value>>> = (0..MAX_STEPS)
        .map(|step| {
            Rc::new(RefCell::new(Value::Bool(
                mask[step / 64] & (1u64 << (step % 64)) != 0,
            )))
        })
        .collect();
    Value::List(items)
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct PlockVariantStepRender {
    pub(crate) kind: u8,
    pub(crate) color: [f32; 3],
}

pub(crate) fn plock_variant_step_render_values(
    state: &Arc<SequencerState>,
    track: usize,
) -> Vec<PlockVariantStepRender> {
    const SEQ_ONLY_COLOR: [f32; 3] = [0.545_098_07, 0.545_098_07, 0.588_235_3];
    let assignments = state.reconcile_plock_variant_registry_for_track(track);
    (0..MAX_STEPS)
        .map(|step| {
            if let Some(assignment) = assignments.get(step).and_then(Clone::clone) {
                PlockVariantStepRender {
                    kind: 2,
                    color: assignment.color,
                }
            } else if sequencer::plock_variants::live_track_has_seq_lock(
                state.as_ref(),
                track,
                step,
            ) {
                PlockVariantStepRender {
                    kind: 1,
                    color: SEQ_ONLY_COLOR,
                }
            } else {
                PlockVariantStepRender {
                    kind: 0,
                    color: [0.0, 0.0, 0.0],
                }
            }
        })
        .collect()
}

pub(crate) fn build_step_plock_kinds(state: &Arc<SequencerState>, track: usize) -> Value {
    build_step_plock_kinds_from_render(&plock_variant_step_render_values(state, track))
}

pub(crate) fn build_step_plock_kinds_from_render(render_values: &[PlockVariantStepRender]) -> Value {
    Value::List(
        render_values
            .iter()
            .map(|render| Rc::new(RefCell::new(Value::Number(render.kind as f64))))
            .collect(),
    )
}

pub(crate) fn build_step_variant_color_channel(
    state: &Arc<SequencerState>,
    track: usize,
    channel: usize,
) -> Value {
    build_step_variant_color_channel_from_render(
        &plock_variant_step_render_values(state, track),
        channel,
    )
}

pub(crate) fn build_step_variant_color_channel_from_render(
    render_values: &[PlockVariantStepRender],
    channel: usize,
) -> Value {
    Value::List(
        render_values
            .iter()
            .map(|render| {
                Rc::new(RefCell::new(Value::Number(
                    render.color.get(channel).copied().unwrap_or(0.0) as f64,
                )))
            })
            .collect(),
    )
}

pub(crate) fn build_all_track_step_plock_kinds(
    state: &Arc<SequencerState>,
    app: &app::App,
) -> Value {
    Value::List(
        (0..app.tracks.len())
            .map(|track| Rc::new(RefCell::new(build_step_plock_kinds(state, track))))
            .collect(),
    )
}

pub(crate) fn build_all_track_step_variant_color_channel(
    state: &Arc<SequencerState>,
    app: &app::App,
    channel: usize,
) -> Value {
    Value::List(
        (0..app.tracks.len())
            .map(|track| {
                Rc::new(RefCell::new(build_step_variant_color_channel(
                    state, track, channel,
                )))
            })
            .collect(),
    )
}

/// One bit per step: whether any effect/instrument/midi-fx/timebase/swing
/// plock exists for that step. Single flat scan per slot instead of the
/// per-(step, slot, param) probing done by track_step_has_plock.
pub(crate) fn track_step_plock_mask(
    state: &Arc<SequencerState>,
    track: usize,
    descriptors: &[Vec<sequencer::effects::EffectDescriptor>],
) -> [u64; MAX_STEPS / 64] {
    let mut mask = [0u64; MAX_STEPS / 64];
    let chain = &state.pattern.effect_chains[track];
    let num_slots = descriptors.get(track).map(|d| d.len()).unwrap_or(0);
    for slot_idx in 0..num_slots {
        if let Some(slot) = chain.get(slot_idx) {
            let np = slot.num_params.load(Ordering::Relaxed) as usize;
            slot.plocks.or_step_plock_mask(&mut mask, np);
        }
    }
    for slot in &state.pattern.midi_fx_slots[track] {
        let np = slot.num_params.load(Ordering::Relaxed) as usize;
        slot.plocks.or_step_plock_mask(&mut mask, np);
    }
    let instrument_slot = &state.pattern.instrument_slots[track];
    let instrument_np = instrument_slot.num_params.load(Ordering::Relaxed) as usize;
    instrument_slot
        .plocks
        .or_step_plock_mask(&mut mask, instrument_np);
    if let Some(Some(rack)) = state.pattern.rack_tracks.lock().unwrap().get(track) {
        for rack_macro in &rack.macros {
            for (step, value) in rack_macro.plocks.iter().enumerate().take(MAX_STEPS) {
                if value.is_some() {
                    mask[step / 64] |= 1u64 << (step % 64);
                }
            }
        }
        for slot in &rack.slots {
            for step in 0..MAX_STEPS {
                if slot.param_plocks.step_has_plock(step) {
                    mask[step / 64] |= 1u64 << (step % 64);
                }
            }
            let num_params = slot.instrument_slot.num_params as usize;
            for step in 0..MAX_STEPS {
                let Some(step_plocks) = slot.instrument_slot.plocks.get(step) else {
                    continue;
                };
                if step_plocks
                    .iter()
                    .take(num_params)
                    .any(|value| value.is_some())
                {
                    mask[step / 64] |= 1u64 << (step % 64);
                }
            }
            for effect in &slot.effect_slots {
                let num_params = effect.num_params as usize;
                for step in 0..MAX_STEPS {
                    if effect
                        .plocks
                        .get(step)
                        .is_some_and(|row| row.iter().take(num_params).any(Option::is_some))
                    {
                        mask[step / 64] |= 1u64 << (step % 64);
                    }
                }
            }
        }
    }
    let timebase_plocks = &state.pattern.timebase_plocks[track];
    let swing_plocks = &state.pattern.swing_plocks[track];
    let swing_resolution_plocks = &state.pattern.swing_resolution_plocks[track];
    for step in 0..MAX_STEPS {
        let word = step / 64;
        let bit = 1u64 << (step % 64);
        if mask[word] & bit != 0 {
            continue;
        }
        if timebase_plocks.has_plock(step)
            || swing_plocks.has_plock(step)
            || swing_resolution_plocks.has_plock(step)
            || sequencer::plock_variants::live_track_has_seq_lock(state.as_ref(), track, step)
            || sequencer::plock_variants::live_track_variant_key(state.as_ref(), track, step)
                .is_some()
        {
            mask[word] |= bit;
        }
    }
    mask
}

pub(crate) fn track_step_has_plock(
    state: &Arc<SequencerState>,
    track: usize,
    descriptors: &[Vec<sequencer::effects::EffectDescriptor>],
    step: usize,
) -> bool {
    let chain = &state.pattern.effect_chains[track];
    let midi_fx_slots = &state.pattern.midi_fx_slots[track];
    let num_slots = descriptors.get(track).map(|d| d.len()).unwrap_or(0);
    let instrument_slot = &state.pattern.instrument_slots[track];
    let instrument_num_params = instrument_slot.num_params.load(Ordering::Relaxed) as usize;
    let timebase_plocks = &state.pattern.timebase_plocks[track];
    let swing_plocks = &state.pattern.swing_plocks[track];
    let swing_resolution_plocks = &state.pattern.swing_resolution_plocks[track];
    let effect_has_plock = (0..num_slots).any(|slot_idx| {
        let Some(slot) = chain.get(slot_idx) else {
            return false;
        };
        let np = slot.num_params.load(Ordering::Relaxed) as usize;
        (0..np).any(|p| slot.plocks.get(step, p).is_some())
    });
    let instrument_has_plock =
        (0..instrument_num_params).any(|p| instrument_slot.plocks.get(step, p).is_some());
    let rack_slot_has_plock = state
        .pattern
        .rack_tracks
        .lock()
        .unwrap()
        .get(track)
        .and_then(|rack| rack.as_ref())
        .is_some_and(|rack| {
            if rack
                .macros
                .iter()
                .any(|rack_macro| rack_macro.plocks.get(step).is_some_and(Option::is_some))
            {
                return true;
            }
            rack.slots.iter().any(|slot| {
                if slot.param_plocks.step_has_plock(step) {
                    return true;
                }
                let num_params = slot.instrument_slot.num_params as usize;
                if slot
                    .instrument_slot
                    .plocks
                    .get(step)
                    .is_some_and(|step_plocks| {
                        step_plocks
                            .iter()
                            .take(num_params)
                            .any(|value| value.is_some())
                    })
                {
                    return true;
                }
                slot.effect_slots.iter().any(|effect| {
                    let num_params = effect.num_params as usize;
                    effect
                        .plocks
                        .get(step)
                        .is_some_and(|row| row.iter().take(num_params).any(Option::is_some))
                })
            })
        });
    let midi_fx_has_plock = midi_fx_slots.iter().any(|slot| {
        let np = slot.num_params.load(Ordering::Relaxed) as usize;
        (0..np).any(|p| slot.plocks.get(step, p).is_some())
    });

    effect_has_plock
        || midi_fx_has_plock
        || instrument_has_plock
        || rack_slot_has_plock
        || timebase_plocks.has_plock(step)
        || swing_plocks.has_plock(step)
        || swing_resolution_plocks.has_plock(step)
        || sequencer::plock_variants::live_track_has_seq_lock(state.as_ref(), track, step)
        || sequencer::plock_variants::live_track_variant_key(state.as_ref(), track, step).is_some()
}

pub(crate) fn playhead_transition_changes_param_bindings(
    state: &Arc<SequencerState>,
    track: usize,
    descriptors: &[Vec<sequencer::effects::EffectDescriptor>],
    selected_steps: &Arc<Mutex<HashSet<usize>>>,
    previous_step: usize,
    current_step: usize,
) -> bool {
    if previous_step == current_step || !selected_steps.lock().unwrap().is_empty() {
        return false;
    }
    track_step_has_plock(state, track, descriptors, previous_step)
        || track_step_has_plock(state, track, descriptors, current_step)
}
