use super::*;

/// Build a Lisp Value::List of bools for record-armed state per track.
pub(crate) fn build_record_armed_value(armed: &[bool]) -> Value {
    let items: Vec<Rc<RefCell<Value>>> = armed
        .iter()
        .map(|a| Rc::new(RefCell::new(Value::Bool(*a))))
        .collect();
    Value::List(items)
}

/// Build a Lisp Value::List of track name strings.
pub(crate) fn build_track_names(names: &[String]) -> Value {
    let items: Vec<Rc<RefCell<Value>>> = names
        .iter()
        .map(|name| Rc::new(RefCell::new(Value::String(name.clone()))))
        .collect();
    Value::List(items)
}

pub(crate) fn build_track_colors(app: &app::App) -> Value {
    let items: Vec<Rc<RefCell<Value>>> = (0..app.tracks.len())
        .map(|track| {
            let color = app
                .track_colors
                .get(track)
                .copied()
                .unwrap_or_else(|| sequencer::track_color::TrackColor::palette_color(track))
                .clamped();
            Rc::new(RefCell::new(Value::List(vec![
                Rc::new(RefCell::new(Value::Number(color.r as f64))),
                Rc::new(RefCell::new(Value::Number(color.g as f64))),
                Rc::new(RefCell::new(Value::Number(color.b as f64))),
            ])))
        })
        .collect();
    Value::List(items)
}

pub(crate) fn build_track_collapsed_from_slice(collapsed: &[bool], track_count: usize) -> Value {
    Value::List(
        (0..track_count)
            .map(|track| {
                Rc::new(RefCell::new(Value::Bool(
                    collapsed.get(track).copied().unwrap_or(false),
                )))
            })
            .collect(),
    )
}

pub(crate) fn build_track_collapsed(app: &app::App) -> Value {
    build_track_collapsed_from_slice(&app.track_collapsed, app.tracks.len())
}

pub(crate) fn build_track_ids(app: &app::App) -> Value {
    let items: Vec<Rc<RefCell<Value>>> = app
        .graph
        .track_node_ids
        .iter()
        .map(|ids| Rc::new(RefCell::new(Value::Number(ids.pan_id as f64))))
        .collect();
    Value::List(items)
}

pub(crate) fn build_track_instrument_types(app: &app::App) -> Value {
    let items: Vec<Rc<RefCell<Value>>> = app
        .graph
        .track_instrument_types
        .iter()
        .map(|instrument_type| {
            let label = match instrument_type {
                sequencer::sequencer::InstrumentType::Sampler => "sampler",
                sequencer::sequencer::InstrumentType::Custom => "custom",
                sequencer::sequencer::InstrumentType::Modulator => "modulator",
                sequencer::sequencer::InstrumentType::Rack => "rack",
            };
            Rc::new(RefCell::new(Value::String(label.to_string())))
        })
        .collect();
    Value::List(items)
}

pub(crate) fn build_track_mod_output_available(app: &app::App) -> Value {
    let items: Vec<Rc<RefCell<Value>>> = (0..app.graph.track_instrument_types.len())
        .map(|track| {
            Rc::new(RefCell::new(Value::Bool(
                app.graph.track_exposes_mod_output(track),
            )))
        })
        .collect();
    Value::List(items)
}

pub(crate) fn build_track_instrument_run_modes(app: &app::App) -> Value {
    let items: Vec<Rc<RefCell<Value>>> = app
        .graph
        .track_instrument_run_modes
        .iter()
        .map(|run_mode| {
            let label = match run_mode {
                sequencer::sequencer::CustomInstrumentRunMode::Instrument => "instrument",
                sequencer::sequencer::CustomInstrumentRunMode::FreePatch => "free_patch",
            };
            Rc::new(RefCell::new(Value::String(label.to_string())))
        })
        .collect();
    Value::List(items)
}

pub(crate) fn sync_track_name_state(
    rt: &mut Runtime,
    track_names: &mut Vec<String>,
    app: &app::App,
) {
    rt.set_reactive("SEQ", "track-ids", build_track_ids(app));
    rt.set_reactive(
        "SEQ",
        "track-instrument-types",
        build_track_instrument_types(app),
    );
    sync_all_rack_slot_selection_binding_fields(rt, app);
    rt.set_reactive(
        "SEQ",
        "track-mod-output-available",
        build_track_mod_output_available(app),
    );
    rt.set_reactive(
        "SEQ",
        "track-instrument-run-modes",
        build_track_instrument_run_modes(app),
    );
    if *track_names != app.tracks {
        *track_names = app.tracks.clone();
    }
    rt.set_reactive("SEQ", "num-tracks", Value::Number(track_names.len() as f64));
    rt.set_reactive("SEQ", "track-names", build_track_names(track_names));
    rt.set_reactive("SEQ", "track-colors", build_track_colors(app));
    rt.set_reactive("SEQ", "track-collapsed", build_track_collapsed(app));
}

/// Build a Lisp Value::List of per-track volumes (0.0–1.0).
pub(crate) fn build_track_volumes(state: &Arc<SequencerState>) -> Value {
    let count = state.active_track_count();
    let items: Vec<Rc<RefCell<Value>>> = (0..count)
        .map(|t| {
            let vol = state.pattern.track_params[t].get_volume();
            Rc::new(RefCell::new(Value::Number(vol as f64)))
        })
        .collect();
    Value::List(items)
}

pub(crate) fn build_track_pans(state: &Arc<SequencerState>) -> Value {
    let count = state.active_track_count();
    let items: Vec<Rc<RefCell<Value>>> = (0..count)
        .map(|t| {
            Rc::new(RefCell::new(Value::Number(
                state.pattern.track_params[t].get_pan() as f64,
            )))
        })
        .collect();
    Value::List(items)
}

pub(crate) fn track_volume_field(track: usize) -> String {
    format!("track-{track}-volume")
}

pub(crate) fn track_pan_field(track: usize) -> String {
    format!("track-{track}-pan")
}

pub(crate) fn sync_track_volume_binding_field(
    rt: &mut Runtime,
    state: &Arc<SequencerState>,
    track: usize,
) {
    if let Some(tp) = state.pattern.track_params.get(track) {
        rt.set_reactive(
            "SEQ",
            &track_volume_field(track),
            Value::Number(tp.get_volume() as f64),
        );
    }
}

pub(crate) fn sync_track_pan_binding_field(
    rt: &mut Runtime,
    state: &Arc<SequencerState>,
    track: usize,
) {
    if let Some(tp) = state.pattern.track_params.get(track) {
        rt.set_reactive(
            "SEQ",
            &track_pan_field(track),
            Value::Number(tp.get_pan() as f64),
        );
    }
}

pub(crate) fn sync_track_volume_pan_binding_fields(rt: &mut Runtime, state: &Arc<SequencerState>) {
    for track in 0..state.active_track_count() {
        sync_track_volume_binding_field(rt, state, track);
        sync_track_pan_binding_field(rt, state, track);
    }
}

pub(crate) fn build_track_outputs(app: &app::App, state: &Arc<SequencerState>) -> Value {
    let count = state.active_track_count();
    let items: Vec<Rc<RefCell<Value>>> = (0..count)
        .map(|t| {
            Rc::new(RefCell::new(build_track_output_label(
                app,
                &state.pattern.track_params[t],
            )))
        })
        .collect();
    Value::List(items)
}

pub(crate) fn build_all_track_bus_sends(app: &app::App, state: &Arc<SequencerState>) -> Value {
    let count = state.active_track_count();
    let items: Vec<Rc<RefCell<Value>>> = (0..count)
        .map(|t| {
            Rc::new(RefCell::new(build_track_bus_sends(
                app,
                &state.pattern.track_params[t],
            )))
        })
        .collect();
    Value::List(items)
}

pub(crate) fn track_bus_send_field(track: usize, bus_idx: usize) -> String {
    format!("track-{track}-bus-{bus_idx}-send")
}

pub(crate) fn current_track_bus_send_field(bus_idx: usize) -> String {
    format!("tp-bus-{bus_idx}-send")
}

pub(crate) fn track_bus_send_amount(
    app: &app::App,
    state: &Arc<SequencerState>,
    track: usize,
    bus_idx: usize,
) -> Option<f32> {
    let bus = app.buses.get(bus_idx)?;
    if bus.id == sequencer::sequencer::BusId::MIX {
        return None;
    }
    let tp = state.pattern.track_params.get(track)?;
    Some(
        tp.sends()
            .iter()
            .find(|send| send.destination == bus.id)
            .map(|send| send.amount)
            .unwrap_or(0.0),
    )
}

pub(crate) fn sync_track_bus_send_binding_field(
    rt: &mut Runtime,
    app: &app::App,
    state: &Arc<SequencerState>,
    track: usize,
    bus_idx: usize,
) {
    if let Some(amount) = track_bus_send_amount(app, state, track, bus_idx) {
        rt.set_reactive(
            "SEQ",
            &track_bus_send_field(track, bus_idx),
            Value::Number(amount as f64),
        );
    }
}

pub(crate) fn sync_track_bus_send_binding_fields(
    rt: &mut Runtime,
    app: &app::App,
    state: &Arc<SequencerState>,
) {
    for track in 0..state.active_track_count() {
        for (bus_idx, bus) in app.buses.iter().enumerate() {
            if bus.id != sequencer::sequencer::BusId::MIX {
                sync_track_bus_send_binding_field(rt, app, state, track, bus_idx);
            }
        }
    }
}

pub(crate) fn sync_current_track_bus_send_binding_field(
    rt: &mut Runtime,
    app: &app::App,
    state: &Arc<SequencerState>,
    track: usize,
    bus_idx: usize,
) {
    if let Some(amount) = track_bus_send_amount(app, state, track, bus_idx) {
        rt.set_reactive(
            "SEQ",
            &current_track_bus_send_field(bus_idx),
            Value::Number(amount as f64),
        );
    }
}

pub(crate) fn sync_current_track_bus_send_binding_fields(
    rt: &mut Runtime,
    app: &app::App,
    state: &Arc<SequencerState>,
    track: usize,
) {
    for (bus_idx, bus) in app.buses.iter().enumerate() {
        if bus.id != sequencer::sequencer::BusId::MIX {
            sync_current_track_bus_send_binding_field(rt, app, state, track, bus_idx);
        }
    }
}

/// Synchronize send controls to the same display step used by synth/effect
/// parameters: selection first, then the playback step, then the pattern
/// baseline. Both mixer-strip and current-track controls share these fields,
/// so selection and playhead changes must update both projections together.
pub(crate) fn sync_selected_track_bus_send_binding_fields(
    rt: &mut Runtime,
    app: &app::App,
    state: &Arc<SequencerState>,
    track: usize,
    selected_steps: &Arc<Mutex<HashSet<usize>>>,
) -> bool {
    let display_step = displayed_plock_step(state, track, selected_plock_step(selected_steps));
    let locks = display_step.map(|_| state.pattern.track_send_plocks[track].snapshot());
    let mut dirty = false;
    for (bus_idx, bus) in app.buses.iter().enumerate() {
        if bus.id == sequencer::sequencer::BusId::MIX {
            continue;
        }
        let Some(baseline) = track_bus_send_amount(app, state, track, bus_idx) else {
            continue;
        };
        let amount = display_step
            .and_then(|step| locks.as_ref()?.get(step))
            .and_then(|row| row.iter().find(|send| send.destination == bus.id))
            .map(|send| send.amount)
            .unwrap_or(baseline);
        dirty |= rt.set_reactive(
            "SEQ",
            &track_bus_send_field(track, bus_idx),
            Value::Number(amount as f64),
        ).effects_dirty;
        dirty |= rt.set_reactive(
            "SEQ",
            &current_track_bus_send_field(bus_idx),
            Value::Number(amount as f64),
        ).effects_dirty;
    }
    dirty
}

pub(crate) fn build_mod_routes(state: &Arc<SequencerState>) -> Value {
    let routes = state.current_mod_connections();
    Value::List(
        routes
            .into_iter()
            .map(|connection| {
                let mut map = std::collections::HashMap::new();
                map.insert(
                    "source".to_string(),
                    Rc::new(RefCell::new(Value::Number(connection.source_track as f64))),
                );
                map.insert(
                    "dest-kind".to_string(),
                    Rc::new(RefCell::new(mod_destination_kind_value(
                        connection.destination,
                    ))),
                );
                map.insert(
                    "dest".to_string(),
                    Rc::new(RefCell::new(mod_destination_id_value(
                        connection.destination,
                    ))),
                );
                map.insert(
                    "input".to_string(),
                    Rc::new(RefCell::new(Value::Number(connection.dest_input as f64))),
                );
                Rc::new(RefCell::new(Value::Map(map)))
            })
            .collect(),
    )
}

pub(crate) fn build_track_mutes(state: &Arc<SequencerState>) -> Value {
    let count = state.active_track_count();
    let items: Vec<Rc<RefCell<Value>>> = (0..count)
        .map(|t| {
            Rc::new(RefCell::new(Value::Bool(
                state.pattern.track_params[t].is_muted(),
            )))
        })
        .collect();
    Value::List(items)
}

pub(crate) fn build_track_solos(state: &Arc<SequencerState>) -> Value {
    let count = state.active_track_count();
    let items: Vec<Rc<RefCell<Value>>> = (0..count)
        .map(|t| {
            Rc::new(RefCell::new(Value::Bool(
                state.pattern.track_params[t].is_solo(),
            )))
        })
        .collect();
    Value::List(items)
}

pub(crate) fn build_track_muted_by_solo(app: &app::App, state: &Arc<SequencerState>) -> Value {
    let count = state.active_track_count();
    let solo = app.solo_audibility();
    let items: Vec<Rc<RefCell<Value>>> = (0..count)
        .map(|t| {
            Rc::new(RefCell::new(Value::Bool(track_muted_by_solo(
                state, t, &solo,
            ))))
        })
        .collect();
    Value::List(items)
}

pub(super) fn track_effectively_muted(
    state: &Arc<SequencerState>,
    track: usize,
    solo: &app::SoloAudibility,
) -> bool {
    let params = &state.pattern.track_params[track];
    params.is_muted() || solo.track_is_muted(params)
}

pub(super) fn track_muted_by_solo(
    state: &Arc<SequencerState>,
    track: usize,
    solo: &app::SoloAudibility,
) -> bool {
    solo.track_is_muted(&state.pattern.track_params[track])
}

/// Per-track "effectively muted" (explicit mute OR muted by another track's
/// solo) as 0/1 numbers, for widget bindings via `bind-seq-nth`. Lets row
/// `:muted` props update without rerunning the row's subtree.
pub(crate) fn build_track_muted_effective(app: &app::App, state: &Arc<SequencerState>) -> Value {
    let count = state.active_track_count();
    let solo = app.solo_audibility();
    let items: Vec<Rc<RefCell<Value>>> = (0..count)
        .map(|t| {
            let muted = track_effectively_muted(state, t, &solo);
            Rc::new(RefCell::new(Value::Number(if muted { 1.0 } else { 0.0 })))
        })
        .collect();
    Value::List(items)
}

/// Per-track UI color channel with the mute dim baked in, matching the Lisp
/// seqv-track-color-r/g/b formulas used by row chrome and compact controls.
pub(crate) fn build_track_color_channel_effective(
    app: &app::App,
    state: &Arc<SequencerState>,
    channel: usize,
) -> Value {
    let count = state.active_track_count();
    let solo = app.solo_audibility();
    let items: Vec<Rc<RefCell<Value>>> = (0..count)
        .map(|track| {
            let muted = track_effectively_muted(state, track, &solo);
            let value = track_color_channel_effective_value(app, track, channel, muted);
            Rc::new(RefCell::new(Value::Number(value)))
        })
        .collect();
    Value::List(items)
}

/// Step-cell color channel: the raw track color, dimmed only for take-governed
/// lanes (takes spec 10 UX). Muting is a separate shader state so muted steps
/// can use fully opaque neutral materials instead of translucent track colors.
pub(crate) fn build_step_color_channel_effective(
    app: &app::App,
    state: &Arc<SequencerState>,
    channel: usize,
) -> Value {
    let count = state.active_track_count();
    let take_states = super::song_state::song_take_lane_states(app);
    let items: Vec<Rc<RefCell<Value>>> = (0..count)
        .map(|track| {
            let dimmed = take_states.get(track) == Some(&1);
            let value = track_color_channel_effective_value(app, track, channel, dimmed);
            Rc::new(RefCell::new(Value::Number(value)))
        })
        .collect();
    Value::List(items)
}

pub(super) fn step_color_channel_effective_field(channel: usize) -> &'static str {
    match channel {
        0 => "step-color-r-effective",
        1 => "step-color-g-effective",
        _ => "step-color-b-effective",
    }
}

pub(super) fn track_color_channel_effective_value(
    app: &app::App,
    track: usize,
    channel: usize,
    dimmed: bool,
) -> f64 {
    let color = app
        .track_colors
        .get(track)
        .copied()
        .unwrap_or_else(|| sequencer::track_color::TrackColor::palette_color(track))
        .clamped();
    let (raw, dim_base) = match channel {
        0 => (color.r as f64, 0.10),
        1 => (color.g as f64, 0.10),
        _ => (color.b as f64, 0.11),
    };
    if dimmed {
        raw * 0.34 + dim_base * 0.66
    } else {
        raw
    }
}

pub(super) fn track_color_channel_effective_field(channel: usize) -> &'static str {
    match channel {
        0 => "track-color-r-effective",
        1 => "track-color-g-effective",
        _ => "track-color-b-effective",
    }
}

pub(crate) fn sync_track_mute_visual_binding_fields(
    rt: &mut Runtime,
    app: &app::App,
    state: &Arc<SequencerState>,
    tracks: impl IntoIterator<Item = usize>,
    sync_muted_by_solo: bool,
) -> bool {
    let count = state.active_track_count();
    let solo = app.solo_audibility();
    let take_states = super::song_state::song_take_lane_states(app);
    let mut effects_dirty = false;

    for track in tracks {
        if track >= count {
            continue;
        }
        if sync_muted_by_solo {
            effects_dirty |= rt
                .set_reactive_list_index(
                    "SEQ",
                    "track-muted-by-solo",
                    track,
                    Value::Bool(track_muted_by_solo(state, track, &solo)),
                )
                .effects_dirty;
        }

        let muted = track_effectively_muted(state, track, &solo);
        effects_dirty |= rt
            .set_reactive_list_index(
                "SEQ",
                "track-muted-effective",
                track,
                Value::Number(if muted { 1.0 } else { 0.0 }),
            )
            .effects_dirty;

        for channel in 0..3 {
            effects_dirty |= rt
                .set_reactive_list_index(
                    "SEQ",
                    track_color_channel_effective_field(channel),
                    track,
                    Value::Number(track_color_channel_effective_value(
                        app, track, channel, muted,
                    )),
                )
                .effects_dirty;
        }
        // Step-cell channels only dim take-governed lanes. Effective mute is
        // passed independently to the step shader above.
        let step_dimmed = take_states.get(track) == Some(&1);
        for channel in 0..3 {
            effects_dirty |= rt
                .set_reactive_list_index(
                    "SEQ",
                    step_color_channel_effective_field(channel),
                    track,
                    Value::Number(track_color_channel_effective_value(
                        app, track, channel, step_dimmed,
                    )),
                )
                .effects_dirty;
        }
    }

    effects_dirty
}

pub(crate) fn sync_track_mixer_state(
    rt: &mut Runtime,
    app: &app::App,
    state: &Arc<SequencerState>,
) {
    rt.set_reactive("SEQ", "track-colors", build_track_colors(app));
    rt.set_reactive("SEQ", "track-collapsed", build_track_collapsed(app));
    rt.set_reactive(
        "SEQ",
        "track-pattern-cells",
        build_track_pattern_cells_value(state, app.tracks.len()),
    );
    sync_track_pattern_cell_state_fields(rt, state, app.tracks.len());
    rt.set_reactive(
        "SEQ",
        "track-instrument-types",
        build_track_instrument_types(app),
    );
    sync_all_rack_slot_selection_binding_fields(rt, app);
    rt.set_reactive(
        "SEQ",
        "track-mod-output-available",
        build_track_mod_output_available(app),
    );
    rt.set_reactive(
        "SEQ",
        "track-instrument-run-modes",
        build_track_instrument_run_modes(app),
    );
    rt.set_reactive("SEQ", "track-volumes", build_track_volumes(state));
    rt.set_reactive("SEQ", "track-mixer-pans", build_track_pans(state));
    sync_track_volume_pan_binding_fields(rt, state);
    rt.set_reactive("SEQ", "track-outputs", build_track_outputs(app, state));
    rt.set_reactive(
        "SEQ",
        "track-bus-sends",
        build_all_track_bus_sends(app, state),
    );
    sync_track_bus_send_binding_fields(rt, app, state);
    rt.set_reactive("SEQ", "mod-routes", build_mod_routes(state));
    rt.set_reactive("SEQ", "track-mutes", build_track_mutes(state));
    rt.set_reactive("SEQ", "track-solos", build_track_solos(state));
    rt.set_reactive(
        "SEQ",
        "track-muted-by-solo",
        build_track_muted_by_solo(app, state),
    );
    rt.set_reactive(
        "SEQ",
        "track-muted-effective",
        build_track_muted_effective(app, state),
    );
    rt.set_reactive(
        "SEQ",
        "step-color-r-effective",
        build_step_color_channel_effective(app, state, 0),
    );
    rt.set_reactive(
        "SEQ",
        "step-color-g-effective",
        build_step_color_channel_effective(app, state, 1),
    );
    rt.set_reactive(
        "SEQ",
        "step-color-b-effective",
        build_step_color_channel_effective(app, state, 2),
    );
    rt.set_reactive(
        "SEQ",
        "track-color-r-effective",
        build_track_color_channel_effective(app, state, 0),
    );
    rt.set_reactive(
        "SEQ",
        "track-color-g-effective",
        build_track_color_channel_effective(app, state, 1),
    );
    rt.set_reactive(
        "SEQ",
        "track-color-b-effective",
        build_track_color_channel_effective(app, state, 2),
    );
}

pub(crate) fn sync_bus_mixer_control_state(rt: &mut Runtime, app: &app::App) {
    let names: Vec<String> = app.buses.iter().map(|bus| bus.name.clone()).collect();
    let volumes: Vec<Rc<RefCell<Value>>> = app
        .buses
        .iter()
        .map(|bus| Rc::new(RefCell::new(Value::Number(bus.volume as f64))))
        .collect();
    let mutes: Vec<Rc<RefCell<Value>>> = app
        .buses
        .iter()
        .map(|bus| Rc::new(RefCell::new(Value::Bool(bus.mute))))
        .collect();
    let solos: Vec<Rc<RefCell<Value>>> = app
        .buses
        .iter()
        .map(|bus| Rc::new(RefCell::new(Value::Bool(bus.solo))))
        .collect();
    let ids: Vec<Rc<RefCell<Value>>> = app
        .buses
        .iter()
        .map(|bus| Rc::new(RefCell::new(Value::Number(bus.id.0 as f64))))
        .collect();
    rt.set_reactive("SEQ", "bus-ids", Value::List(ids));
    rt.set_reactive("SEQ", "bus-names", build_track_names(&names));
    rt.set_reactive("SEQ", "bus-volumes", Value::List(volumes));
    rt.set_reactive("SEQ", "bus-mutes", Value::List(mutes));
    rt.set_reactive("SEQ", "bus-solos", Value::List(solos));
}

pub(crate) fn sync_bus_mixer_state(rt: &mut Runtime, app: &app::App) {
    sync_bus_mixer_control_state(rt, app);
    rt.set_reactive("SEQ", "bus-effects", build_bus_effects_value(app));
}

pub(crate) fn sync_track_mixer_empty_state(rt: &mut Runtime) {
    rt.set_reactive("SEQ", "track-volumes", Value::List(vec![]));
    rt.set_reactive("SEQ", "track-mixer-pans", Value::List(vec![]));
    rt.set_reactive("SEQ", "track-outputs", Value::List(vec![]));
    rt.set_reactive("SEQ", "track-colors", Value::List(vec![]));
    rt.set_reactive("SEQ", "track-collapsed", Value::List(vec![]));
    rt.set_reactive("SEQ", "track-pattern-cells", Value::List(vec![]));
    rt.set_reactive("SEQ", "track-instrument-types", Value::List(vec![]));
    rt.set_reactive("SEQ", "track-mod-output-available", Value::List(vec![]));
    rt.set_reactive("SEQ", "track-bus-sends", Value::List(vec![]));
    rt.set_reactive("SEQ", "track-mutes", Value::List(vec![]));
    rt.set_reactive("SEQ", "track-solos", Value::List(vec![]));
    rt.set_reactive("SEQ", "track-muted-by-solo", Value::List(vec![]));
    rt.set_reactive("SEQ", "track-muted-effective", Value::List(vec![]));
    rt.set_reactive("SEQ", "track-color-r-effective", Value::List(vec![]));
    rt.set_reactive("SEQ", "track-color-g-effective", Value::List(vec![]));
    rt.set_reactive("SEQ", "track-color-b-effective", Value::List(vec![]));
    rt.set_reactive("SEQ", "bus-names", Value::List(vec![]));
    rt.set_reactive("SEQ", "bus-volumes", Value::List(vec![]));
    rt.set_reactive("SEQ", "bus-mutes", Value::List(vec![]));
    rt.set_reactive("SEQ", "bus-solos", Value::List(vec![]));
}

