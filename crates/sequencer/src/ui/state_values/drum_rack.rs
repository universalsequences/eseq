use super::*;

pub(super) fn rack_slot_type_name(slot: &sequencer::sequencer::RackSlotSnapshot) -> &'static str {
    match slot.instrument_type {
        sequencer::sequencer::InstrumentType::Sampler => "sampler",
        sequencer::sequencer::InstrumentType::Custom => "custom",
        sequencer::sequencer::InstrumentType::Modulator => "modulator",
        sequencer::sequencer::InstrumentType::Rack => "rack",
    }
}

pub(super) fn rack_slot_raw_name(
    app: &app::App,
    slot_idx: usize,
    slot: &sequencer::sequencer::RackSlotSnapshot,
) -> String {
    match slot.instrument_type {
        sequencer::sequencer::InstrumentType::Sampler => slot
            .sample_id
            .as_ref()
            .map(|(_, name, _)| name.clone())
            .filter(|name| !name.is_empty())
            .unwrap_or_else(|| format!("Sampler {}", slot_idx + 1)),
        sequencer::sequencer::InstrumentType::Custom
        | sequencer::sequencer::InstrumentType::Modulator => slot
            .track_sound_state
            .engine_id
            .and_then(|engine_id| app.editor.engine_registry.get(engine_id))
            .map(|engine| engine.name.clone())
            .or_else(|| slot.track_sound_state.loaded_preset.clone())
            .unwrap_or_else(|| format!("Instrument {}", slot_idx + 1)),
        sequencer::sequencer::InstrumentType::Rack => format!("Unsupported {}", slot_idx + 1),
    }
}

pub(super) fn drum_rack_pad_label(pad_note: i32) -> String {
    let name = match pad_note.rem_euclid(12) {
        0 => "C",
        1 => "C#",
        2 => "D",
        3 => "D#",
        4 => "E",
        5 => "F",
        6 => "F#",
        7 => "G",
        8 => "G#",
        9 => "A",
        10 => "A#",
        _ => "B",
    };
    format!("{name}{}", 4 + pad_note.div_euclid(12))
}

pub(super) fn drum_rack_pad_bank_label(bank_start: i32) -> String {
    let bank_end = (bank_start + sequencer::sequencer::DRUM_RACK_PAD_COUNT as i32 - 1)
        .min(sequencer::sequencer::DRUM_RACK_LAST_PAD_NOTE);
    format!(
        "{} - {}",
        drum_rack_pad_label(bank_start),
        drum_rack_pad_label(bank_end)
    )
}

pub(super) fn drum_rack_uses_pad_notes(app: &app::App, track: usize) -> bool {
    app.state
        .pattern
        .rack_tracks
        .lock()
        .unwrap()
        .get(track)
        .and_then(Option::as_ref)
        .is_some_and(|rack| rack.routing == sequencer::sequencer::RackRouting::ByPitch)
}

pub(crate) fn build_track_drum_racks_value(app: &app::App) -> Value {
    Value::List(
        (0..app.tracks.len())
            .map(|track| value_cell(Value::Bool(drum_rack_uses_pad_notes(app, track))))
            .collect(),
    )
}

/// The sequencer treats a drum-rack transpose as the pad note that routes to
/// a slot. Keep this display mapping in one place so the pad grid, expanded
/// sequencer, compact sequencer, and step inspector name the same sound.
pub(crate) struct DrumRackSoundOption {
    pub(crate) pad_note: i32,
    slot_idx: usize,
    name: String,
    pad_label: String,
    label: String,
    short_label: String,
}

pub(super) fn drum_rack_sound_short_label(name: &str) -> String {
    let letters = name
        .chars()
        .filter(|character| character.is_alphabetic())
        .take(3)
        .collect::<String>();
    if letters.is_empty() {
        name.chars()
            .filter(|character| !character.is_whitespace())
            .take(3)
            .collect()
    } else {
        letters
    }
}

pub(crate) fn drum_rack_sound_options(app: &app::App, track: usize) -> Vec<DrumRackSoundOption> {
    let rack = app
        .state
        .pattern
        .rack_tracks
        .lock()
        .unwrap()
        .get(track)
        .cloned()
        .flatten();
    let Some(rack) = rack else {
        return Vec::new();
    };
    if rack.routing != sequencer::sequencer::RackRouting::ByPitch {
        return Vec::new();
    }
    let mut options = rack
        .slots
        .iter()
        .enumerate()
        .filter_map(|(slot_idx, slot)| {
            let pad_note = slot.pad_note?;
            let name = instrument_display_name(&rack_slot_raw_name(app, slot_idx, slot));
            let pad_label = drum_rack_pad_label(pad_note);
            Some(DrumRackSoundOption {
                pad_note,
                slot_idx,
                label: format!("{pad_label} · {name}"),
                short_label: drum_rack_sound_short_label(&name),
                name,
                pad_label,
            })
        })
        .collect::<Vec<_>>();
    options.sort_by_key(|sound| sound.pad_note);
    options
}

pub(super) fn drum_rack_sound_value(track: usize, option: DrumRackSoundOption) -> Rc<RefCell<Value>> {
    let mut value = HashMap::new();
    value.insert(
        "transpose".to_string(),
        value_cell(Value::Number(option.pad_note as f64)),
    );
    value.insert(
        "slot-idx".to_string(),
        value_cell(Value::Number(option.slot_idx as f64)),
    );
    insert_string_prop(
        &mut value,
        "gain-field",
        rack_slot_value_field(
            track,
            option.slot_idx,
            sequencer::sequencer::RackSlotParam::Gain,
        ),
    );
    insert_string_prop(
        &mut value,
        "mute-field",
        rack_slot_value_field(
            track,
            option.slot_idx,
            sequencer::sequencer::RackSlotParam::Mute,
        ),
    );
    insert_string_prop(
        &mut value,
        "solo-field",
        rack_slot_value_field(
            track,
            option.slot_idx,
            sequencer::sequencer::RackSlotParam::Solo,
        ),
    );
    insert_string_prop(
        &mut value,
        "peak-field",
        rack_slot_peak_field(track, option.slot_idx),
    );
    insert_string_prop(
        &mut value,
        "selected-field",
        rack_slot_selected_field(track, option.slot_idx),
    );
    insert_string_prop(&mut value, "name", option.name);
    insert_string_prop(&mut value, "pad-label", option.pad_label);
    insert_string_prop(&mut value, "label", option.label);
    insert_string_prop(&mut value, "short-label", option.short_label);
    Rc::new(RefCell::new(Value::Map(value)))
}

pub(crate) fn build_all_track_drum_sounds_value(app: &app::App) -> Value {
    Value::List(
        (0..app.tracks.len())
            .map(|track| {
                let sounds = drum_rack_sound_options(app, track)
                    .into_iter()
                    .map(|option| drum_rack_sound_value(track, option))
                    .collect();
                value_cell(Value::List(sounds))
            })
            .collect(),
    )
}

/// Publishes the rack's global slot selection for drum-lane widgets without
/// rebuilding the sequencer tree. The selected identity for a drum rack is a
/// pad note, so resolve it through the rack snapshot before lighting a slot.
pub(crate) fn sync_all_rack_slot_selection_binding_fields(
    rt: &mut Runtime,
    app: &app::App,
) -> bool {
    let racks = app.state.pattern.rack_tracks.lock().unwrap();
    let mut dirty = false;
    for (track, rack) in racks.iter().enumerate() {
        let Some(rack) = rack.as_ref() else {
            continue;
        };
        let selected_slot = app.selected_rack_slot_index_for_rack(track, rack);
        for slot_idx in 0..rack.slots.len() {
            dirty |= rt
                .set_reactive(
                    "SEQ",
                    &rack_slot_selected_field(track, slot_idx),
                    Value::Bool(Some(slot_idx) == selected_slot),
                )
                .effects_dirty;
        }
    }
    dirty
}

pub(crate) fn drum_lane_step_active(
    state: &Arc<SequencerState>,
    track: usize,
    pad_note: i32,
    step: usize,
) -> bool {
    if track >= state.pattern.patterns.len()
        || step >= MAX_STEPS
        || !state.pattern.patterns[track].is_active(step)
    {
        return false;
    }
    let count = state.pattern.chord_data[track].count(step);
    if count == 0 {
        return state.pattern.step_data[track]
            .get(step, StepParam::Transpose)
            .round() as i32
            == pad_note;
    }
    (0..count)
        .any(|voice| state.pattern.chord_data[track].get(step, voice).round() as i32 == pad_note)
}

pub(crate) fn drum_lane_step_duration_covered(
    state: &Arc<SequencerState>,
    track: usize,
    pad_note: i32,
    target_step: usize,
) -> bool {
    if track >= state.pattern.patterns.len() || target_step >= MAX_STEPS {
        return false;
    }
    let num_steps = state.pattern.track_params[track]
        .get_num_steps()
        .min(MAX_STEPS);
    target_step < num_steps
        && (0..=target_step).any(|source_step| {
            state
                .drum_lane_step_duration(track, source_step, pad_note)
                .is_some_and(|duration| duration > (target_step - source_step) as f32)
        })
}

pub(crate) fn sync_drum_lane_step_binding_fields(
    rt: &mut Runtime,
    state: &Arc<SequencerState>,
    app: &app::App,
    track: usize,
    only_step: Option<usize>,
) -> bool {
    if track >= app.tracks.len() {
        return false;
    }
    let sounds = drum_rack_sound_options(app, track);
    if sounds.is_empty() {
        return false;
    }
    let mut registered_fields = match rt.global_value("SEQ") {
        Some(Value::Map(fields)) => fields.keys().cloned().collect::<HashSet<_>>(),
        _ => HashSet::new(),
    };
    let steps = only_step.map_or(0..MAX_STEPS, |step| {
        step..step.saturating_add(1).min(MAX_STEPS)
    });
    let mut dirty = false;
    for sound in sounds {
        for step in steps.clone() {
            let selected_field = drum_lane_step_selected_field(track, sound.pad_note, step);
            if registered_fields.insert(selected_field.clone()) {
                dirty |= rt
                    .set_reactive("SEQ", &selected_field, Value::Bool(false))
                    .effects_dirty;
            }
            let duration_field = drum_lane_step_duration_field(track, sound.pad_note, step);
            dirty |= rt
                .set_reactive(
                    "SEQ",
                    &duration_field,
                    Value::Bool(drum_lane_step_duration_covered(
                        state,
                        track,
                        sound.pad_note,
                        step,
                    )),
                )
                .effects_dirty;
            dirty |= rt
                .set_reactive(
                    "SEQ",
                    &drum_lane_step_active_field(track, sound.pad_note, step),
                    Value::Bool(drum_lane_step_active(state, track, sound.pad_note, step)),
                )
                .effects_dirty;
        }
    }
    dirty
}

pub(crate) fn sync_drum_lane_step_binding_fields_for_steps(
    rt: &mut Runtime,
    state: &Arc<SequencerState>,
    app: &app::App,
    track: usize,
    steps: &[usize],
) -> bool {
    if track >= app.tracks.len() || steps.is_empty() {
        return false;
    }
    let sounds = drum_rack_sound_options(app, track);
    if sounds.is_empty() {
        return false;
    }
    let mut registered_fields = match rt.global_value("SEQ") {
        Some(Value::Map(fields)) => fields.keys().cloned().collect::<HashSet<_>>(),
        _ => HashSet::new(),
    };
    let mut dirty = false;
    for sound in sounds {
        for &step in steps.iter().filter(|step| **step < MAX_STEPS) {
            let selected_field = drum_lane_step_selected_field(track, sound.pad_note, step);
            if registered_fields.insert(selected_field.clone()) {
                dirty |= rt
                    .set_reactive("SEQ", &selected_field, Value::Bool(false))
                    .effects_dirty;
            }
            dirty |= rt
                .set_reactive(
                    "SEQ",
                    &drum_lane_step_duration_field(track, sound.pad_note, step),
                    Value::Bool(drum_lane_step_duration_covered(
                        state,
                        track,
                        sound.pad_note,
                        step,
                    )),
                )
                .effects_dirty;
            dirty |= rt
                .set_reactive(
                    "SEQ",
                    &drum_lane_step_active_field(track, sound.pad_note, step),
                    Value::Bool(drum_lane_step_active(state, track, sound.pad_note, step)),
                )
                .effects_dirty;
        }
    }
    dirty
}

pub(crate) fn sync_all_drum_lane_step_binding_fields(
    rt: &mut Runtime,
    state: &Arc<SequencerState>,
    app: &app::App,
) -> bool {
    let mut dirty = false;
    for track in 0..app.tracks.len() {
        dirty |= sync_drum_lane_step_binding_fields(rt, state, app, track, None);
    }
    dirty
}

pub(super) fn drum_rack_pad_bank_value(bank_start: i32, selected_bank_start: i32) -> Rc<RefCell<Value>> {
    let mut bank_map: HashMap<String, Rc<RefCell<Value>>> = HashMap::new();
    bank_map.insert(
        "bank-start".to_string(),
        value_cell(Value::Number(bank_start as f64)),
    );
    bank_map.insert(
        "selected".to_string(),
        value_cell(Value::Bool(bank_start == selected_bank_start)),
    );
    insert_string_prop(&mut bank_map, "label", drum_rack_pad_bank_label(bank_start));
    Rc::new(RefCell::new(Value::Map(bank_map)))
}

pub(super) fn rack_pad_value(
    app: &app::App,
    track: usize,
    rack: &sequencer::sequencer::RackTrackSnapshot,
    pad_note: i32,
    selected_pad_note: i32,
) -> Rc<RefCell<Value>> {
    let slot_idx = rack
        .slots
        .iter()
        .position(|slot| slot.pad_note == Some(pad_note));
    let mut pad_map: HashMap<String, Rc<RefCell<Value>>> = HashMap::new();
    pad_map.insert("track".to_string(), value_cell(Value::Number(track as f64)));
    pad_map.insert(
        "pad-note".to_string(),
        value_cell(Value::Number(pad_note as f64)),
    );
    pad_map.insert(
        "selected".to_string(),
        value_cell(Value::Bool(pad_note == selected_pad_note)),
    );
    insert_string_prop(&mut pad_map, "label", drum_rack_pad_label(pad_note));
    if let Some(slot_idx) = slot_idx {
        let slot = &rack.slots[slot_idx];
        pad_map.insert(
            "slot".to_string(),
            value_cell(Value::Number(slot_idx as f64)),
        );
        pad_map.insert(
            "idx".to_string(),
            value_cell(Value::Number(slot_idx as f64)),
        );
        pad_map.insert("occupied".to_string(), value_cell(Value::Bool(true)));
        insert_string_prop(&mut pad_map, "type", rack_slot_type_name(slot));
        let raw_name = rack_slot_raw_name(app, slot_idx, slot);
        insert_string_prop(&mut pad_map, "name", raw_name.clone());
        insert_string_prop(
            &mut pad_map,
            "display-name",
            instrument_display_name(&raw_name),
        );
        pad_map.insert("mute".to_string(), value_cell(Value::Bool(slot.mute)));
        pad_map.insert("solo".to_string(), value_cell(Value::Bool(slot.solo)));
        pad_map.insert(
            "choke-group".to_string(),
            value_cell(Value::Number(slot.choke_group.unwrap_or(0) as f64)),
        );
    } else {
        pad_map.insert("occupied".to_string(), value_cell(Value::Bool(false)));
        pad_map.insert("slot".to_string(), value_cell(Value::Number(-1.0)));
        pad_map.insert("idx".to_string(), value_cell(Value::Number(-1.0)));
        insert_string_prop(&mut pad_map, "type", "empty");
        insert_string_prop(&mut pad_map, "name", "");
        insert_string_prop(&mut pad_map, "display-name", "");
        pad_map.insert("mute".to_string(), value_cell(Value::Bool(false)));
        pad_map.insert("solo".to_string(), value_cell(Value::Bool(false)));
        pad_map.insert("choke-group".to_string(), value_cell(Value::Number(0.0)));
    }
    Rc::new(RefCell::new(Value::Map(pad_map)))
}
