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

// ── Pad trigger lights (eseq-4b5.16) ────────────────────────────────────
// A pad lights while it sounds, whatever fired it: a pad-grid click, an
// armed rack's live keys, or the member track's own sequenced steps. All
// three land on the pad's MEMBER TRACK, so the light is a per-member-track
// signal — the grid cell and the mini-map cell both read the same one, which
// is what lets a pad on a page the grid is not showing still flash in the map.

/// How long a pad stays lit after the trigger that lit it. The light is a
/// decay anchored on the trigger, not a mirror of the note's gate: a clipped
/// hi-hat's gate can be shorter than a UI frame and would otherwise flash for
/// zero frames, while a held pad key keeps refreshing this and stays lit.
pub(crate) const RACK_PAD_TRIGGER_HOLD: Duration = Duration::from_millis(140);

/// Binding field a pad cell reads: `1.0` while the pad's member track is lit.
/// Per track, not per (rack, pad note), so moving a pad to another note moves
/// its light with it for free.
pub(crate) fn rack_pad_trigger_field(track: usize) -> String {
    format!("rack-pad-trigger-{track}")
}

/// Pad lights for every track, `false` everywhere outside a drum rack.
///
/// Two sources feed it. `trigger_flash` is the audio thread's per-track latch,
/// set by every trigger path there is (sequenced steps, live keyboard, rack
/// slots) and read by nobody until now — consuming it is what makes the light
/// miss-free, since a hit that fell entirely between two UI frames still left
/// the latch set. Active-note activity then HOLDS the light for as long as the
/// note actually sounds, so a held pad key stays lit rather than blinking once.
pub(crate) fn read_rack_pad_trigger_flags(
    app: &app::App,
    state: &Arc<SequencerState>,
    track_active_notes: &[Vec<sequencer::sequencer::ActiveNoteActivity>],
    triggered_at: &mut Vec<Option<Instant>>,
    now: Instant,
) -> Vec<bool> {
    let track_count = app.tracks.len();
    triggered_at.resize(track_count, None);
    let mut flags = vec![false; track_count];
    for group in app.groups.iter().filter(|group| group.is_rack()) {
        for &track in &group.members {
            if track >= track_count {
                continue;
            }
            let latched = state
                .transport
                .trigger_flash
                .get(track)
                .is_some_and(|flash| flash.swap(0, Ordering::Relaxed) != 0);
            let sounding = track_active_notes
                .get(track)
                .is_some_and(|notes| !notes.is_empty());
            if latched || sounding {
                triggered_at[track] = Some(now);
            }
            flags[track] = triggered_at[track]
                .is_some_and(|at| now.duration_since(at) < RACK_PAD_TRIGGER_HOLD);
        }
    }
    flags
}

/// Publish only the pads whose light changed. A rack that is not playing holds
/// every flag at `false`, so an idle panel publishes nothing at all.
pub(crate) fn sync_rack_pad_trigger_field_delta(
    rt: &mut Runtime,
    previous: &[bool],
    flags: &[bool],
) -> bool {
    let mut effects_dirty = false;
    for (track, &lit) in flags.iter().enumerate() {
        if previous.get(track).copied().unwrap_or(false) == lit {
            continue;
        }
        effects_dirty |= rt
            .set_reactive(
                "SEQ",
                &rack_pad_trigger_field(track),
                Value::Number(if lit { 1.0 } else { 0.0 }),
            )
            .effects_dirty;
    }
    // A track that disappeared takes its light with it.
    for track in flags.len()..previous.len() {
        if !previous[track] {
            continue;
        }
        effects_dirty |= rt
            .set_reactive("SEQ", &rack_pad_trigger_field(track), Value::Number(0.0))
            .effects_dirty;
    }
    effects_dirty
}
