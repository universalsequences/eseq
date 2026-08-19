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

/// Publishes the rack's global slot selection for the rack panel without
/// rebuilding the sequencer tree.
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
