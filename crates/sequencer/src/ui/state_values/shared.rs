use super::*;

pub(super) fn field_safe_name(name: &str) -> String {
    name.chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '_' || ch == '-' {
                ch
            } else {
                '_'
            }
        })
        .collect()
}

pub(crate) fn instrument_param_value_field(track: usize, param_idx: usize, name: &str) -> String {
    format!(
        "track-{track}-instrument-param-{param_idx}-{}",
        field_safe_name(name)
    )
}

pub(crate) fn fx_instrument_param_value_field(param_idx: usize, name: &str) -> String {
    format!(
        "fx-instrument-param-{param_idx}-{}",
        field_safe_name(name)
    )
}

pub(crate) fn instrument_tensor_value_field(track: usize, tensor_idx: usize, name: &str) -> String {
    format!(
        "track-{track}-instrument-tensor-{tensor_idx}-{}",
        field_safe_name(name)
    )
}

pub(crate) fn fx_instrument_tensor_value_field(tensor_idx: usize, name: &str) -> String {
    format!(
        "fx-instrument-tensor-{tensor_idx}-{}",
        field_safe_name(name)
    )
}

pub(crate) fn sampler_selection_time_field(track: usize, marker: &str) -> String {
    format!("track-{track}-sampler-selection-{marker}-time")
}

pub(crate) fn rack_slot_sampler_selection_time_field(
    track: usize,
    slot_idx: usize,
    marker: &str,
) -> String {
    format!("track-{track}-rack-slot-{slot_idx}-sampler-selection-{marker}-time")
}

pub(crate) fn instrument_base_note_value_field(track: usize) -> String {
    format!("track-{track}-instrument-base-note")
}

pub(crate) fn fx_instrument_base_note_value_field() -> &'static str {
    "fx-instrument-base-note"
}

pub(crate) fn rack_macro_value_field(track: usize, macro_idx: usize) -> String {
    format!("track-{track}-rack-macro-{macro_idx}")
}

pub(crate) fn rack_macro_plock_active_field(track: usize, macro_idx: usize) -> String {
    format!("track-{track}-rack-macro-{macro_idx}-plock-active")
}

pub(crate) fn rack_macro_plock_default_field(track: usize, macro_idx: usize) -> String {
    format!("track-{track}-rack-macro-{macro_idx}-plock-default")
}

pub(crate) fn rack_slot_value_field(
    track: usize,
    slot_idx: usize,
    param: sequencer::sequencer::RackSlotParam,
) -> String {
    format!("track-{track}-rack-slot-{slot_idx}-{}", param.name())
}

pub(crate) fn rack_slot_selected_field(track: usize, slot_idx: usize) -> String {
    format!("track-{track}-rack-slot-{slot_idx}-selected")
}

pub(crate) fn rack_slot_instrument_param_value_field(
    track: usize,
    slot_idx: usize,
    param_idx: usize,
    name: &str,
) -> String {
    format!(
        "track-{track}-rack-slot-{slot_idx}-instrument-param-{param_idx}-{}",
        field_safe_name(name)
    )
}

pub(crate) fn rack_slot_effect_param_value_field(
    track: usize,
    slot_idx: usize,
    effect_slot: usize,
    param_idx: usize,
    name: &str,
) -> String {
    format!(
        "track-{track}-rack-slot-{slot_idx}-fx-{effect_slot}-param-{param_idx}-{}",
        field_safe_name(name)
    )
}

pub(crate) fn track_effect_param_value_field(
    track: usize,
    slot_idx: usize,
    param_idx: usize,
    name: &str,
) -> String {
    format!(
        "track-{track}-fx-{slot_idx}-param-{param_idx}-{}",
        field_safe_name(name)
    )
}

pub(crate) fn midi_fx_param_value_field(
    track: usize,
    slot_idx: usize,
    param_idx: usize,
    name: &str,
) -> String {
    format!(
        "track-{track}-midi-fx-{slot_idx}-param-{param_idx}-{}",
        field_safe_name(name)
    )
}

pub(crate) fn bus_effect_param_value_field(
    bus_idx: usize,
    slot_idx: usize,
    param_idx: usize,
    name: &str,
) -> String {
    format!(
        "bus-{bus_idx}-fx-{slot_idx}-param-{param_idx}-{}",
        field_safe_name(name)
    )
}

pub(super) fn insert_string_prop(
    map: &mut HashMap<String, Rc<RefCell<Value>>>,
    key: &str,
    value: impl Into<String>,
) {
    map.insert(
        key.to_string(),
        Rc::new(RefCell::new(Value::String(value.into()))),
    );
}

pub(super) fn insert_param_ui_metadata(
    map: &mut HashMap<String, Rc<RefCell<Value>>>,
    metadata: Option<&sequencer::effects::ParamUiMetadata>,
) {
    let Some(metadata) = metadata else { return };
    if let Some(group) = &metadata.group {
        insert_string_prop(map, "group", group);
    }
    if let Some(env) = &metadata.env {
        insert_string_prop(map, "env", env);
    }
    if let Some(role) = &metadata.role {
        insert_string_prop(map, "role", role);
    }
    if let Some(display_name) = &metadata.display_name {
        insert_string_prop(map, "display-name", display_name);
    }
    if let Some(options) = &metadata.asset_options {
        let mut option_map = HashMap::new();
        insert_string_prop(&mut option_map, "tensor", &options.tensor);
        insert_string_prop(&mut option_map, "file", &options.file);
        option_map.insert(
            "key".to_string(),
            Rc::new(RefCell::new(Value::Keyword(options.key.clone()))),
        );
        if let Some(asset_base) = &options.asset_base {
            insert_string_prop(
                &mut option_map,
                "asset-base",
                asset_base.to_string_lossy(),
            );
        }
        map.insert(
            "options".to_string(),
            Rc::new(RefCell::new(Value::Map(option_map))),
        );
    }
}

pub(super) fn instrument_slot_param_value(
    slot: &sequencer::effects::EffectSlotState,
    desc: &sequencer::effects::EffectDescriptor,
    param_idx: usize,
    plock_step: Option<usize>,
) -> f32 {
    plock_step
        .and_then(|step| slot.plocks.get(step, param_idx))
        .unwrap_or_else(|| {
            if param_idx < slot.num_params.load(Ordering::Relaxed) as usize {
                slot.defaults.get(param_idx)
            } else {
                desc.params
                    .get(param_idx)
                    .map(|param| param.default)
                    .unwrap_or_default()
            }
        })
}

pub(super) fn selected_voice_mod_source_indices(
    desc: &sequencer::effects::EffectDescriptor,
    slot: &sequencer::effects::EffectSlotState,
    plock_step: Option<usize>,
) -> Vec<usize> {
    sequencer::instruments::voice_modulator::selected_source_param_indices(&desc.params, |idx, _| {
        instrument_slot_param_value(slot, desc, idx, plock_step)
    })
}

pub(super) fn selected_voice_mod_source_indices_for_optional_slot(
    desc: &sequencer::effects::EffectDescriptor,
    slot: Option<&sequencer::effects::EffectSlotState>,
    plock_step: Option<usize>,
) -> Vec<usize> {
    if let Some(slot) = slot {
        return selected_voice_mod_source_indices(desc, slot, plock_step);
    }
    sequencer::instruments::voice_modulator::selected_source_param_indices(&desc.params, |_, param| {
        param.default
    })
}

pub(super) fn param_supports_value_binding(pdesc: &sequencer::effects::ParamDescriptor) -> bool {
    matches!(pdesc.kind, sequencer::effects::ParamKind::Continuous { .. })
        || matches!(pdesc.kind, sequencer::effects::ParamKind::Enum { .. })
        || pdesc.name.eq_ignore_ascii_case("enabled")
}

pub(super) fn slot_param_stored_value(
    slot: &sequencer::effects::EffectSlotState,
    pdesc: &sequencer::effects::ParamDescriptor,
    param_idx: usize,
    display_step: Option<usize>,
) -> f32 {
    display_step
        .and_then(|step| slot.plocks.get(step, param_idx))
        .unwrap_or_else(|| {
            if param_idx < slot.num_params.load(Ordering::Relaxed) as usize {
                slot.defaults.get(param_idx)
            } else {
                pdesc.default
            }
        })
}

pub(super) fn reactive_set_needs_ui(result: eseqlisp::runtime::ReactiveSetResult) -> bool {
    result.effects_dirty || result.widgets_dirty
}
