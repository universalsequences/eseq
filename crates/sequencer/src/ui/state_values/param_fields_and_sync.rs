use super::*;

fn instrument_param_display_value(
    app: &app::App,
    track: usize,
    param_idx: usize,
    display_step: Option<usize>,
    selected_neural_neurons: Option<
        &std::collections::BTreeSet<sequencer::lisp_host::SelectedNeuralNeuron>,
    >,
) -> Option<(String, f32)> {
    app.graph
        .instrument_descriptors
        .get(track)
        .and_then(|desc| desc.params.get(param_idx))
        .and_then(|pdesc| {
            app.state.pattern.instrument_slots.get(track).map(|slot| {
                let neural_value = selected_neural_neurons.and_then(|selection| {
                    sequencer::lisp_host::selected_neural_instrument_plock_value(
                        &app.state, selection, track, param_idx,
                    )
                });
                let stored = neural_value
                    .or_else(|| {
                        display_step.and_then(|step| {
                            held_plock_value(&app.state, track, step, |s| {
                                slot.plocks.get(s, param_idx)
                            })
                        })
                    })
                    .or_else(|| app.effective_instrument_param_value(track, param_idx))
                    .unwrap_or_else(|| {
                        slot_param_stored_value(slot, pdesc, param_idx, display_step)
                    });
                (pdesc.name.clone(), pdesc.stored_to_user(stored))
            })
        })
}

fn sync_instrument_param_value_fields(
    rt: &mut Runtime,
    app: &app::App,
    track: usize,
    param_idx: usize,
    display_step: Option<usize>,
    selected_neural_neurons: Option<
        &std::collections::BTreeSet<sequencer::lisp_host::SelectedNeuralNeuron>,
    >,
    publish_fx_relative: bool,
) -> bool {
    let Some((name, value)) = instrument_param_display_value(
        app,
        track,
        param_idx,
        display_step,
        selected_neural_neurons,
    ) else {
        return false;
    };
    let value = Value::Number(value as f64);
    let mut needs_ui = reactive_set_needs_ui(rt.set_reactive(
        "SEQ",
        &instrument_param_value_field(track, param_idx, &name),
        value.clone(),
    ));
    if publish_fx_relative {
        needs_ui |= reactive_set_needs_ui(rt.set_reactive(
            "SEQ",
            &fx_instrument_param_value_field(param_idx, &name),
            value,
        ));
    }
    needs_ui
}

pub(crate) fn sync_instrument_param_value_field(
    rt: &mut Runtime,
    app: &app::App,
    track: usize,
    param_idx: usize,
    display_step: Option<usize>,
) -> bool {
    sync_instrument_param_value_fields(rt, app, track, param_idx, display_step, None, false)
}

pub(crate) fn sync_fx_instrument_param_value_field(
    rt: &mut Runtime,
    app: &app::App,
    track: usize,
    param_idx: usize,
    display_step: Option<usize>,
) -> bool {
    sync_instrument_param_value_fields(rt, app, track, param_idx, display_step, None, true)
}

fn sync_instrument_tensor_value_fields(
    rt: &mut Runtime,
    app: &app::App,
    track: usize,
    tensor_idx: usize,
    display_step: Option<usize>,
    publish_fx_relative: bool,
) -> bool {
    let Some((name, values)) = app
        .graph
        .instrument_descriptors
        .get(track)
        .and_then(|desc| desc.tensor_params.get(tensor_idx))
        .and_then(|tdesc| {
            app.state.pattern.instrument_slots.get(track).map(|slot| {
                let values = slot
                    .tensor_params
                    .resolved_values(display_step, tensor_idx)
                    .unwrap_or_else(|| tdesc.default.clone());
                (tdesc.name.clone(), values)
            })
        })
    else {
        return false;
    };
    let list = || Value::List(
        values
            .iter()
            .map(|value| Rc::new(RefCell::new(Value::Number(*value as f64))))
            .collect()
    );
    let mut needs_ui = reactive_set_needs_ui(rt.set_reactive(
        "SEQ",
        &instrument_tensor_value_field(track, tensor_idx, &name),
        list(),
    ));
    if publish_fx_relative {
        needs_ui |= reactive_set_needs_ui(rt.set_reactive(
            "SEQ",
            &fx_instrument_tensor_value_field(tensor_idx, &name),
            list(),
        ));
    }
    needs_ui
}

pub(crate) fn sync_instrument_tensor_value_field(
    rt: &mut Runtime,
    app: &app::App,
    track: usize,
    tensor_idx: usize,
    display_step: Option<usize>,
) -> bool {
    sync_instrument_tensor_value_fields(rt, app, track, tensor_idx, display_step, false)
}

pub(crate) fn sync_fx_instrument_tensor_value_field(
    rt: &mut Runtime,
    app: &app::App,
    track: usize,
    tensor_idx: usize,
    display_step: Option<usize>,
) -> bool {
    sync_instrument_tensor_value_fields(rt, app, track, tensor_idx, display_step, true)
}

pub(crate) fn sync_rack_macro_value_fields(
    rt: &mut Runtime,
    app: &app::App,
    track: usize,
    display_step: Option<usize>,
) -> bool {
    let rack_macros = {
        let racks = app.state.pattern.rack_tracks.lock().unwrap();
        let Some(Some(rack)) = racks.get(track) else {
            return false;
        };
        rack.macros
            .iter()
            .map(|rack_macro| {
                let plock_value = display_step
                    .and_then(|step| rack_macro.plocks.get(step))
                    .and_then(|value| *value);
                (rack_macro.id, rack_macro.value, plock_value)
            })
            .collect::<Vec<_>>()
    };
    let mut needs_ui = false;
    for (id, base_value, plock_value) in rack_macros {
        let value = app
            .effective_rack_macro_value(track, id, display_step)
            .unwrap_or(base_value);
        needs_ui |= reactive_set_needs_ui(rt.set_reactive(
            "SEQ",
            &rack_macro_value_field(track, id.index()),
            Value::Number(value as f64),
        ));
        needs_ui |= reactive_set_needs_ui(rt.set_reactive(
            "SEQ",
            &rack_macro_plock_active_field(track, id.index()),
            Value::Number(if plock_value.is_some() { 1.0 } else { 0.0 }),
        ));
        needs_ui |= reactive_set_needs_ui(rt.set_reactive(
            "SEQ",
            &rack_macro_plock_default_field(track, id.index()),
            Value::Number(base_value as f64),
        ));
    }
    needs_ui
}

pub(crate) fn sync_rack_macro_value_field(
    rt: &mut Runtime,
    app: &app::App,
    track: usize,
    id: sequencer::sequencer::RackMacroId,
    display_step: Option<usize>,
) -> bool {
    let (base_value, plock_value) = {
        let racks = app.state.pattern.rack_tracks.lock().unwrap();
        let Some(rack_macro) = racks
            .get(track)
            .and_then(Option::as_ref)
            .and_then(|rack| rack.macros.get(id.index()))
        else {
            return false;
        };
        (
            rack_macro.value,
            display_step
                .and_then(|step| rack_macro.plocks.get(step))
                .and_then(|value| *value),
        )
    };
    let value = app
        .effective_rack_macro_value(track, id, display_step)
        .unwrap_or(base_value);
    let mut needs_ui = reactive_set_needs_ui(rt.set_reactive(
        "SEQ",
        &rack_macro_value_field(track, id.index()),
        Value::Number(value as f64),
    ));
    needs_ui |= reactive_set_needs_ui(rt.set_reactive(
        "SEQ",
        &rack_macro_plock_active_field(track, id.index()),
        Value::Number(if plock_value.is_some() { 1.0 } else { 0.0 }),
    ));
    needs_ui |= reactive_set_needs_ui(rt.set_reactive(
        "SEQ",
        &rack_macro_plock_default_field(track, id.index()),
        Value::Number(base_value as f64),
    ));
    needs_ui
}

pub(super) fn rack_slot_control_value(
    rack: &sequencer::sequencer::RackTrackSnapshot,
    slot_idx: usize,
    slot: &sequencer::sequencer::RackSlotSnapshot,
    param: sequencer::sequencer::RackSlotParam,
    display_step: Option<usize>,
) -> f32 {
    if let Some(value) = display_step.and_then(|step| slot.param_plocks.get(step, param)) {
        return param.clamp(value);
    }
    rack_macro_mapped_value(rack, display_step, |target| {
        matches!(
            target,
            sequencer::sequencer::RackMacroTarget::SlotParam {
                slot,
                param: target_param,
            } if *slot == slot_idx
                && sequencer::sequencer::RackSlotParam::from_name(target_param) == Some(param)
        )
    })
    .map(|value| param.clamp(value))
    .unwrap_or_else(|| param.clamp(slot.param_value_at_step(param, usize::MAX)))
}

pub(super) fn set_rack_value_field_updates(
    rt: &mut Runtime,
    updates: impl IntoIterator<Item = (String, Value)>,
) -> bool {
    updates.into_iter().fold(false, |needs_ui, (field, value)| {
        reactive_set_needs_ui(rt.set_reactive("SEQ", &field, value)) || needs_ui
    })
}

pub(super) fn rack_slot_sample_duration(app: &app::App, slot: &sequencer::sequencer::RackSlotSnapshot) -> f64 {
    let Some((buffer_id, sample_name, _)) = slot.sample_id.as_ref() else {
        return 1.0;
    };
    app.sample_buffer_path_registry
        .get(buffer_id)
        .or_else(|| app.sample_path_registry.get(sample_name))
        .and_then(|path| {
            eseqlisp::audio::sample::get_registered_sample(&path.display().to_string())
        })
        .map(|sample| sample.duration_seconds)
        .unwrap_or(1.0)
}

pub(super) fn rack_sampler_selection_update(
    app: &app::App,
    track: usize,
    slot_idx: usize,
    slot: &sequencer::sequencer::RackSlotSnapshot,
    param_idx: usize,
    stored_value: f32,
) -> Option<(String, Value)> {
    if slot.instrument_type != sequencer::sequencer::InstrumentType::Sampler {
        return None;
    }
    let marker = match param_idx {
        2 => "start",
        3 => "end",
        _ => return None,
    };
    Some((
        rack_slot_sampler_selection_time_field(track, slot_idx, marker),
        Value::Number(stored_value as f64 * rack_slot_sample_duration(app, slot)),
    ))
}

pub(crate) fn sync_rack_slot_control_value_field(
    rt: &mut Runtime,
    app: &app::App,
    track: usize,
    slot_idx: usize,
    param: sequencer::sequencer::RackSlotParam,
    display_step: Option<usize>,
) -> bool {
    let value = {
        let racks = app.state.pattern.rack_tracks.lock().unwrap();
        let Some(rack) = racks.get(track).and_then(Option::as_ref) else {
            return false;
        };
        let Some(slot) = rack.slots.get(slot_idx) else {
            return false;
        };
        rack_slot_control_value(rack, slot_idx, slot, param, display_step)
    };
    let value = if matches!(
        param,
        sequencer::sequencer::RackSlotParam::Mute | sequencer::sequencer::RackSlotParam::Solo
    ) {
        Value::Bool(value > 0.5)
    } else {
        Value::Number(value as f64)
    };
    reactive_set_needs_ui(rt.set_reactive(
        "SEQ",
        &rack_slot_value_field(track, slot_idx, param),
        value,
    ))
}

pub(crate) fn sync_rack_slot_instrument_param_value_field(
    rt: &mut Runtime,
    app: &app::App,
    track: usize,
    slot_idx: usize,
    param_idx: usize,
    display_step: Option<usize>,
) -> bool {
    let (name, value, selection_update) = {
        let racks = app.state.pattern.rack_tracks.lock().unwrap();
        let Some(rack) = racks.get(track).and_then(Option::as_ref) else {
            return false;
        };
        let Some(slot) = rack.slots.get(slot_idx) else {
            return false;
        };
        let Some(descriptor) = app.rack_slot_instrument_descriptor(slot) else {
            return false;
        };
        let Some(param) = descriptor.params.get(param_idx) else {
            return false;
        };
        let stored =
            rack_slot_param_value(rack, slot_idx, slot, &descriptor, param_idx, display_step);
        (
            param.name.clone(),
            param.stored_to_user(stored),
            rack_sampler_selection_update(app, track, slot_idx, slot, param_idx, stored),
        )
    };
    let mut needs_ui = reactive_set_needs_ui(rt.set_reactive(
        "SEQ",
        &rack_slot_instrument_param_value_field(track, slot_idx, param_idx, &name),
        Value::Number(value as f64),
    ));
    if let Some((field, value)) = selection_update {
        needs_ui |= reactive_set_needs_ui(rt.set_reactive("SEQ", &field, value));
    }
    needs_ui
}

pub(crate) fn sync_rack_slot_effect_param_value_field(
    rt: &mut Runtime,
    app: &app::App,
    track: usize,
    rack_slot: usize,
    effect_slot: usize,
    param_idx: usize,
    display_step: Option<usize>,
) -> bool {
    let (name, value) = {
        let racks = app.state.pattern.rack_tracks.lock().unwrap();
        let Some(rack) = racks.get(track).and_then(Option::as_ref) else {
            return false;
        };
        let Some(slot) = rack.slots.get(rack_slot) else {
            return false;
        };
        let Some(descriptor) = slot.effect_descriptors.get(effect_slot) else {
            return false;
        };
        let Some(snapshot) = slot.effect_slots.get(effect_slot) else {
            return false;
        };
        let Some(param) = descriptor.params.get(param_idx) else {
            return false;
        };
        let value = rack_effect_param_value(
            rack,
            rack_slot,
            effect_slot,
            snapshot,
            descriptor,
            param_idx,
            display_step,
        );
        (param.name.clone(), value)
    };
    reactive_set_needs_ui(rt.set_reactive(
        "SEQ",
        &rack_slot_effect_param_value_field(track, rack_slot, effect_slot, param_idx, &name),
        Value::Number(value as f64),
    ))
}

pub(crate) fn sync_rack_panel_param_value_fields(
    rt: &mut Runtime,
    app: &app::App,
    track: usize,
    display_step: Option<usize>,
) -> bool {
    let mut updates = Vec::new();
    {
        let racks = app.state.pattern.rack_tracks.lock().unwrap();
        let Some(Some(rack)) = racks.get(track) else {
            return false;
        };
        for (slot_idx, slot) in rack.slots.iter().enumerate() {
            for param in sequencer::sequencer::RackSlotParam::ALL {
                let value = rack_slot_control_value(rack, slot_idx, slot, param, display_step);
                let value = if matches!(
                    param,
                    sequencer::sequencer::RackSlotParam::Mute
                        | sequencer::sequencer::RackSlotParam::Solo
                ) {
                    Value::Bool(value > 0.5)
                } else {
                    Value::Number(value as f64)
                };
                updates.push((rack_slot_value_field(track, slot_idx, param), value));
            }

            if let Some(descriptor) = app.rack_slot_instrument_descriptor(slot) {
                for (param_idx, param) in descriptor.params.iter().enumerate() {
                    let value = rack_slot_param_value(
                        rack,
                        slot_idx,
                        slot,
                        &descriptor,
                        param_idx,
                        display_step,
                    );
                    updates.push((
                        rack_slot_instrument_param_value_field(
                            track,
                            slot_idx,
                            param_idx,
                            &param.name,
                        ),
                        Value::Number(param.stored_to_user(value) as f64),
                    ));
                    if let Some(update) =
                        rack_sampler_selection_update(app, track, slot_idx, slot, param_idx, value)
                    {
                        updates.push(update);
                    }
                }
            }

            for (effect_slot, (descriptor, snapshot)) in slot
                .effect_descriptors
                .iter()
                .zip(&slot.effect_slots)
                .enumerate()
            {
                if snapshot.node_id == 0 {
                    continue;
                }
                for (param_idx, param) in descriptor.params.iter().enumerate() {
                    let value = rack_effect_param_value(
                        rack,
                        slot_idx,
                        effect_slot,
                        snapshot,
                        descriptor,
                        param_idx,
                        display_step,
                    );
                    updates.push((
                        rack_slot_effect_param_value_field(
                            track,
                            slot_idx,
                            effect_slot,
                            param_idx,
                            &param.name,
                        ),
                        Value::Number(value as f64),
                    ));
                }
            }
        }
    }
    set_rack_value_field_updates(rt, updates)
}

pub(crate) fn sync_rack_macro_target_value_fields(
    rt: &mut Runtime,
    app: &app::App,
    track: usize,
    id: sequencer::sequencer::RackMacroId,
    display_step: Option<usize>,
) -> bool {
    let mut updates = Vec::new();
    {
        let racks = app.state.pattern.rack_tracks.lock().unwrap();
        let Some(Some(rack)) = racks.get(track) else {
            return false;
        };
        let Some(rack_macro) = rack.macros.get(id.index()) else {
            return false;
        };
        let mut sampler_descriptor = None;
        for mapping in &rack_macro.mappings {
            match &mapping.target {
                sequencer::sequencer::RackMacroTarget::SlotParam { slot, param } => {
                    let Some(param) = sequencer::sequencer::RackSlotParam::from_name(param) else {
                        continue;
                    };
                    let Some(slot_data) = rack.slots.get(*slot) else {
                        continue;
                    };
                    let displayed =
                        rack_slot_control_value(rack, *slot, slot_data, param, display_step);
                    let value = if matches!(
                        param,
                        sequencer::sequencer::RackSlotParam::Mute
                            | sequencer::sequencer::RackSlotParam::Solo
                    ) {
                        Value::Bool(displayed > 0.5)
                    } else {
                        Value::Number(displayed as f64)
                    };
                    updates.push((rack_slot_value_field(track, *slot, param), value));
                }
                sequencer::sequencer::RackMacroTarget::SlotInstrumentParam {
                    slot,
                    param_index,
                    ..
                } => {
                    let Some(slot_data) = rack.slots.get(*slot) else {
                        continue;
                    };
                    let descriptor = if let Some(descriptor) =
                        app.rack_slot_cached_instrument_descriptor(slot_data)
                    {
                        descriptor
                    } else if matches!(
                        slot_data.instrument_type,
                        sequencer::sequencer::InstrumentType::Sampler
                    ) {
                        sampler_descriptor.get_or_insert_with(
                            sequencer::effects::EffectDescriptor::builtin_sampler,
                        )
                    } else {
                        continue;
                    };
                    let Some(param) = descriptor.params.get(*param_index) else {
                        continue;
                    };
                    let stored = rack_slot_param_value(
                        rack,
                        *slot,
                        slot_data,
                        descriptor,
                        *param_index,
                        display_step,
                    );
                    updates.push((
                        rack_slot_instrument_param_value_field(
                            track,
                            *slot,
                            *param_index,
                            &param.name,
                        ),
                        Value::Number(param.stored_to_user(stored) as f64),
                    ));
                    if let Some(update) = rack_sampler_selection_update(
                        app,
                        track,
                        *slot,
                        slot_data,
                        *param_index,
                        stored,
                    ) {
                        updates.push(update);
                    }
                }
                sequencer::sequencer::RackMacroTarget::SlotEffectParam {
                    slot,
                    effect_slot,
                    param_index,
                    ..
                } => {
                    let Some(slot_data) = rack.slots.get(*slot) else {
                        continue;
                    };
                    let Some(descriptor) = slot_data.effect_descriptors.get(*effect_slot) else {
                        continue;
                    };
                    let Some(snapshot) = slot_data.effect_slots.get(*effect_slot) else {
                        continue;
                    };
                    let Some(param) = descriptor.params.get(*param_index) else {
                        continue;
                    };
                    let displayed = rack_effect_param_value(
                        rack,
                        *slot,
                        *effect_slot,
                        snapshot,
                        descriptor,
                        *param_index,
                        display_step,
                    );
                    updates.push((
                        rack_slot_effect_param_value_field(
                            track,
                            *slot,
                            *effect_slot,
                            *param_index,
                            &param.name,
                        ),
                        Value::Number(displayed as f64),
                    ));
                }
            }
        }
    }
    set_rack_value_field_updates(rt, updates)
}

pub(crate) fn sync_instrument_param_value_field_with_neural_selection(
    rt: &mut Runtime,
    app: &app::App,
    track: usize,
    param_idx: usize,
    display_step: Option<usize>,
    selected_neural_neurons: Option<
        &std::collections::BTreeSet<sequencer::lisp_host::SelectedNeuralNeuron>,
    >,
) -> bool {
    sync_instrument_param_value_fields(
        rt,
        app,
        track,
        param_idx,
        display_step,
        selected_neural_neurons,
        false,
    )
}

pub(crate) fn sync_fx_instrument_param_value_field_with_neural_selection(
    rt: &mut Runtime,
    app: &app::App,
    track: usize,
    param_idx: usize,
    display_step: Option<usize>,
    selected_neural_neurons: Option<
        &std::collections::BTreeSet<sequencer::lisp_host::SelectedNeuralNeuron>,
    >,
) -> bool {
    sync_instrument_param_value_fields(
        rt,
        app,
        track,
        param_idx,
        display_step,
        selected_neural_neurons,
        true,
    )
}

pub(crate) fn sync_sampler_selection_time_fields(
    rt: &mut Runtime,
    app: &app::App,
    track: usize,
    display_step: Option<usize>,
) -> bool {
    if !app.is_sampler_track(track) {
        return false;
    }
    let sample_duration = app
        .sampler_path_for_track(track)
        .as_ref()
        .and_then(|p| eseqlisp::audio::sample::get_registered_sample(&p.display().to_string()))
        .map(|sample| sample.duration_seconds)
        .unwrap_or(1.0);
    let Some(slot) = app.state.pattern.instrument_slots.get(track) else {
        return false;
    };
    let start_raw = display_step
        .and_then(|step| slot.plocks.get(step, 2))
        .unwrap_or_else(|| slot.defaults.get(2));
    let end_raw = display_step
        .and_then(|step| slot.plocks.get(step, 3))
        .unwrap_or_else(|| slot.defaults.get(3));
    let start = start_raw as f64 * sample_duration;
    let end = end_raw as f64 * sample_duration;
    let mut needs_ui = reactive_set_needs_ui(rt.set_reactive(
        "SEQ",
        &sampler_selection_time_field(track, "start"),
        Value::Number(start),
    ));
    needs_ui |= reactive_set_needs_ui(rt.set_reactive(
        "SEQ",
        &sampler_selection_time_field(track, "end"),
        Value::Number(end),
    ));
    needs_ui
}

fn sync_instrument_base_note_value_fields(
    rt: &mut Runtime,
    app: &app::App,
    track: usize,
    publish_fx_relative: bool,
) -> bool {
    if track >= app.tracks.len() {
        return false;
    }
    let value = Value::Number(f32::from_bits(
        app.state.pattern.instrument_base_note_offsets[track].load(Ordering::Relaxed),
    ) as f64);
    let mut needs_ui = reactive_set_needs_ui(rt.set_reactive(
        "SEQ",
        &instrument_base_note_value_field(track),
        value.clone(),
    ));
    if publish_fx_relative {
        needs_ui |= reactive_set_needs_ui(rt.set_reactive(
            "SEQ",
            fx_instrument_base_note_value_field(),
            value,
        ));
    }
    needs_ui
}

pub(crate) fn sync_instrument_base_note_value_field(
    rt: &mut Runtime,
    app: &app::App,
    track: usize,
) -> bool {
    sync_instrument_base_note_value_fields(rt, app, track, false)
}

pub(crate) fn sync_fx_instrument_base_note_value_field(
    rt: &mut Runtime,
    app: &app::App,
    track: usize,
) -> bool {
    sync_instrument_base_note_value_fields(rt, app, track, true)
}

pub(crate) fn sync_track_effect_param_value_field(
    rt: &mut Runtime,
    app: &app::App,
    track: usize,
    slot_idx: usize,
    param_idx: usize,
    display_step: Option<usize>,
) -> bool {
    if let Some((name, value)) = app
        .graph
        .effect_descriptors
        .get(track)
        .and_then(|slots| slots.get(slot_idx))
        .and_then(|desc| desc.params.get(param_idx).map(|p| (&desc.name, p)))
        .and_then(|(_, pdesc)| {
            app.state
                .pattern
                .effect_chains
                .get(track)
                .and_then(|chain| chain.get(slot_idx))
                .map(|slot| {
                    let stored = display_step
                        .and_then(|step| slot.plocks.get(step, param_idx))
                        .or_else(|| app.effective_slot_param_value(track, slot_idx, param_idx))
                        .unwrap_or_else(|| {
                            slot_param_stored_value(slot, pdesc, param_idx, display_step)
                        });
                    (pdesc.name.clone(), stored)
                })
        })
    {
        return reactive_set_needs_ui(rt.set_reactive(
            "SEQ",
            &track_effect_param_value_field(track, slot_idx, param_idx, &name),
            Value::Number(value as f64),
        ));
    }
    false
}

pub(crate) fn sync_track_effect_param_value_field_with_neural_selection(
    rt: &mut Runtime,
    app: &app::App,
    track: usize,
    slot_idx: usize,
    param_idx: usize,
    display_step: Option<usize>,
    selected_neural_neurons: Option<
        &std::collections::BTreeSet<sequencer::lisp_host::SelectedNeuralNeuron>,
    >,
) -> bool {
    if let Some((name, value)) = app
        .graph
        .effect_descriptors
        .get(track)
        .and_then(|slots| slots.get(slot_idx))
        .and_then(|desc| desc.params.get(param_idx).map(|p| (&desc.name, p)))
        .and_then(|(_, pdesc)| {
            app.state
                .pattern
                .effect_chains
                .get(track)
                .and_then(|chain| chain.get(slot_idx))
                .map(|slot| {
                    let neural_value = selected_neural_neurons.and_then(|selection| {
                        sequencer::lisp_host::selected_neural_effect_plock_value(
                            &app.state, selection, track, slot_idx, param_idx,
                        )
                    });
                    let stored = neural_value
                        .or_else(|| {
                            display_step.and_then(|step| {
                                held_plock_value(&app.state, track, step, |s| {
                                    slot.plocks.get(s, param_idx)
                                })
                            })
                        })
                        .or_else(|| app.effective_slot_param_value(track, slot_idx, param_idx))
                        .unwrap_or_else(|| {
                            slot_param_stored_value(slot, pdesc, param_idx, display_step)
                        });
                    (pdesc.name.clone(), stored)
                })
        })
    {
        return reactive_set_needs_ui(rt.set_reactive(
            "SEQ",
            &track_effect_param_value_field(track, slot_idx, param_idx, &name),
            Value::Number(value as f64),
        ));
    }
    false
}

pub(crate) fn sync_midi_fx_param_value_field(
    rt: &mut Runtime,
    state: &Arc<SequencerState>,
    track: usize,
    slot_idx: usize,
    param_idx: usize,
    display_step: Option<usize>,
) -> bool {
    let chain = state.pattern.track_params[track].midi_fx_chain();
    if let Some((name, value)) = chain
        .get(slot_idx)
        .and_then(|fx_name| sequencer::lisp_host::load_midi_fx_descriptor(fx_name))
        .and_then(|desc| desc.params.get(param_idx).cloned())
        .and_then(|pdesc| {
            state
                .pattern
                .midi_fx_slots
                .get(track)
                .and_then(|slots| slots.get(slot_idx))
                .map(|slot| {
                    let stored = slot_param_stored_value(slot, &pdesc, param_idx, display_step);
                    (pdesc.name, stored)
                })
        })
    {
        return reactive_set_needs_ui(rt.set_reactive(
            "SEQ",
            &midi_fx_param_value_field(track, slot_idx, param_idx, &name),
            Value::Number(value as f64),
        ));
    }
    false
}

pub(crate) fn sync_bus_effect_param_value_field(
    rt: &mut Runtime,
    app: &app::App,
    bus_idx: usize,
    slot_idx: usize,
    param_idx: usize,
) -> bool {
    if let Some((name, value)) = app.buses.get(bus_idx).and_then(|bus| {
        bus.effect_descriptors
            .get(slot_idx)
            .and_then(|desc| desc.params.get(param_idx))
            .and_then(|pdesc| {
                bus.effect_slots.get(slot_idx).map(|slot| {
                    (
                        pdesc.name.clone(),
                        slot.defaults
                            .get(param_idx)
                            .copied()
                            .unwrap_or(pdesc.default),
                    )
                })
            })
    }) {
        return reactive_set_needs_ui(rt.set_reactive(
            "SEQ",
            &bus_effect_param_value_field(bus_idx, slot_idx, param_idx, &name),
            Value::Number(value as f64),
        ));
    }
    false
}

pub(crate) fn sync_fx_param_binding_fields(
    rt: &mut Runtime,
    app: &app::App,
    state: &Arc<SequencerState>,
    track: usize,
    selected_steps: &Arc<Mutex<HashSet<usize>>>,
) -> bool {
    sync_fx_param_binding_fields_with_neural_selection(rt, app, state, track, selected_steps, None)
}

pub(crate) fn sync_fx_param_binding_fields_with_neural_selection(
    rt: &mut Runtime,
    app: &app::App,
    state: &Arc<SequencerState>,
    track: usize,
    selected_steps: &Arc<Mutex<HashSet<usize>>>,
    selected_neural_neurons: Option<
        &std::collections::BTreeSet<sequencer::lisp_host::SelectedNeuralNeuron>,
    >,
) -> bool {
    let mut needs_ui = false;
    if track < app.tracks.len() {
        let selected_step = selected_plock_step(selected_steps);
        let display_step = displayed_plock_step(state, track, selected_step);
        needs_ui |= sync_rack_macro_value_fields(rt, app, track, display_step);
        needs_ui |= sync_rack_panel_param_value_fields(rt, app, track, display_step);
        needs_ui |= sync_fx_instrument_base_note_value_field(rt, app, track);
        needs_ui |= sync_sampler_selection_time_fields(rt, app, track, display_step);
        if let Some(desc) = app.graph.instrument_descriptors.get(track) {
            for (param_idx, pdesc) in desc.params.iter().enumerate() {
                if param_supports_value_binding(pdesc) {
                    needs_ui |= sync_fx_instrument_param_value_field_with_neural_selection(
                        rt,
                        app,
                        track,
                        param_idx,
                        display_step,
                        selected_neural_neurons,
                    );
                }
            }
            for tensor_idx in 0..desc.tensor_params.len() {
                needs_ui |= sync_fx_instrument_tensor_value_field(
                    rt,
                    app,
                    track,
                    tensor_idx,
                    display_step,
                );
            }
        }
        if let Some(slots) = app.graph.effect_descriptors.get(track) {
            for (slot_idx, desc) in slots.iter().enumerate() {
                for (param_idx, pdesc) in desc.params.iter().enumerate() {
                    if param_supports_value_binding(pdesc) {
                        needs_ui |= sync_track_effect_param_value_field_with_neural_selection(
                            rt,
                            app,
                            track,
                            slot_idx,
                            param_idx,
                            display_step,
                            selected_neural_neurons,
                        );
                    }
                }
            }
        }
        for (slot_idx, name) in state.pattern.track_params[track]
            .midi_fx_chain()
            .iter()
            .enumerate()
        {
            if let Some(desc) = sequencer::lisp_host::load_midi_fx_descriptor(name) {
                for (param_idx, pdesc) in desc.params.iter().enumerate() {
                    if param_supports_value_binding(pdesc) {
                        needs_ui |= sync_midi_fx_param_value_field(
                            rt,
                            state,
                            track,
                            slot_idx,
                            param_idx,
                            display_step,
                        );
                    }
                }
            }
        }
    }

    for (bus_idx, bus) in app.buses.iter().enumerate() {
        for (slot_idx, desc) in bus.effect_descriptors.iter().enumerate() {
            for (param_idx, pdesc) in desc.params.iter().enumerate() {
                if param_supports_value_binding(pdesc) {
                    needs_ui |=
                        sync_bus_effect_param_value_field(rt, app, bus_idx, slot_idx, param_idx);
                }
            }
        }
    }
    needs_ui
}

pub(crate) fn sync_track_playhead_field_delta(
    rt: &mut Runtime,
    state: &Arc<SequencerState>,
    app: &app::App,
    previous: &mut Vec<u32>,
) -> bool {
    let track_count = app.tracks.len();
    let mut current = Vec::with_capacity(track_count);
    let mut effects_dirty = false;
    let mut snapshot_changed = previous.len() != track_count;

    for t in 0..track_count {
        let playhead = state.transport.track_playheads[t].load(Ordering::Relaxed);
        let active_step = track_active_playhead_step(state, t);
        let active_row = active_step / PAGE_SIZE;
        let active_col = active_step % PAGE_SIZE;
        if let Some(prev_playhead) = previous.get(t).copied() {
            if prev_playhead != playhead {
                snapshot_changed = true;
                let num_steps = state.pattern.track_params[t]
                    .get_num_steps()
                    .max(1)
                    .min(MAX_STEPS);
                let prev_active_step = (prev_playhead as usize).min(num_steps.saturating_sub(1));
                let prev_active_row = prev_active_step / PAGE_SIZE;
                if prev_active_row != active_row {
                    effects_dirty |= rt
                        .set_reactive(
                            "SEQ",
                            &track_playhead_page_field(t),
                            Value::Number(active_row as f64),
                        )
                        .effects_dirty;
                    effects_dirty |= rt
                        .set_reactive(
                            "SEQ",
                            &track_playhead_row_field(t, prev_active_row),
                            Value::Number(-1.0),
                        )
                        .effects_dirty;
                    effects_dirty |= rt
                        .set_reactive(
                            "SEQ",
                            &track_playhead_row_active_field(t, prev_active_row),
                            Value::Bool(false),
                        )
                        .effects_dirty;
                }
                if prev_active_step != active_step {
                    effects_dirty |= rt
                        .set_reactive(
                            "SEQ",
                            &track_playhead_active_field(t, prev_active_step),
                            Value::Bool(false),
                        )
                        .effects_dirty;
                    effects_dirty |= rt
                        .set_reactive(
                            "SEQ",
                            &track_playhead_active_field(t, active_step),
                            Value::Bool(true),
                        )
                        .effects_dirty;
                    effects_dirty |= rt
                        .set_reactive(
                            "SEQ",
                            &track_playhead_row_field(t, active_row),
                            Value::Number(active_col as f64),
                        )
                        .effects_dirty;
                    effects_dirty |= rt
                        .set_reactive(
                            "SEQ",
                            &track_playhead_row_active_field(t, active_row),
                            Value::Bool(true),
                        )
                        .effects_dirty;
                }
            }
        } else {
            effects_dirty |= rt
                .set_reactive(
                    "SEQ",
                    &track_playhead_page_field(t),
                    Value::Number(active_row as f64),
                )
                .effects_dirty;
            effects_dirty |= rt
                .set_reactive(
                    "SEQ",
                    &track_playhead_active_field(t, active_step),
                    Value::Bool(true),
                )
                .effects_dirty;
            effects_dirty |= rt
                .set_reactive(
                    "SEQ",
                    &track_playhead_row_field(t, active_row),
                    Value::Number(active_col as f64),
                )
                .effects_dirty;
            effects_dirty |= rt
                .set_reactive(
                    "SEQ",
                    &track_playhead_row_active_field(t, active_row),
                    Value::Bool(true),
                )
                .effects_dirty;
        }
        current.push(playhead);
    }

    if snapshot_changed {
        *previous = current;
    }

    effects_dirty
}

pub(crate) fn sync_all_track_sequencer_state(
    rt: &mut Runtime,
    state: &Arc<SequencerState>,
    app: &app::App,
    current_track_idx: usize,
    selected_steps: &Arc<Mutex<HashSet<usize>>>,
) {
    sync_all_track_sequencer_state_inner(rt, state, app, current_track_idx, selected_steps, None);
}

pub(crate) fn sync_all_track_sequencer_state_profiled(
    rt: &mut Runtime,
    state: &Arc<SequencerState>,
    app: &app::App,
    current_track_idx: usize,
    selected_steps: &Arc<Mutex<HashSet<usize>>>,
) -> AllTrackSequencerSyncProfile {
    let mut profile = AllTrackSequencerSyncProfile::default();
    sync_all_track_sequencer_state_inner(
        rt,
        state,
        app,
        current_track_idx,
        selected_steps,
        Some(&mut profile),
    );
    profile
}

pub(super) fn sync_all_track_sequencer_state_inner(
    rt: &mut Runtime,
    state: &Arc<SequencerState>,
    app: &app::App,
    current_track_idx: usize,
    selected_steps: &Arc<Mutex<HashSet<usize>>>,
    mut profile: Option<&mut AllTrackSequencerSyncProfile>,
) {
    let total_started = profile.as_ref().map(|_| Instant::now());
    let started = profile.as_ref().map(|_| Instant::now());
    rt.set_reactive(
        "SEQ",
        "track-steps",
        build_all_track_steps_value(state, app),
    );
    if let Some(profile) = profile.as_deref_mut() {
        profile.track_steps = started.expect("profile timer").elapsed();
    }

    let started = profile.as_ref().map(|_| Instant::now());
    rt.set_reactive(
        "SEQ",
        "track-num-steps",
        build_all_track_num_steps_value(state, app),
    );
    if let Some(profile) = profile.as_deref_mut() {
        profile.track_num_steps = started.expect("profile timer").elapsed();
    }

    let started = profile.as_ref().map(|_| Instant::now());
    rt.set_reactive(
        "SEQ",
        "track-timebases",
        build_all_track_timebase_labels_value(state, app, current_track_idx, selected_steps),
    );
    if let Some(profile) = profile.as_deref_mut() {
        profile.track_timebases = started.expect("profile timer").elapsed();
    }

    let started = profile.as_ref().map(|_| Instant::now());
    rt.set_reactive(
        "SEQ",
        "track-duration-spans",
        build_all_track_duration_spans_value(state, app),
    );
    if let Some(profile) = profile.as_deref_mut() {
        profile.track_duration_spans = started.expect("profile timer").elapsed();
    }

    let started = profile.as_ref().map(|_| Instant::now());
    let plock_masks: Vec<[u64; MAX_STEPS / 64]> = (0..app.tracks.len())
        .map(|track| track_step_plock_mask(state, track, &app.graph.effect_descriptors))
        .collect();
    rt.set_reactive(
        "SEQ",
        "track-step-has-plocks",
        build_all_track_step_has_plocks_from_masks(&plock_masks),
    );
    rt.set_reactive(
        "SEQ",
        "track-step-plock-kinds",
        build_all_track_step_plock_kinds(state, app),
    );
    rt.set_reactive(
        "SEQ",
        "track-step-variant-r",
        build_all_track_step_variant_color_channel(state, app, 0),
    );
    rt.set_reactive(
        "SEQ",
        "track-step-variant-g",
        build_all_track_step_variant_color_channel(state, app, 1),
    );
    rt.set_reactive(
        "SEQ",
        "track-step-variant-b",
        build_all_track_step_variant_color_channel(state, app, 2),
    );
    if let Some(profile) = profile.as_deref_mut() {
        profile.track_step_has_plocks = started.expect("profile timer").elapsed();
    }

    let started = profile.as_ref().map(|_| Instant::now());
    rt.set_reactive(
        "SEQ",
        "track-playheads",
        build_all_track_playheads_value(state, app),
    );
    if let Some(profile) = profile.as_deref_mut() {
        profile.track_playheads = started.expect("profile timer").elapsed();
    }

    let started = profile.as_ref().map(|_| Instant::now());
    rt.set_reactive(
        "SEQ",
        "track-velocities",
        build_all_track_param_lists_value(state, app, StepParam::Velocity),
    );
    if let Some(profile) = profile.as_deref_mut() {
        profile.track_velocities = started.expect("profile timer").elapsed();
    }

    let started = profile.as_ref().map(|_| Instant::now());
    rt.set_reactive(
        "SEQ",
        "track-durations",
        build_all_track_param_lists_value(state, app, StepParam::Duration),
    );
    if let Some(profile) = profile.as_deref_mut() {
        profile.track_durations = started.expect("profile timer").elapsed();
    }

    let started = profile.as_ref().map(|_| Instant::now());
    rt.set_reactive(
        "SEQ",
        "track-auxas",
        build_all_track_param_lists_value(state, app, StepParam::AuxA),
    );
    if let Some(profile) = profile.as_deref_mut() {
        profile.track_auxas = started.expect("profile timer").elapsed();
    }

    let started = profile.as_ref().map(|_| Instant::now());
    rt.set_reactive(
        "SEQ",
        "track-transposes",
        build_all_track_param_lists_value(state, app, StepParam::Transpose),
    );
    sync_all_rack_slot_selection_binding_fields(rt, app);
    if let Some(profile) = profile.as_deref_mut() {
        profile.track_transposes = started.expect("profile timer").elapsed();
    }

    let started = profile.as_ref().map(|_| Instant::now());
    rt.set_reactive(
        "SEQ",
        "track-pans",
        build_all_track_param_lists_value(state, app, StepParam::Pan),
    );
    if let Some(profile) = profile.as_deref_mut() {
        profile.track_pans = started.expect("profile timer").elapsed();
    }

    let started = profile.as_ref().map(|_| Instant::now());
    rt.set_reactive(
        "SEQ",
        "track-syncs",
        build_all_track_param_lists_value(state, app, StepParam::Sync),
    );
    if let Some(profile) = profile.as_deref_mut() {
        profile.track_syncs = started.expect("profile timer").elapsed();
    }

    let started = profile.as_ref().map(|_| Instant::now());
    rt.set_reactive(
        "SEQ",
        "track-delays",
        build_all_track_param_lists_value(state, app, StepParam::Delay),
    );
    if let Some(profile) = profile.as_deref_mut() {
        profile.track_delays = started.expect("profile timer").elapsed();
    }
    rt.set_reactive(
        "SEQ",
        "track-process-lanes",
        build_all_track_process_lanes_value(state, app.tracks.len()),
    );

    if let Some(profile) = profile.as_deref_mut() {
        profile.step_bindings = sync_all_track_step_binding_fields_profiled(
            rt,
            state,
            app,
            current_track_idx,
            selected_steps,
            &plock_masks,
        );
    } else {
        sync_all_track_step_binding_fields(
            rt,
            state,
            app,
            current_track_idx,
            selected_steps,
            &plock_masks,
        );
    }

    let started = profile.as_ref().map(|_| Instant::now());
    sync_all_track_playhead_fields(rt, state, app);
    if let Some(profile) = profile.as_deref_mut() {
        profile.playhead_fields = started.expect("profile timer").elapsed();
        profile.elapsed = total_started.expect("profile timer").elapsed();
    }
}

/// Build a Lisp Value::List of floats for a given step param on a given track.
pub(crate) fn build_param_list(
    state: &Arc<SequencerState>,
    track: usize,
    param: StepParam,
) -> Value {
    let items: Vec<Rc<RefCell<Value>>> = (0..MAX_STEPS)
        .map(|s| {
            let val = state.pattern.step_data[track].get(s, param);
            Rc::new(RefCell::new(Value::Number(val as f64)))
        })
        .collect();
    Value::List(items)
}

pub(crate) fn fx_step_param_value_field(param: StepParam) -> Option<&'static str> {
    match param {
        StepParam::Velocity => Some("fx-step-value-velocity"),
        StepParam::Duration => Some("fx-step-value-duration"),
        StepParam::Transpose => Some("fx-step-value-transpose"),
        _ => None,
    }
}

pub(crate) fn fx_step_cursor_from_runtime(rt: &Runtime) -> usize {
    match rt.global_value("cursor-step") {
        Some(Value::Number(step)) if step >= 0.0 => step as usize,
        _ => 0,
    }
}

/// Refresh the fixed-size step-parameter strip without rerunning its Lisp
/// effect. Every consumer is a retained numeric binding, so cursor and
/// selection changes stay on the targeted widget path.
pub(crate) fn sync_fx_step_cursor_binding_fields(
    rt: &mut Runtime,
    state: &Arc<SequencerState>,
    track: usize,
    cursor_step: usize,
    selected_step: Option<usize>,
    selected_count: usize,
) -> bool {
    if track >= state.active_track_count() {
        return false;
    }
    let num_steps = state.pattern.track_params[track]
        .get_num_steps()
        .max(1)
        .min(MAX_STEPS);
    let cursor_step = cursor_step.min(num_steps.saturating_sub(1));
    let parameter_step = selected_step
        .unwrap_or(cursor_step)
        .min(num_steps.saturating_sub(1));
    let mut dirty = rt
        .set_reactive(
            "SEQ",
            "fx-step-cursor-number",
            Value::Number((cursor_step + 1) as f64),
        )
        .effects_dirty;
    dirty |= rt
        .set_reactive(
            "SEQ",
            "fx-step-selection-count",
            Value::Number(selected_count as f64),
        )
        .effects_dirty;
    dirty |= rt
        .set_reactive(
            "SEQ",
            "fx-step-parameter-step",
            Value::Number(parameter_step as f64),
        )
        .effects_dirty;
    for param in [StepParam::Velocity, StepParam::Duration, StepParam::Transpose] {
        let field = fx_step_param_value_field(param)
            .expect("step parameter strip field should exist");
        dirty |= rt
            .set_reactive(
                "SEQ",
                field,
                Value::Number(state.pattern.step_data[track].get(parameter_step, param) as f64),
            )
            .effects_dirty;
    }
    dirty
}

pub(crate) fn sync_step_param_lists(rt: &mut Runtime, state: &Arc<SequencerState>, track: usize) {
    rt.set_reactive(
        "SEQ",
        "velocities",
        build_param_list(state, track, StepParam::Velocity),
    );
    rt.set_reactive(
        "SEQ",
        "durations",
        build_param_list(state, track, StepParam::Duration),
    );
    rt.set_reactive(
        "SEQ",
        "transposes",
        build_param_list(state, track, StepParam::Transpose),
    );
    rt.set_reactive(
        "SEQ",
        "auxas",
        build_param_list(state, track, StepParam::AuxA),
    );
    rt.set_reactive(
        "SEQ",
        "pans",
        build_param_list(state, track, StepParam::Pan),
    );
    rt.set_reactive(
        "SEQ",
        "syncs",
        build_param_list(state, track, StepParam::Sync),
    );
    rt.set_reactive(
        "SEQ",
        "delays",
        build_param_list(state, track, StepParam::Delay),
    );
    rt.set_reactive(
        "SEQ",
        "track-velocities",
        build_all_active_track_param_lists_value(state, StepParam::Velocity),
    );
    rt.set_reactive(
        "SEQ",
        "track-durations",
        build_all_active_track_param_lists_value(state, StepParam::Duration),
    );
    rt.set_reactive(
        "SEQ",
        "track-transposes",
        build_all_active_track_param_lists_value(state, StepParam::Transpose),
    );
    rt.set_reactive(
        "SEQ",
        "track-auxas",
        build_all_active_track_param_lists_value(state, StepParam::AuxA),
    );
    rt.set_reactive(
        "SEQ",
        "track-pans",
        build_all_active_track_param_lists_value(state, StepParam::Pan),
    );
    rt.set_reactive(
        "SEQ",
        "track-syncs",
        build_all_active_track_param_lists_value(state, StepParam::Sync),
    );
    rt.set_reactive(
        "SEQ",
        "track-delays",
        build_all_active_track_param_lists_value(state, StepParam::Delay),
    );
    sync_process_chain_state(rt, state, state.active_track_count(), track);
}

pub(crate) fn build_accumulator_names(app: &app::App) -> Vec<String> {
    let mut names = BUILTIN_ACCUMULATOR_NAMES
        .iter()
        .map(|name| (*name).to_string())
        .collect::<Vec<_>>();
    if let Some(runtime) = app.editor.scratch_runtime.as_ref() {
        names.extend(runtime.accumulator_names());
    }
    names
}

pub(crate) fn build_accumulator_options(app: &app::App) -> Value {
    let items = build_accumulator_names(app)
        .into_iter()
        .map(|name| Rc::new(RefCell::new(Value::String(name))))
        .collect();
    Value::List(items)
}

pub(crate) fn build_accum_mode_options() -> Value {
    let items = ACCUM_MODE_LABELS
        .iter()
        .map(|label| Rc::new(RefCell::new(Value::String((*label).to_string()))))
        .collect();
    Value::List(items)
}

pub(crate) fn build_fts_options() -> Value {
    let items = FTS_SCALE_NAMES
        .iter()
        .map(|scale| Rc::new(RefCell::new(Value::String((*scale).to_string()))))
        .collect();
    Value::List(items)
}

pub(crate) fn mute_group_label(group: u8) -> String {
    match group.min(8) {
        0 => "Off".to_string(),
        group => group.to_string(),
    }
}

pub(crate) fn build_mute_group_options() -> Value {
    let items = std::iter::once("Off".to_string())
        .chain((1..=8).map(|group| group.to_string()))
        .map(|label| Rc::new(RefCell::new(Value::String(label))))
        .collect();
    Value::List(items)
}

pub(crate) fn builtin_accumulator_default_limit(idx: usize) -> f32 {
    match idx {
        1 => 48.0,
        2 => 1.0,
        _ => 0.0,
    }
}

pub(crate) fn accum_mode_label(mode: u32) -> &'static str {
    ACCUM_MODE_LABELS
        .get(mode as usize)
        .copied()
        .unwrap_or(ACCUM_MODE_LABELS[0])
}

pub(crate) fn selected_accumulator_name(app: &app::App, track: usize) -> String {
    let tp = &app.state.pattern.track_params[track];
    if let Some(name) = tp.script_accumulator_name() {
        return name;
    }
    build_accumulator_names(app)
        .get(tp.get_accumulator_idx())
        .cloned()
        .unwrap_or_else(|| "Off".to_string())
}
