use crate::*;

pub(super) const COMMANDS: &[&str] = &[
    "move-saved-instrument",
    "load-instrument-preset",
    "save-preset",
    "overwrite-preset",
    "new-project",
    "save-project",
    "promote-preset-to-sound",
    "load-project",
];

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
    let piano_roll_selection = ctx.shared.piano_roll_selection.clone();
    let ui_epoch = ctx.shared.ui_epoch.clone();
    let fx_epoch = ctx.shared.fx_epoch.clone();
    let ui_invalidations = ctx.shared.ui_invalidations.clone();
    let expanded_step_projection = ctx.shared.expanded_step_projection.clone();
    let track_pan_ids = ctx.shared.track_pan_ids.clone();
    let track_collapsed = ctx.shared.track_collapsed.clone();
    let bus_state = ctx.shared.bus_state.clone();
    let bus_node_ids = ctx.shared.bus_node_ids.clone();
    let track_groups = ctx.shared.track_groups.clone();
    let record_armed = ctx.shared.record_armed.clone();
    let accumulator_names = ctx.shared.accumulator_names.clone();
    let arrangement_clipboard = ctx.shared.arrangement_clipboard.clone();
    match name {
        "move-saved-instrument" => {
            let name = extract_string_from_payload(&payload, "name");
            let folder = extract_string_from_payload(&payload, "folder");
            match (name, folder) {
                (Some(name), Some(folder)) => {
                    match sequencer::lisp_host::move_saved_instrument(&name, &folder) {
                        Ok(new_name) => {
                            if let Err(error) = editor
                                .runtime_mut()
                                .eval_str("(eseq.browser/refresh-buffer)")
                            {
                                eprintln!(
                                    "instrument browser: failed to refresh after move: {error:?}"
                                );
                            }
                            editor.refresh_runtime_side_effects();
                            editor.handle_host_event(HostEvent::Status(format!(
                                "Moved instrument: {new_name}"
                            )));
                            editor.mark_needs_redraw();
                        }
                        Err(error) => {
                            editor.handle_host_event(HostEvent::Status(format!(
                                "Error moving instrument: {error}"
                            )));
                        }
                    }
                }
                _ => editor.handle_host_event(HostEvent::Status(
                    "Instrument move is missing a name or folder".to_string(),
                )),
            }
        }
        "load-instrument-preset" => {
            if let Value::Map(ref map) = payload {
                let preset_name =
                    map.get("name").and_then(|cell| match &*cell.borrow() {
                        Value::String(name) => Some(name.clone()),
                        _ => None,
                    });
                if let Some(preset_name) = preset_name {
                    let track = current_track.load(Ordering::Relaxed);
                    let is_rack = app.graph.track_instrument_types.get(track)
                        == Some(&sequencer::sequencer::InstrumentType::Rack);
                    let load_result = if is_rack {
                        app.load_rack_preset_onto_track(track, &preset_name)
                    } else {
                        load_instrument_preset_into_track(&mut app, track, &preset_name)
                    };
                    match load_result {
                        Ok(()) => {
                            if is_rack {
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
                            } else {
                                let rt = editor.runtime_mut();
                                rt.set_reactive(
                                    "SEQ",
                                    "instrument-panel",
                                    build_instrument_panel_value(
                                        &app,
                                        track,
                                        &selected_steps,
                                    ),
                                );
                                sync_sidebar_browser(rt, &app, track);
                                rt.run_reactive_cycle();
                                editor.refresh_runtime_side_effects();
                                ui_epoch.fetch_add(1, Ordering::Relaxed);
                            }
                            editor.handle_host_event(HostEvent::Status(format!(
                                "Loaded preset '{preset_name}'"
                            )));
                        }
                        Err(e) => {
                            editor.handle_host_event(HostEvent::Status(format!(
                                "Error loading preset: {e}"
                            )));
                        }
                    }
                }
            }
        }
        "save-preset" => {
            if let Value::Map(ref map) = payload {
                let preset_name =
                    map.get("name").and_then(|cell| match &*cell.borrow() {
                        Value::String(s) => Some(s.clone()),
                        _ => None,
                    });
                let overwrite = map
                    .get("overwrite")
                    .map(|cell| match &*cell.borrow() {
                        Value::Bool(b) => *b,
                        _ => false,
                    })
                    .unwrap_or(false);
                if let Some(name) = preset_name {
                    let name = name.trim().to_string();
                    if name.is_empty() {
                        editor.handle_host_event(HostEvent::Status(
                            "Preset name cannot be empty".to_string(),
                        ));
                    } else {
                        let track = current_track.load(Ordering::Relaxed);
                        app.ui.cursor_track = track;
                        let save_result =
                            app.save_current_track_as_preset(&name, overwrite);
                        // Refresh sidebar presets list
                        let rt = editor.runtime_mut();
                        sync_sidebar_browser(rt, &app, track);
                        rt.run_reactive_cycle();
                        editor.refresh_runtime_side_effects();
                        match save_result {
                            Ok(()) => editor.handle_host_event(HostEvent::Status(
                                format!("Saved preset '{name}'"),
                            )),
                            Err(error) => editor.handle_host_event(HostEvent::Status(
                                format!("Error saving preset: {error}"),
                            )),
                        }
                    }
                }
            }
        }
        "overwrite-preset" => {
            let track = current_track.load(Ordering::Relaxed);
            app.ui.cursor_track = track;
            app.overwrite_loaded_preset();
            let rt = editor.runtime_mut();
            sync_sidebar_browser(rt, &app, track);
            rt.run_reactive_cycle();
            editor.refresh_runtime_side_effects();
        }
        "new-project" => {
            app.start_new_project();
            if let Err(error) = clear_project_script_tabs(&mut editor) {
                editor.handle_host_event(HostEvent::Status(error));
            }
            push_project_scratch_to_named_buffer(&mut editor, &app);
            if let Err(error) =
                evaluate_project_scratch_on_ui_runtime(&mut editor, &app)
            {
                editor.handle_host_event(HostEvent::Status(format!(
                    "Scratch UI eval error: {error}"
                )));
            }
            selected_steps.lock().unwrap().clear();
            piano_roll_selection.lock().unwrap().clear();
            // The arrangement clipboard stores pattern/take IDS, not content:
            // kept across a project switch it would paste whatever happens to
            // carry those ids now (region spec 5.1).
            *arrangement_clipboard.lock().unwrap() = None;
            *ctx.track_names = app.tracks.clone();
            sync_shared_track_collapsed(&track_collapsed, &app);
            current_track.store(0, Ordering::Relaxed);
            {
                let mut pan_ids = track_pan_ids.lock().unwrap();
                pan_ids.clear();
                push_solo_mutes(lg_raw, app, &state);
            }
            *bus_node_ids.lock().unwrap() = app.graph.bus_node_ids.clone();
            *record_armed.lock().unwrap() = Vec::new();
            // Keep the shared bus mirror in sync so pull_shared_bus_state
            // can't restore the previous project's buses.
            *bus_state.lock().unwrap() = app.buses.clone();
            // Clear group state so the new project starts ungrouped and
            // the frame diff doesn't restore the previous project's groups.
            *track_groups.lock().unwrap() = app.groups.clone();
            selected_tracks.lock().unwrap().clear();
            *accumulator_names.lock().unwrap() = build_accumulator_names(&app);
            ctx.meters.cached_track_peak_levels.clear();
            ctx.meters.cached_bus_peak_levels =
                read_bus_peak_levels(app.graph.lg, &app.graph.bus_node_ids);
            (ctx.meters.cached_modulator_phases, ctx.meters.cached_modulator_levels) =
                read_modulator_display_values(app.graph.lg, &app);
            ctx.meters.last_meter_poll_at = Instant::now();

            let bpm = state.transport.bpm.load(Ordering::Relaxed);
            let playing = state.transport.playing.load(Ordering::Relaxed);
            let transport_playhead = state.transport.playhead.load(Ordering::Relaxed);
            let rt = editor.runtime_mut();
            sync_pattern_state(rt, &state);
            sync_project_state(rt, &app);
            rt.set_reactive("SEQ", "playing", Value::Bool(playing));
            rt.set_reactive("SEQ", "bpm", Value::Number(bpm as f64));
            rt.set_reactive(
                "SEQ",
                "transport-playhead",
                Value::Number(transport_playhead as f64),
            );
            sync_bus_mixer_state(rt, &app);
            sync_groups_bindings(rt, &app.groups);
            sync_bus_peak_fields(rt, &ctx.meters.cached_bus_peak_levels);
            sync_modulator_phase_fields(rt, &ctx.meters.cached_modulator_phases);
            sync_modulator_level_fields(rt, &ctx.meters.cached_modulator_levels);
            rt.set_reactive("SEQ", "num-tracks", Value::Number(0.0));
            set_current_track_reactive(rt, 0, 0);
            rt.set_reactive("SEQ", "track-ids", Value::List(vec![]));
            rt.set_reactive("SEQ", "track-names", Value::List(vec![]));
            rt.set_reactive("SEQ", "record-armed", Value::List(vec![]));
            rt.set_reactive("SEQ", "selected-steps", Value::List(vec![]));
            sync_playhead_fields(rt, 0, 1);
            rt.set_reactive("SEQ", "steps", Value::List(vec![]));
            rt.set_reactive("SEQ", "velocities", Value::List(vec![]));
            rt.set_reactive("SEQ", "durations", Value::List(vec![]));
            rt.set_reactive("SEQ", "transposes", Value::List(vec![]));
            rt.set_reactive("SEQ", "pans", Value::List(vec![]));
            rt.set_reactive("SEQ", "syncs", Value::List(vec![]));
            rt.set_reactive("SEQ", "delays", Value::List(vec![]));
            sync_track_mixer_empty_state(rt);
            rt.set_reactive("SEQ", "effects", Value::List(vec![]));
            rt.set_reactive("SEQ", "midi-effects", Value::List(vec![]));
            rt.set_reactive("SEQ", "instrument-panel", Value::List(vec![]));
            rt.set_reactive("SEQ", "step-has-plocks", Value::List(vec![]));
            rt.set_reactive("SEQ", "track-steps", Value::List(vec![]));
            rt.set_reactive("SEQ", "track-num-steps", Value::List(vec![]));
            rt.set_reactive("SEQ", "track-duration-spans", Value::List(vec![]));
            rt.set_reactive("SEQ", "track-playheads", Value::List(vec![]));
            rt.set_reactive("SEQ", "track-step-has-plocks", Value::List(vec![]));
            sync_sidebar_browser(rt, &app, 0);
            rt.clear_subtree_effects_for_named_target("*sequencer*");
            rt.run_reactive_cycle();
            editor.refresh_runtime_side_effects();
            refresh_visible_track_topology_layouts(&mut editor);

            ctx.frame.prev_current_track = 0;
            ctx.frame.prev_playhead = 0;
            ctx.frame.prev_transport_playhead = transport_playhead;
            ctx.frame.prev_bpm = bpm;
            ctx.frame.prev_playing = playing;
            ctx.frame.prev_pattern_epoch = state.transport.pattern_epoch.load(Ordering::Relaxed);
            ctx.frame.prev_track_peak_levels.clear();
            ctx.frame.prev_modulator_phases = ctx.meters.cached_modulator_phases.clone();
            ctx.frame.prev_modulator_levels = ctx.meters.cached_modulator_levels.clone();
            ctx.frame.prev_track_playheads = track_playheads_snapshot(&state, &app);
            ctx.frame.prev_track_button_states = track_button_state_snapshot(&state);
            ctx.frame.prev_ui_epoch = ui_epoch.fetch_add(1, Ordering::Relaxed) + 1;

            editor.handle_host_event(HostEvent::Status("New project".to_string()));
        }
        "save-project" => {
            let _ = current_track_for_app(&mut app, &current_track);
            pull_named_scratch_buffer_into_project(&editor, &mut app);
            let requested_name = if let Value::Map(ref map) = payload {
                map.get("name").and_then(|cell| match &*cell.borrow() {
                    Value::String(name) => Some(name.clone()),
                    _ => None,
                })
            } else {
                None
            };
            match app.save_project_with_name(requested_name.as_deref()) {
                Ok(save_name) => {
                    let rt = editor.runtime_mut();
                    sync_project_state(rt, &app);
                    rt.run_reactive_cycle();
                    editor.refresh_runtime_side_effects();
                    editor.handle_host_event(HostEvent::Status(format!(
                        "Saved project '{save_name}'"
                    )));
                }
                Err(error) => {
                    editor.handle_host_event(HostEvent::Status(format!(
                        "Error saving project: {error}"
                    )));
                }
            }
        }
        "promote-preset-to-sound" => {
            let track = extract_usize_from_payload(&payload, "track")
                .or_else(|| current_track_for_app(&mut app, &current_track));
            let preset_name = extract_string_from_payload(&payload, "name");
            match (track, preset_name) {
                (Some(track), Some(name)) if !name.trim().is_empty() => {
                    app.ui.cursor_track = track;
                    match app.promote_preset_to_sound(track, &name) {
                        Ok(_) => {
                            let rt = editor.runtime_mut();
                            sync_project_state(rt, &app);
                            rt.run_reactive_cycle();
                            editor.refresh_runtime_side_effects();
                            editor.handle_host_event(HostEvent::Status(format!(
                                "Added preset '{name}' to Sounds"
                            )));
                        }
                        Err(error) => editor.handle_host_event(HostEvent::Status(
                            format!("Error adding preset to Sounds: {error}"),
                        )),
                    }
                }
                _ => editor.handle_host_event(HostEvent::Status(
                    "Preset promotion requires a track and preset name".to_string(),
                )),
            }
        }
        "load-project" => {
            let requested_name = if let Value::Map(ref map) = payload {
                map.get("name").and_then(|cell| match &*cell.borrow() {
                    Value::String(name) => Some(name.clone()),
                    _ => None,
                })
            } else {
                None
            };
            let Some(project_name) =
                requested_name.filter(|name| !name.trim().is_empty())
            else {
                editor.handle_host_event(HostEvent::Status(
                    "Error loading project: missing project name".to_string(),
                ));
                return;
            };
            eprintln!("metal_seq: host load-project name={project_name}");
            ui_invalidations.clear();
            expanded_step_projection.clear();
            match app.queue_project_load_named(&project_name) {
                Ok(()) => {
                    eprintln!("metal_seq: queued project load name={project_name}");
                    editor.handle_host_event(HostEvent::Status(format!(
                        "Opening project '{project_name}'..."
                    )));
                }
                Err(error) => {
                    eprintln!(
                        "metal_seq: queue project load failed name={} error={}",
                        project_name, error
                    );
                    editor.handle_host_event(HostEvent::Status(format!(
                        "Error loading project: {error}"
                    )));
                }
            }
        }
        _ => {}
    }
}
