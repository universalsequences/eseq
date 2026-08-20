use crate::*;

pub(super) const COMMANDS: &[&str] = &[
    "reveal-sequencer-track",
    "rename-track",
    "rename-group",
    "convert-group-to-drum-rack",
    "ungroup-tracks",
    "add-track-sampler",
    "add-track-rack",
    "add-track-layer-rack",
    "add-track-modulator",
    "swap-track-builtin-instrument",
    "add-track-sample",
    "add-track-instrument",
    "swap-track-instrument",
    "delete-track",
    "delete-tracks",
    "delete-track-group",
    "load-sound-onto-track",
    "add-track-from-sound",
    "group-selected-tracks",
    "move-track-to-group",
    "remove-track-from-group",
];

fn extract_track_indices(payload: &Value) -> Option<Vec<usize>> {
    let Value::Map(fields) = payload else {
        return None;
    };
    let tracks = fields.get("tracks")?;
    let tracks = tracks.borrow();
    let Value::List(tracks) = &*tracks else {
        return None;
    };
    tracks
        .iter()
        .map(|track| {
            let track = track.borrow();
            let Value::Number(track) = &*track else {
                return None;
            };
            (track.is_finite() && *track >= 0.0 && track.fract() == 0.0)
                .then_some(*track as usize)
        })
        .collect()
}

pub(crate) fn apply_rename_group_host_command(
    app: &mut app::App,
    payload: &Value,
) -> Result<(), String> {
    let group_id = extract_usize_from_payload(payload, "group-id")
        .map(|group_id| group_id as u64)
        .ok_or_else(|| "Rename group requires a group id and name".to_string())?;
    let name = extract_string_from_payload(payload, "name")
        .ok_or_else(|| "Rename group requires a group id and name".to_string())?;
    app.rename_group_recorded(group_id, name)
}

fn sync_after_track_topology_delete(
    app: &mut app::App,
    editor: &mut Editor,
    ctx: &mut LoopCtx<'_>,
    new_idx: usize,
) {
    let state = ctx.shared.state.clone();
    ctx.shared.current_track.store(new_idx, Ordering::Relaxed);
    *ctx.shared.track_pan_ids.lock().unwrap() = app
        .graph
        .track_node_ids
        .iter()
        .map(|ids| ids.pan_id)
        .collect();
    push_solo_mutes(ctx.shared.lg_raw, &state);
    ctx.meters.cached_track_peak_levels =
        read_track_peak_levels(app.graph.lg, &app.graph.track_node_ids);
    ctx.meters.cached_bus_peak_levels =
        read_bus_peak_levels(app.graph.lg, &app.graph.bus_node_ids);
    (ctx.meters.cached_modulator_phases, ctx.meters.cached_modulator_levels) =
        read_modulator_display_values(app.graph.lg, app);
    ctx.meters.last_meter_poll_at = Instant::now();
    *ctx.shared.record_armed.lock().unwrap() = app.graph.record_armed.clone();
    *ctx.shared.track_groups.lock().unwrap() = app.groups.clone();
    *ctx.shared.bus_state.lock().unwrap() = app.buses.clone();
    natives::prune_stale_group_references(
        &ctx.shared.armed_rack,
        &ctx.shared.active_delete_target,
        &ctx.shared.active_delete_target_version,
        &app.groups,
    );
    *ctx.shared.bus_node_ids.lock().unwrap() = app.graph.bus_node_ids.clone();

    let rt = editor.runtime_mut();
    sync_track_topology_state(
        rt,
        app,
        &state,
        ctx.track_names,
        new_idx,
        &ctx.shared.selected_steps,
        &ctx.shared.piano_roll_selection,
        &ctx.shared.accumulator_names,
        &ctx.shared.record_armed,
        &ctx.meters.cached_track_peak_levels,
    );
    sync_bus_mixer_state(rt, app);
    sync_bus_peak_fields(rt, &ctx.meters.cached_bus_peak_levels);
    sync_modulator_phase_fields(rt, &ctx.meters.cached_modulator_phases);
    sync_modulator_level_fields(rt, &ctx.meters.cached_modulator_levels);
    rt.clear_subtree_effects_for_named_target("*sequencer*");
    rt.run_reactive_cycle();
    editor.refresh_runtime_side_effects();
    refresh_visible_track_topology_layouts(editor);
    ctx.frame.prev_track_playheads = track_playheads_snapshot(&state, app);
    ctx.frame.prev_track_button_states = track_button_state_snapshot(&state);
    ctx.shared.ui_epoch.fetch_add(1, Ordering::Relaxed);
}

#[allow(clippy::too_many_lines)]
pub(super) fn handle(
    name: &str,
    payload: Value,
    mut app: &mut app::App,
    mut editor: &mut Editor,
    ctx: &mut LoopCtx<'_>,
) {
    let state = ctx.shared.state.clone();
    let lg_raw = ctx.shared.lg_raw;
    let current_track = ctx.shared.current_track.clone();
    let selected_tracks = ctx.shared.selected_tracks.clone();
    let selected_steps = ctx.shared.selected_steps.clone();
    let selected_neural_neurons = ctx.shared.selected_neural_neurons.clone();
    let ui_epoch = ctx.shared.ui_epoch.clone();
    let fx_epoch = ctx.shared.fx_epoch.clone();
    let track_pan_ids = ctx.shared.track_pan_ids.clone();
    let bus_state = ctx.shared.bus_state.clone();
    let bus_node_ids = ctx.shared.bus_node_ids.clone();
    let track_groups = ctx.shared.track_groups.clone();
    let active_delete_target = ctx.shared.active_delete_target.clone();
    let active_delete_target_version = ctx.shared.active_delete_target_version.clone();
    let record_armed = ctx.shared.record_armed.clone();
    let accumulator_names = ctx.shared.accumulator_names.clone();
    match name {
        "reveal-sequencer-track" => {
            if let Some(track) = extract_usize_from_payload(&payload, "track") {
                if track < app.tracks.len() {
                    reveal_sequencer_current_track(&mut editor, &app, track);
                }
            }
        }
        "rename-track" => {
            let track = extract_usize_from_payload(&payload, "track");
            let requested_name = extract_string_from_payload(&payload, "name");
            match (track, requested_name) {
                (Some(track), Some(requested_name)) => {
                    match app.apply_recorded_track_name(track, &requested_name) {
                        Ok(app::edit::EditOutcome::Applied(result)) => {
                            *ctx.track_names = app.tracks.clone();
                            let rt = editor.runtime_mut();
                            rt.set_reactive(
                                "SEQ",
                                "track-names",
                                build_track_names(&ctx.track_names),
                            );
                            rt.run_reactive_cycle();
                            editor.refresh_runtime_side_effects();
                            editor.show_transient_message(result.label);
                        }
                        Ok(app::edit::EditOutcome::NoOp) => {}
                        Ok(app::edit::EditOutcome::AppliedUnrecorded) => {
                            editor.handle_host_event(HostEvent::Error(
                                "Track rename was applied without history".to_string(),
                            ));
                        }
                        Err(error) => editor.handle_host_event(HostEvent::Error(error)),
                    }
                }
                _ => editor.handle_host_event(HostEvent::Error(
                    "Track rename requires a track and name".to_string(),
                )),
            }
        }
        "rename-group" => {
            match apply_rename_group_host_command(&mut app, &payload) {
                Ok(()) => {
                    *track_groups.lock().unwrap() = app.groups.clone();
                    *bus_state.lock().unwrap() = app.buses.clone();
                    let rt = editor.runtime_mut();
                    sync_groups_bindings(rt, &app.groups);
                    sync_bus_mixer_state(rt, &app);
                    rt.run_reactive_cycle();
                    editor.refresh_runtime_side_effects();
                    editor.show_transient_message("Rename track group".to_string());
                    ui_epoch.fetch_add(1, Ordering::Relaxed);
                }
                Err(error) => editor.handle_host_event(HostEvent::Error(error)),
            }
        }
        "convert-group-to-drum-rack" => {
            let Some(group_id) = extract_usize_from_payload(&payload, "group-id")
                .map(|group_id| group_id as u64)
            else {
                editor.handle_host_event(HostEvent::Status(
                    "Convert group to drum rack requires a group id".to_string(),
                ));
                return;
            };
            match app.convert_group_to_drum_rack_recorded(group_id) {
                Ok(()) => {
                    host_commands::drum_rack_v2::sync_after_rack_structure_change(
                        &mut app,
                        &mut editor,
                        ctx,
                        None,
                    );
                    editor.handle_host_event(HostEvent::Status(
                        "Converted group to drum rack".to_string(),
                    ));
                }
                Err(error) => editor.handle_host_event(HostEvent::Status(error)),
            }
        }
        "ungroup-tracks" => {
            let Some(group_id) = extract_usize_from_payload(&payload, "group-id")
                .map(|group_id| group_id as u64)
            else {
                editor.handle_host_event(HostEvent::Status(
                    "Ungroup tracks requires a group id".to_string(),
                ));
                return;
            };
            match app.ungroup_tracks_recorded(group_id) {
                Ok(()) => {
                    host_commands::drum_rack_v2::sync_after_rack_structure_change(
                        &mut app,
                        &mut editor,
                        ctx,
                        None,
                    );
                    editor.handle_host_event(HostEvent::Status(
                        "Ungrouped tracks".to_string(),
                    ));
                }
                Err(error) => editor.handle_host_event(HostEvent::Status(error)),
            }
        }
        "add-track-sampler" => match app.graph_controller().add_blank_sampler_track()
            .and_then(|idx| {
                app.commit_created_track(idx, "Add sampler track")?;
                Ok(idx)
            }) {
            Ok(idx) => {
                current_track.store(idx, Ordering::Relaxed);
                let new_name = app.tracks[idx].clone();
                ctx.track_names.push(new_name.clone());
                {
                    let mut pan_ids = track_pan_ids.lock().unwrap();
                    pan_ids.push(app.graph.track_node_ids[idx].pan_id);
                    push_solo_mutes(lg_raw, &state);
                }
                record_armed.lock().unwrap().push(false);
                let rt = editor.runtime_mut();
                rt.set_reactive(
                    "SEQ",
                    "num-tracks",
                    Value::Number(ctx.track_names.len() as f64),
                );
                rt.set_reactive("SEQ", "track-ids", build_track_ids(&app));
                set_current_track_reactive(rt, app.tracks.len(), idx);
                rt.set_reactive("SEQ", "track-names", build_track_names(&ctx.track_names));
                sync_all_track_sequencer_state(rt, &state, &app, idx, &selected_steps);
                rt.set_reactive("SEQ", "steps", build_steps_value(&state, idx));
                sync_step_param_lists(rt, &state, idx);
                sync_track_mixer_state(rt, &app, &state);
                sync_bus_mixer_state(rt, &app);
                sync_track_peak_fields(rt, &ctx.meters.cached_track_peak_levels);
                sync_bus_peak_fields(rt, &ctx.meters.cached_bus_peak_levels);
                rt.set_reactive(
                    "SEQ",
                    "effects",
                    build_effects_value(
                        &state,
                        idx,
                        &app.graph.effect_descriptors,
                        &selected_steps,
                    ),
                );
                rt.set_reactive(
                    "SEQ",
                    "midi-effects",
                    build_midi_effects_value(&state, idx, &selected_steps),
                );
                rt.set_reactive(
                    "SEQ",
                    "instrument-panel",
                    build_instrument_panel_value(&app, idx, &selected_steps),
                );
                *accumulator_names.lock().unwrap() = build_accumulator_names(&app);
                let selected_neural_snapshot =
                    selected_neural_neurons.lock().unwrap().clone();
                sync_track_params_with_neural_selection(
                    rt,
                    &app,
                    &state,
                    idx,
                    &selected_steps,
                    Some(&selected_neural_snapshot),
                );
                sync_fx_param_binding_fields_with_neural_selection(
                    rt,
                    &app,
                    &state,
                    idx,
                    &selected_steps,
                    Some(&selected_neural_snapshot),
                );
                rt.set_reactive(
                    "SEQ",
                    "step-has-plocks",
                    build_step_has_plocks(&state, idx, &app.graph.effect_descriptors),
                );
                rt.run_reactive_cycle();
                editor.refresh_runtime_side_effects();
                ui_epoch.fetch_add(1, Ordering::Relaxed);
                editor.handle_host_event(HostEvent::Status(format!(
                    "Added sampler track {}: {new_name}",
                    idx + 1
                )));
            }
            Err(e) => {
                editor.handle_host_event(HostEvent::Status(format!(
                    "Error adding sampler track: {e}"
                )));
            }
        },
        // Creating a drum rack builds a track group carrying a pad map, not a
        // slot-based rack track (docs/drum-rack-v2-spec.md, "Core model"). Pads
        // claim member tracks lazily, so a rack created from a selected sample
        // starts with exactly one member on the first pad.
        "add-track-rack" => {
            let path_str = extract_path_from_payload(&payload)
                .filter(|path| !path.trim().is_empty());
            // Both halves are one transaction: a failing sample half rolls the
            // rack group back rather than leaving a phantom the status message
            // below would deny.
            let sample = path_str.as_deref().map(Path::new);
            match app.create_drum_rack_with_pad_recorded(sample) {
                Ok((group_id, track)) => {
                    if let Some(path_str) = path_str.as_deref() {
                        register_waveform_sample(Path::new(path_str));
                    }
                    // An empty rack creates no track, so there is nothing to
                    // focus — passing `None` keeps focus where the user left it.
                    host_commands::drum_rack_v2::sync_after_rack_structure_change(
                        &mut app,
                        &mut editor,
                        ctx,
                        track,
                    );
                    let name = host_commands::drum_rack_v2::group_name(&app, group_id)
                        .unwrap_or_else(|| "Drum Rack".to_string());
                    editor.handle_host_event(HostEvent::Status(format!(
                        "Added drum rack: {name}"
                    )));
                }
                Err(e) => {
                    // The transaction rolled back, so republish the group/bus
                    // state the failed halves may have touched on their way out.
                    host_commands::drum_rack_v2::sync_after_rack_structure_change(
                        &mut app,
                        &mut editor,
                        ctx,
                        None,
                    );
                    editor.handle_host_event(HostEvent::Status(format!(
                        "Error adding drum rack: {e}"
                    )));
                }
            }
        }
        "add-track-layer-rack" => {
            let path_str = extract_path_from_payload(&payload)
                .filter(|path| !path.trim().is_empty());
            let result = if let Some(path_str) = path_str {
                let path = PathBuf::from(path_str);
                app.graph_controller().add_sampler_rack_track(&[path])
            } else {
                app.graph_controller().add_empty_layer_rack_track()
            };
            let result = result.and_then(|idx| {
                app.commit_created_track(idx, "Add layer rack track")?;
                Ok(idx)
            });
            match result {
                Ok(idx) => {
                    sync_after_instrument_track_apply(
                        &mut app,
                        &mut editor,
                        &state,
                        idx,
                        &current_track,
                        &mut *ctx.track_names,
                        &track_pan_ids,
                        &record_armed,
                        &selected_steps,
                        &accumulator_names,
                        &ctx.meters.cached_track_peak_levels,
                        &ctx.meters.cached_bus_peak_levels,
                        &ui_epoch,
                        lg_raw,
                    );
                    let new_name = app.tracks[idx].clone();
                    editor.handle_host_event(HostEvent::Status(format!(
                        "Added layer rack track {}: {new_name}",
                        idx + 1
                    )));
                }
                Err(e) => {
                    editor.handle_host_event(HostEvent::Status(format!(
                        "Error adding layer rack track: {e}"
                    )));
                }
            }
        }
        "add-track-modulator" => match app.graph_controller().add_modulator_track()
            .and_then(|idx| {
                app.commit_created_track(idx, "Add modulator track")?;
                Ok(idx)
            }) {
            Ok(idx) => {
                sync_after_instrument_track_apply(
                    &mut app,
                    &mut editor,
                    &state,
                    idx,
                    &current_track,
                    &mut *ctx.track_names,
                    &track_pan_ids,
                    &record_armed,
                    &selected_steps,
                    &accumulator_names,
                    &ctx.meters.cached_track_peak_levels,
                    &ctx.meters.cached_bus_peak_levels,
                    &ui_epoch,
                    lg_raw,
                );
                let new_name = app.tracks[idx].clone();
                editor.handle_host_event(HostEvent::Status(format!(
                    "Added modulator track {}: {new_name}",
                    idx + 1
                )));
            }
            Err(e) => {
                editor.handle_host_event(HostEvent::Status(format!(
                    "Error adding modulator track: {e}"
                )));
            }
        },
        "swap-track-builtin-instrument" => {
            let track = extract_usize_from_payload(&payload, "track");
            let instrument = extract_string_from_payload(&payload, "name");
            let preserve_track_selection =
                extract_bool_from_payload(&payload, "preserve-track-selection");
            match (track, instrument.as_deref()) {
                (Some(track), Some("sampler")) => {
                    match load_or_convert_sampler_track(
                        &mut app,
                        &mut editor,
                        &state,
                        &current_track,
                        &mut *ctx.track_names,
                        &selected_steps,
                        lg_raw,
                        track,
                        None,
                        preserve_track_selection,
                    ) {
                        Ok(result) => {
                            let _ = editor
                                .runtime_mut()
                                .eval_str("(set! sbrowser-tab \"samples\")");
                            let status = result.reset_summary.map_or_else(
                                || format!("Sampler already active ({})", result.name),
                                |summary| {
                                    host_commands::instrument_swap_status(
                                        "sampler", summary,
                                    )
                                },
                            );
                            editor.handle_host_event(HostEvent::Status(status));
                        }
                        Err(error) => editor.handle_host_event(HostEvent::Status(
                            format!("Cannot convert track to sampler: {error}"),
                        )),
                    }
                }
                (Some(_), Some(name)) => {
                    editor.handle_host_event(HostEvent::Status(format!(
                        "Builtin instrument conversion is not supported for {name}"
                    )))
                }
                _ => editor.handle_host_event(HostEvent::Status(
                    "Builtin instrument swap is missing a track or name".to_string(),
                )),
            }
        }
        "add-track-sample" => {
            let path_str = extract_path_from_payload(&payload);
            let group_id = extract_usize_from_payload(&payload, "group-id")
                .map(|group_id| group_id as u64);
            // Dropping a sound on an empty drum-rack pad: the new track becomes
            // that pad's member (docs/drum-rack-v2-spec.md, "Track budget").
            let rack_pad_note = extract_i32_from_payload(&payload, "pad-note");
            let preserve_browser_context =
                extract_bool_from_payload(&payload, "preserve-browser-context");
            eprintln!(
                "sample-host-command: add-track-sample payload={payload:?}; extracted_path={path_str:?}; preserve_browser_context={preserve_browser_context}"
            );
            if let Some(path_str) = path_str {
                if preserve_browser_context {
                    preserve_sample_browser_context_for_loaded_sample(
                        &mut editor,
                        &path_str,
                    );
                }
                let path = Path::new(&path_str);
                let groups_before = app.groups.clone();
                match app.graph_controller().add_track(path) {
                    Ok(idx) => {
                        host_commands::add_new_track_to_group(
                            &mut app,
                            idx,
                            group_id,
                            rack_pad_note,
                        );
                        if let Err(error) = app.commit_created_track(idx, "Add sample track") {
                            app.groups = groups_before;
                            *track_groups.lock().unwrap() = app.groups.clone();
                            editor.handle_host_event(HostEvent::Status(format!(
                                "Error adding track: {error}"
                            )));
                            return;
                        }
                        *track_groups.lock().unwrap() = app.groups.clone();
                        register_waveform_sample(path);
                        let selected = host_commands::selection_after_added_track(
                            idx,
                            rack_pad_note,
                            &current_track,
                            app.tracks.len(),
                        );
                        current_track.store(selected, Ordering::Relaxed);
                        let new_name = app.tracks[idx].clone();
                        ctx.track_names.push(new_name.clone());
                        // Update pan IDs for new track
                        {
                            let mut pan_ids = track_pan_ids.lock().unwrap();
                            pan_ids.push(app.graph.track_node_ids[idx].pan_id);
                            push_solo_mutes(lg_raw, &state);
                        }
                        // Extend record_armed for new track
                        record_armed.lock().unwrap().push(false);
                        // Update reactive state
                        let rt = editor.runtime_mut();
                        rt.set_reactive(
                            "SEQ",
                            "num-tracks",
                            Value::Number(ctx.track_names.len() as f64),
                        );
                        rt.set_reactive("SEQ", "track-ids", build_track_ids(&app));
                        set_current_track_reactive(rt, app.tracks.len(), selected);
                        rt.set_reactive(
                            "SEQ",
                            "track-names",
                            build_track_names(&ctx.track_names),
                        );
                        sync_all_track_sequencer_state(
                            rt,
                            &state,
                            &app,
                            selected,
                            &selected_steps,
                        );
                        rt.set_reactive("SEQ", "steps", build_steps_value(&state, selected));
                        sync_step_param_lists(rt, &state, selected);
                        sync_track_mixer_state(rt, &app, &state);
                        sync_groups_bindings(rt, &app.groups);
                        sync_bus_mixer_state(rt, &app);
                        sync_track_peak_fields(rt, &ctx.meters.cached_track_peak_levels);
                        sync_bus_peak_fields(rt, &ctx.meters.cached_bus_peak_levels);
                        rt.set_reactive(
                            "SEQ",
                            "effects",
                            build_effects_value(
                                &state,
                                selected,
                                &app.graph.effect_descriptors,
                                &selected_steps,
                            ),
                        );
                        rt.set_reactive(
                            "SEQ",
                            "midi-effects",
                            build_midi_effects_value(&state, selected, &selected_steps),
                        );
                        rt.set_reactive(
                            "SEQ",
                            "instrument-panel",
                            build_instrument_panel_value(&app, selected, &selected_steps),
                        );
                        *accumulator_names.lock().unwrap() =
                            build_accumulator_names(&app);
                        let selected_neural_snapshot =
                            selected_neural_neurons.lock().unwrap().clone();
                        sync_track_params_with_neural_selection(
                            rt,
                            &app,
                            &state,
                            selected,
                            &selected_steps,
                            Some(&selected_neural_snapshot),
                        );
                        sync_fx_param_binding_fields_with_neural_selection(
                            rt,
                            &app,
                            &state,
                            selected,
                            &selected_steps,
                            Some(&selected_neural_snapshot),
                        );
                        rt.set_reactive(
                            "SEQ",
                            "step-has-plocks",
                            build_step_has_plocks(
                                &state,
                                selected,
                                &app.graph.effect_descriptors,
                            ),
                        );
                        rt.run_reactive_cycle();
                        editor.refresh_runtime_side_effects();
                        ui_epoch.fetch_add(1, Ordering::Relaxed);
                        editor.handle_host_event(HostEvent::Status(format!(
                            "Added track {}: {new_name}",
                            idx + 1
                        )));
                    }
                    Err(e) => {
                        if preserve_browser_context {
                            preserve_sample_browser_context_for_loaded_sample(
                                &mut editor,
                                "",
                            );
                        }
                        editor.handle_host_event(HostEvent::Status(format!(
                            "Error adding track: {e}"
                        )));
                    }
                }
            }
        }
        "add-track-instrument" | "swap-track-instrument" => {
            if let Some(pending) = ctx.sessions.pending_saved_instrument_load.as_ref() {
                let escaped = escape_lisp_string(&pending.name);
                let _ = editor.runtime_mut().eval_str(&format!(
                    "(set! sbrowser-loading-instrument-name \"{escaped}\")"
                ));
                editor.handle_host_event(HostEvent::Status(
                    "An instrument is already loading".to_string(),
                ));
                return;
            }
            let Some(instrument_name) = extract_string_from_payload(&payload, "name")
            else {
                let _ = editor
                    .runtime_mut()
                    .eval_str("(set! sbrowser-loading-instrument-name \"\")");
                editor.handle_host_event(HostEvent::Status(
                    "Instrument load is missing a name".to_string(),
                ));
                return;
            };
            let preserve_track_selection =
                extract_bool_from_payload(&payload, "preserve-track-selection");
            let target = if name == "swap-track-instrument" {
                let Some(track) = extract_usize_from_payload(&payload, "track") else {
                    let _ = editor
                        .runtime_mut()
                        .eval_str("(set! sbrowser-loading-instrument-name \"\")");
                    editor.handle_host_event(HostEvent::Status(
                        "Instrument swap is missing a track".to_string(),
                    ));
                    return;
                };
                match capture_instrument_swap_target(
                    &app,
                    track,
                    preserve_track_selection,
                ) {
                    Ok(target) => target,
                    Err(error) => {
                        let _ = editor
                            .runtime_mut()
                            .eval_str("(set! sbrowser-loading-instrument-name \"\")");
                        editor.handle_host_event(HostEvent::Status(format!(
                            "Cannot swap instrument: {error}"
                        )));
                        return;
                    }
                }
            } else {
                SavedInstrumentLoadTarget::AddTrack {
                    group_id: extract_usize_from_payload(&payload, "group-id")
                        .map(|group_id| group_id as u64),
                    // Dropping an instrument on an empty drum-rack pad: the new
                    // track becomes that pad's member, exactly as the sample
                    // path above (docs/drum-rack-v2-spec.md, "Track budget").
                    pad_note: extract_i32_from_payload(&payload, "pad-note"),
                }
            };
            let escaped = escape_lisp_string(&instrument_name);
            let _ = editor.runtime_mut().eval_str(&format!(
                "(set! sbrowser-loading-instrument-name \"{escaped}\")"
            ));
            let source =
                match sequencer::lisp_host::load_instrument_source(&instrument_name) {
                    Ok(source) => source,
                    Err(error) => {
                        let _ = editor
                            .runtime_mut()
                            .eval_str("(set! sbrowser-loading-instrument-name \"\")");
                        editor.handle_host_event(HostEvent::Status(format!(
                            "Error loading instrument source: {error}"
                        )));
                        return;
                    }
                };
            let run_mode = match sequencer::lisp_host::load_instrument_run_mode(
                &instrument_name,
            ) {
                Ok(run_mode) => run_mode,
                Err(error) => {
                    let _ = editor
                        .runtime_mut()
                        .eval_str("(set! sbrowser-loading-instrument-name \"\")");
                    editor.handle_host_event(HostEvent::Status(format!(
                        "Error loading instrument metadata: {error}"
                    )));
                    return;
                }
            };
            if let Some(cached_result) = try_apply_cached_saved_instrument(
                &mut app,
                target,
                &instrument_name,
                &source,
                run_mode,
            ) {
                let _ = editor
                    .runtime_mut()
                    .eval_str("(set! sbrowser-loading-instrument-name \"\")");
                match cached_result {
                    Ok(SavedInstrumentLoadApply::Added { track, group_id, pad_note }) => {
                        finish_added_instrument_track(
                            track,
                            AddTrackInstrumentCtx {
                                app: &mut app,
                                editor: &mut editor,
                                state: &state,
                                current_track: &current_track,
                                track_names: &mut *ctx.track_names,
                                track_pan_ids: &track_pan_ids,
                                record_armed: &record_armed,
                                selected_steps: &selected_steps,
                                accumulator_names: &accumulator_names,
                                cached_track_peak_levels: &ctx.meters.cached_track_peak_levels,
                                group_id,
                                pad_note,
                                track_groups: &track_groups,
                                ui_epoch: &ui_epoch,
                                lg_raw,
                            },
                        )
                    }
                    Ok(SavedInstrumentLoadApply::Swapped {
                        track,
                        summary,
                        preserve_track_selection,
                    }) => {
                        finish_swapped_instrument_track(
                            &instrument_name,
                            track,
                            summary,
                            preserve_track_selection,
                            SwapTrackInstrumentCtx {
                                app: &mut app,
                                editor: &mut editor,
                                state: &state,
                                current_track: &current_track,
                                track_names: &mut *ctx.track_names,
                                selected_steps: &selected_steps,
                                fx_epoch: &fx_epoch,
                                ui_epoch: &ui_epoch,
                            },
                        )
                    }
                    Err(error) => {
                        let action = match target {
                            SavedInstrumentLoadTarget::AddTrack { .. } => {
                                "adding instrument track"
                            }
                            SavedInstrumentLoadTarget::SwapTrack { .. } => {
                                "swapping instrument"
                            }
                        };
                        editor.handle_host_event(HostEvent::Status(format!(
                            "Error {action}: {error}"
                        )))
                    }
                }
                return;
            }
            let sample_rate = app.graph.sample_rate;
            let asset_base =
                sequencer::lisp_host::instrument_source_path(&instrument_name)
                    .ok()
                    .and_then(|path| path.parent().map(|parent| parent.to_path_buf()));
            let compile_source = source.clone();
            let (tx, rx) = std::sync::mpsc::channel();
            std::thread::spawn(move || {
                let result =
                    sequencer::lisp_host::compile_and_load_instrument_with_asset_base(
                        &compile_source,
                        sample_rate,
                        asset_base.as_deref(),
                    );
                let _ = tx.send(result);
            });
            ctx.sessions.pending_saved_instrument_load = Some(PendingSavedInstrumentLoad {
                name: instrument_name.clone(),
                source,
                run_mode,
                target,
                receiver: rx,
            });
            let action = match target {
                SavedInstrumentLoadTarget::AddTrack { .. } => "Loading instrument",
                SavedInstrumentLoadTarget::SwapTrack { .. } => {
                    "Loading instrument for swap"
                }
            };
            editor.handle_host_event(HostEvent::Status(format!(
                "{action}: {instrument_name}"
            )));
            editor.mark_needs_redraw();
        }
        "delete-track" => {
            let track = match &payload {
                Value::Map(map) => {
                    map.get("track").and_then(|cell| match &*cell.borrow() {
                        Value::Number(n) => Some(*n as usize),
                        _ => None,
                    })
                }
                Value::Number(n) => Some(*n as usize),
                _ => None,
            }
            .unwrap_or_else(|| current_track.load(Ordering::Relaxed));
            let request_id = if state.is_playing() {
                let request_id = state.request_track_delete_boundary(track);
                let wait_deadline = Instant::now() + Duration::from_millis(250);
                while !state.topology_edit_ready(request_id)
                    && Instant::now() < wait_deadline
                {
                    std::thread::sleep(Duration::from_millis(1));
                }
                if !state.topology_edit_ready(request_id) {
                    state.complete_topology_edit(request_id);
                    state.publish_scheduler_snapshot();
                    editor.handle_host_event(HostEvent::Status(
                        "Delete timed out waiting for playback boundary".to_string(),
                    ));
                    return;
                }
                Some(request_id)
            } else {
                None
            };

            match app.delete_track_recorded(track) {
                Ok(new_idx) => {
                    if let Some(request_id) = request_id {
                        state.complete_topology_edit(request_id);
                        state.publish_scheduler_snapshot();
                    }
                    sync_after_track_topology_delete(&mut app, &mut editor, ctx, new_idx);
                    editor.handle_host_event(HostEvent::Status(format!(
                        "Deleted track {}",
                        track + 1
                    )));
                }
                Err(e) => {
                    if let Some(request_id) = request_id {
                        state.complete_topology_edit(request_id);
                        state.publish_scheduler_snapshot();
                    }
                    editor.handle_host_event(HostEvent::Status(format!(
                        "Error deleting track: {e}"
                    )));
                }
            }
        }
        "delete-tracks" => {
            let Some(mut tracks) = extract_track_indices(&payload) else {
                editor.handle_host_event(HostEvent::Status(
                    "Delete tracks requires a list of track indices".to_string(),
                ));
                return;
            };
            tracks.sort_unstable();
            tracks.dedup();
            if tracks.len() < 2 {
                editor.handle_host_event(HostEvent::Status(
                    "Delete tracks requires at least two tracks".to_string(),
                ));
                return;
            }
            let boundary_track = tracks[0];
            let request_id = if state.is_playing() {
                let request_id = state.request_track_delete_boundary(boundary_track);
                let wait_deadline = Instant::now() + Duration::from_millis(250);
                while !state.topology_edit_ready(request_id)
                    && Instant::now() < wait_deadline
                {
                    std::thread::sleep(Duration::from_millis(1));
                }
                if !state.topology_edit_ready(request_id) {
                    state.complete_topology_edit(request_id);
                    state.publish_scheduler_snapshot();
                    editor.handle_host_event(HostEvent::Status(
                        "Delete timed out waiting for playback boundary".to_string(),
                    ));
                    return;
                }
                Some(request_id)
            } else {
                None
            };

            match app.delete_tracks_recorded(tracks.clone()) {
                Ok(new_idx) => {
                    if let Some(request_id) = request_id {
                        state.complete_topology_edit(request_id);
                        state.publish_scheduler_snapshot();
                    }
                    selected_tracks.lock().unwrap().clear();
                    if active_delete_target.lock().unwrap().take().is_some() {
                        active_delete_target_version.fetch_add(1, Ordering::Relaxed);
                    }
                    sync_after_track_topology_delete(&mut app, &mut editor, ctx, new_idx);
                    editor.handle_host_event(HostEvent::Status(format!(
                        "Deleted {} tracks",
                        tracks.len()
                    )));
                }
                Err(error) => {
                    if let Some(request_id) = request_id {
                        state.complete_topology_edit(request_id);
                        state.publish_scheduler_snapshot();
                    }
                    editor.handle_host_event(HostEvent::Status(format!(
                        "Error deleting tracks: {error}"
                    )));
                }
            }
        }
        "delete-track-group" => {
            let Some(group_id) = extract_usize_from_payload(&payload, "group-id")
                .map(|id| id as u64)
            else {
                editor.handle_host_event(HostEvent::Status(
                    "Delete track group requires a group id".to_string(),
                ));
                return;
            };
            let boundary_track = app.groups.iter()
                .find(|group| group.id == group_id)
                .and_then(|group| group.members.first().copied())
                .unwrap_or(0);
            // A lazily-populated rack can own zero tracks. Deleting one must not
            // yank the selection to the last track, so keep the current one.
            let owns_tracks = app.groups.iter()
                .find(|group| group.id == group_id)
                .is_some_and(|group| {
                    !group.members.is_empty()
                        || group.rack_members.iter().any(|rack_id| {
                            app.groups.iter().any(|rack| {
                                rack.id == *rack_id && !rack.members.is_empty()
                            })
                        })
                });
            let request_id = if state.is_playing() {
                let request_id = state.request_track_delete_boundary(boundary_track);
                let wait_deadline = Instant::now() + Duration::from_millis(250);
                while !state.topology_edit_ready(request_id) && Instant::now() < wait_deadline {
                    std::thread::sleep(Duration::from_millis(1));
                }
                if !state.topology_edit_ready(request_id) {
                    state.complete_topology_edit(request_id);
                    state.publish_scheduler_snapshot();
                    editor.handle_host_event(HostEvent::Status(
                        "Delete timed out waiting for playback boundary".to_string(),
                    ));
                    return;
                }
                Some(request_id)
            } else {
                None
            };
            match app.delete_group_with_members_recorded(group_id) {
                Ok(new_idx) => {
                    if let Some(request_id) = request_id {
                        state.complete_topology_edit(request_id);
                        state.publish_scheduler_snapshot();
                    }
                    let new_idx = if owns_tracks {
                        new_idx
                    } else {
                        current_track
                            .load(Ordering::Relaxed)
                            .min(app.tracks.len().saturating_sub(1))
                    };
                    sync_after_track_topology_delete(&mut app, &mut editor, ctx, new_idx);
                    editor.handle_host_event(HostEvent::Status(
                        "Deleted track group".to_string(),
                    ));
                }
                Err(error) => {
                    if let Some(request_id) = request_id {
                        state.complete_topology_edit(request_id);
                        state.publish_scheduler_snapshot();
                    }
                    editor.handle_host_event(HostEvent::Status(format!(
                        "Error deleting track group: {error}"
                    )));
                }
            }
        }
        "load-sound-onto-track" => {
            let preserve_track_selection =
                extract_bool_from_payload(&payload, "preserve-track-selection");
            let track = extract_usize_from_payload(&payload, "track")
                .or_else(|| current_track_for_app(&mut app, &current_track));
            let path = extract_path_from_payload(&payload);
            match (track, path) {
                (Some(track), Some(path)) => {
                    match app.load_sound_onto_track(track, Path::new(&path)) {
                        Ok(()) => {
                            sync_after_instrument_track_apply_with_selection(
                                &mut app,
                                &mut editor,
                                &state,
                                track,
                                &current_track,
                                &mut *ctx.track_names,
                                &track_pan_ids,
                                &record_armed,
                                &selected_steps,
                                &accumulator_names,
                                &ctx.meters.cached_track_peak_levels,
                                &ctx.meters.cached_bus_peak_levels,
                                &ui_epoch,
                                lg_raw,
                                preserve_track_selection,
                            );
                            fx_epoch.fetch_add(1, Ordering::Relaxed);
                            editor.handle_host_event(HostEvent::Status(
                                "Loaded Sound".to_string(),
                            ));
                        }
                        Err(error) => editor.handle_host_event(HostEvent::Status(
                            format!("Error loading Sound: {error}"),
                        )),
                    }
                }
                _ => editor.handle_host_event(HostEvent::Status(
                    "Loading a Sound requires a track and path".to_string(),
                )),
            }
        }
        "add-track-from-sound" => {
            let path = extract_path_from_payload(&payload);
            match path {
                Some(path) => match app.add_track_from_sound(Path::new(&path))
                    .and_then(|track| {
                        app.commit_created_track(track, "Add Sound track")?;
                        Ok(track)
                    }) {
                    Ok(track) => {
                        sync_after_instrument_track_apply(
                            &mut app,
                            &mut editor,
                            &state,
                            track,
                            &current_track,
                            &mut *ctx.track_names,
                            &track_pan_ids,
                            &record_armed,
                            &selected_steps,
                            &accumulator_names,
                            &ctx.meters.cached_track_peak_levels,
                            &ctx.meters.cached_bus_peak_levels,
                            &ui_epoch,
                            lg_raw,
                        );
                        fx_epoch.fetch_add(1, Ordering::Relaxed);
                        editor.handle_host_event(HostEvent::Status(
                            "Added track from Sound".to_string(),
                        ));
                    }
                    Err(error) => editor.handle_host_event(HostEvent::Status(format!(
                        "Error adding Sound track: {error}"
                    ))),
                },
                None => editor.handle_host_event(HostEvent::Status(
                    "Sound drop is missing a path".to_string(),
                )),
            }
        }
        "group-selected-tracks" => {
            // Fold the multi-selected tracks into a new group backed by
            // an auto-created bus. Reject if <2 tracks or any member is
            // already grouped (one group per track).
            let members: Vec<usize> = {
                let set = selected_tracks.lock().unwrap();
                let mut v: Vec<usize> = set
                    .iter()
                    .copied()
                    .filter(|&t| t < app.tracks.len())
                    .collect();
                v.sort_unstable();
                v
            };
            // A selected rack member stands in for its whole rack: the rack
            // joins as one unit (docs/drum-rack-v2-spec.md, "Racks inside
            // track groups") instead of being torn apart track by track.
            let mut racks: Vec<u64> = Vec::new();
            let mut loose: Vec<usize> = Vec::new();
            for track in &members {
                match app
                    .groups
                    .iter()
                    .find(|group| group.members.contains(track))
                {
                    Some(group) if group.is_rack() => {
                        if !racks.contains(&group.id) {
                            racks.push(group.id);
                        }
                    }
                    // A track in a plain group is already grouped; leave it.
                    Some(_) => loose.push(*track),
                    None => loose.push(*track),
                }
            }
            let already_grouped = loose
                .iter()
                .any(|m| app.groups.iter().any(|g| g.members.contains(m)))
                || racks
                    .iter()
                    .any(|rack| app.rack_parent_group(*rack).is_some());
            if loose.len() + racks.len() >= 2 && !already_grouped {
                let Ok(bus) = app.group_tracks_and_racks_recorded(loose.clone(), racks.clone())
                else {
                    editor.handle_host_event(HostEvent::Status(
                        "Could not group the selected tracks".to_string(),
                    ));
                    return;
                };
                let selected_bus_index = app
                    .buses
                    .iter()
                    .position(|candidate| candidate.id == bus)
                    .expect("new group backing bus must be present in app buses");
                selected_tracks.lock().unwrap().clear();
                if active_delete_target.lock().unwrap().take().is_some() {
                    active_delete_target_version.fetch_add(1, Ordering::Relaxed);
                }
                *bus_state.lock().unwrap() = app.buses.clone();
                *bus_node_ids.lock().unwrap() = app.graph.bus_node_ids.clone();
                *track_groups.lock().unwrap() = app.groups.clone();
                let ct = current_track.load(Ordering::Relaxed);
                let rt = editor.runtime_mut();
                sync_track_mixer_state(rt, &app, &state);
                sync_bus_mixer_state(rt, &app);
                sync_groups_bindings(rt, &app.groups);
                sync_selected_tracks_bindings(
                    rt,
                    app.tracks.len(),
                    ct,
                    &HashSet::new(),
                );
                let _ =
                    rt.eval_str(&format!("(set! eseq.seq-core-state/selected-bus {selected_bus_index})"));
                rt.run_reactive_cycle();
                editor.refresh_runtime_side_effects();
                ui_epoch.fetch_add(1, Ordering::Relaxed);
            }
        }
        "move-rack-to-group" => {
            // Drag-drop a rack (by GroupId) onto the plain group at `gidx`. The
            // rack joins as one unit and its bus chains into the parent's.
            let rack = extract_usize_from_payload(&payload, "rack").map(|id| id as u64);
            let gidx = extract_usize_from_payload(&payload, "gidx");
            if let (Some(rack), Some(gidx)) = (rack, gidx) {
                if app.move_rack_to_group_recorded(rack, gidx).is_ok() {
                    *bus_state.lock().unwrap() = app.buses.clone();
                    *bus_node_ids.lock().unwrap() = app.graph.bus_node_ids.clone();
                    *track_groups.lock().unwrap() = app.groups.clone();
                    let rt = editor.runtime_mut();
                    sync_track_mixer_state(rt, &app, &state);
                    sync_bus_mixer_state(rt, &app);
                    sync_groups_bindings(rt, &app.groups);
                    rt.run_reactive_cycle();
                    editor.refresh_runtime_side_effects();
                    ui_epoch.fetch_add(1, Ordering::Relaxed);
                }
            }
        }
        "remove-rack-from-group" => {
            // Pull a rack back out of its parent group; its bus returns to the
            // master mix and the parent dissolves if it drops below two units.
            let rack = extract_usize_from_payload(&payload, "rack").map(|id| id as u64);
            if let Some(rack) = rack {
                if app.remove_rack_from_group_recorded(rack).is_ok() {
                    *bus_state.lock().unwrap() = app.buses.clone();
                    *bus_node_ids.lock().unwrap() = app.graph.bus_node_ids.clone();
                    *track_groups.lock().unwrap() = app.groups.clone();
                    let rt = editor.runtime_mut();
                    sync_track_mixer_state(rt, &app, &state);
                    sync_bus_mixer_state(rt, &app);
                    sync_groups_bindings(rt, &app.groups);
                    rt.run_reactive_cycle();
                    editor.refresh_runtime_side_effects();
                    ui_epoch.fetch_add(1, Ordering::Relaxed);
                }
            }
        }
        "move-track-to-group" => {
            // Drag-drop: add `track` to the group at `gidx` (moving it
            // out of any other group first). Dissolve a source group
            // that would drop below 2 members.
            let track = extract_usize_from_payload(&payload, "track");
            let gidx = extract_usize_from_payload(&payload, "gidx");
            if let (Some(track), Some(gidx)) = (track, gidx) {
                if app.move_track_to_group_recorded(track, gidx).is_ok() {
                        *bus_state.lock().unwrap() = app.buses.clone();
                        *bus_node_ids.lock().unwrap() = app.graph.bus_node_ids.clone();
                        *track_groups.lock().unwrap() = app.groups.clone();
                        let rt = editor.runtime_mut();
                        sync_track_mixer_state(rt, &app, &state);
                        sync_bus_mixer_state(rt, &app);
                        sync_groups_bindings(rt, &app.groups);
                        rt.run_reactive_cycle();
                        editor.refresh_runtime_side_effects();
                        ui_epoch.fetch_add(1, Ordering::Relaxed);
                }
            }
        }
        "remove-track-from-group" => {
            // Drag-drop onto the sample zone: pull `track` out of its
            // group, routing it back to the master mix. Dissolve the
            // group if it would fall below 2 members.
            if let Some(track) = extract_usize_from_payload(&payload, "track") {
                if app.remove_track_from_group_recorded(track).is_ok() {
                    *bus_state.lock().unwrap() = app.buses.clone();
                    *bus_node_ids.lock().unwrap() = app.graph.bus_node_ids.clone();
                    *track_groups.lock().unwrap() = app.groups.clone();
                    let rt = editor.runtime_mut();
                    sync_track_mixer_state(rt, &app, &state);
                    sync_bus_mixer_state(rt, &app);
                    sync_groups_bindings(rt, &app.groups);
                    rt.run_reactive_cycle();
                    editor.refresh_runtime_side_effects();
                    ui_epoch.fetch_add(1, Ordering::Relaxed);
                }
            }
        }
        _ => {}
    }
}
