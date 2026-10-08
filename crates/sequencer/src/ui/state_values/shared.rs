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

/// An unresolved options reference as the UI's degrade path reads it
/// (`:tensor`, `:file`, `:key`, `:asset-base`): the host kinds'
/// `param.asset-options`.
pub(crate) fn param_asset_options_value(options: &sequencer::effects::ParamAssetOptions) -> Value {
    let mut option_map = HashMap::new();
    insert_string_prop(&mut option_map, "tensor", &options.tensor);
    insert_string_prop(&mut option_map, "file", &options.file);
    option_map.insert(
        "key".to_string(),
        Rc::new(RefCell::new(Value::Keyword(options.key.clone()))),
    );
    if let Some(asset_base) = &options.asset_base {
        insert_string_prop(&mut option_map, "asset-base", asset_base.to_string_lossy());
    }
    Value::Map(option_map)
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

/// The p-lock value audibly in force at `step` for one parameter, honoring
/// off-step p-lock hold semantics: the step's own p-lock wins; otherwise, on
/// an OFF step, walk back (wrap-aware) to the nearest preceding p-lock — an
/// off-step p-lock is track-level automation that holds until the next p-lock
/// or the next ON trigger's full param stamp, so a triggered step without its
/// own p-lock ends the walk and the base value is in force. Mirrors the
/// scheduler's off-step p-lock application, so the knob shows what is heard.
///
/// `has_any` is an O(1) gate for the whole p-lock table the closure reads: when
/// the table holds no p-lock at all the walk can only return `None`, so skip it
/// rather than scanning every step (this runs per parameter on the meter poll).
pub(crate) fn held_plock_value(
    state: &SequencerState,
    track: usize,
    step: usize,
    has_any: bool,
    mut plock_at: impl FnMut(usize) -> Option<f32>,
) -> Option<f32> {
    if !has_any {
        return None;
    }
    let num_steps = state
        .pattern
        .track_params
        .get(track)?
        .get_num_steps()
        .clamp(1, sequencer::sequencer::MAX_STEPS);
    let pattern = state.pattern.patterns.get(track)?;
    let mut s = step % num_steps;
    for _ in 0..num_steps {
        if let Some(value) = plock_at(s) {
            return Some(value);
        }
        if pattern.is_active(s) {
            return None;
        }
        s = if s == 0 { num_steps - 1 } else { s - 1 };
    }
    None
}

/// What a device param shows at `display_step`, in stored units, and whether
/// a p-lock supplies it: the p-lock in force there ([`held_plock_value`]),
/// else `effective` (the base under any engaged macro override; `None` past
/// the slot's live params), else the slot's stored base. Shared by the legacy
/// `track-N-instrument-param-*` / `track-N-fx-S-param-*` publishers and the
/// host kinds' `param.value` / `param.locked`.
pub(crate) fn device_param_display(
    state: &SequencerState,
    track: usize,
    slot: &sequencer::effects::EffectSlotState,
    pdesc: &sequencer::effects::ParamDescriptor,
    param_idx: usize,
    display_step: Option<usize>,
    effective: Option<f32>,
) -> (f32, bool) {
    let lock = display_step.and_then(|step| {
        held_plock_value(state, track, step, slot.plocks.has_any_plock(), |s| {
            slot.plocks.get(s, param_idx)
        })
    });
    match lock {
        Some(value) => (value, true),
        None => (
            effective.unwrap_or_else(|| slot_param_stored_value(slot, pdesc, param_idx, None)),
            false,
        ),
    }
}

pub(crate) fn slot_param_stored_value(
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

/// The roll rate's label (`transport.roll-rate`) from the
/// transport's `roll_rate` atomic.
pub(crate) fn roll_rate_label(raw: u32) -> &'static str {
    sequencer::sequencer::Timebase::from_index(raw).label()
}

/// The position of the track `id` names now; `None` once it is gone (or
/// past the active tracks). Commands addressed by `TrackId` resolve their
/// track with it when they land.
pub(crate) fn live_track_index(app: &app::App, id: sequencer::sequencer::TrackId) -> Option<usize> {
    let track = app.track_registry.index_of(id)?;
    (track < app.state.active_track_count()).then_some(track)
}
