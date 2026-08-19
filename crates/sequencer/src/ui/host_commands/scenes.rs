use crate::*;

pub(super) const COMMANDS: &[&str] = &[
    "set-scene-launch-quantize",
    "fork-track-pattern",
    "clone-track-pattern",
    "delete-track-pattern",
    "set-scene-cell",
    "clear-scene-cell",
    "launch-track-pattern",
    "switch-pattern",
    "rename-scene",
    "reorder-scene",
    "propagate-current-track-to-all-patterns",
    "clone-pattern",
    "delete-pattern",
];

#[allow(clippy::too_many_lines)]
pub(super) fn handle(
    name: &str,
    payload: Value,
    mut app: &mut app::App,
    editor: &mut Editor,
    ctx: &mut LoopCtx<'_>,
) {
    let state = ctx.shared.state.clone();
    let current_track = ctx.shared.current_track.clone();
    let selected_steps = ctx.shared.selected_steps.clone();
    let selected_neural_neurons = ctx.shared.selected_neural_neurons.clone();
    let piano_roll_selection = ctx.shared.piano_roll_selection.clone();
    let ui_epoch = ctx.shared.ui_epoch.clone();
    let fx_value_epoch = ctx.shared.fx_value_epoch.clone();
    let expanded_step_projection = ctx.shared.expanded_step_projection.clone();
    let active_delete_target = ctx.shared.active_delete_target.clone();
    let active_delete_target_version = ctx.shared.active_delete_target_version.clone();
    let track_collapsed = ctx.shared.track_collapsed.clone();
    let accumulator_names = ctx.shared.accumulator_names.clone();
    match name {
        "set-scene-launch-quantize" => {
            let Value::String(label) = payload else {
                editor.handle_host_event(HostEvent::Error(
                    "Scene launch quantization selection was invalid".to_string(),
                ));
                return;
            };
            let Some(quantize) =
                sequencer::quantized_launch::LaunchQuantize::from_transport_label(
                    &label,
                )
            else {
                editor.handle_host_event(HostEvent::Error(format!(
                    "Unknown scene launch quantization: {label}"
                )));
                return;
            };
            editor.runtime_mut().set_reactive(
                "SEQ",
                "scene-launch-quantize",
                Value::String(quantize.transport_label().to_string()),
            );
            editor.runtime_mut().run_reactive_cycle();
            editor.refresh_runtime_side_effects();
            editor.mark_needs_redraw();
        }
        "fork-track-pattern" => {
            let track = match payload {
                Value::Map(ref map) => map
                    .get("track")
                    .and_then(|cell| match &*cell.borrow() {
                        Value::Number(n) => Some(*n as usize),
                        _ => None,
                    })
                    .unwrap_or_else(|| current_track.load(Ordering::Relaxed)),
                Value::Number(n) => n as usize,
                _ => current_track.load(Ordering::Relaxed),
            };
            let num_tracks = app.tracks.len();
            if track >= num_tracks {
                editor.handle_host_event(HostEvent::Status(format!(
                    "Track pattern fork failed: track {} is out of range",
                    track + 1
                )));
                return;
            }
            let forked = app.apply_recorded_scene_structure_mutation(
                "Fork track pattern",
                |app| app.state.fork_current_track_pattern(
                    track,
                    num_tracks,
                    &app.graph.track_buffer_ids,
                    &app.graph.track_sample_rates,
                    &app.tracks,
                    &app.graph.track_instrument_types,
                ).ok_or_else(|| format!("Could not fork track {} pattern", track + 1)),
            );
            let Ok(pattern_id) = forked else {
                editor.handle_host_event(HostEvent::Status(format!(
                    "Track pattern fork failed for track {}",
                    track + 1
                )));
                return;
            };
            editor.handle_host_event(HostEvent::Status(format!(
                "Forked track {} pattern {}",
                track + 1,
                pattern_id.0
            )));
        }
        "clone-track-pattern" => {
            let (track, source_pattern_id) = match payload {
                Value::Map(ref map) => (
                    map.get("track")
                        .and_then(|cell| match &*cell.borrow() {
                            Value::Number(n) if *n >= 0.0 => Some(*n as usize),
                            _ => None,
                        })
                        .unwrap_or_else(|| current_track.load(Ordering::Relaxed)),
                    map.get("pattern-id")
                        .or_else(|| map.get("pattern_id"))
                        .and_then(|cell| match &*cell.borrow() {
                            Value::Number(n) if *n >= 0.0 => Some(PatternId(*n as u64)),
                            _ => None,
                        }),
                ),
                Value::Number(n) => (n as usize, None),
                _ => (current_track.load(Ordering::Relaxed), None),
            };
            let num_tracks = app.tracks.len();
            if track >= num_tracks {
                editor.handle_host_event(HostEvent::Status(format!(
                    "Track pattern clone failed: track {} is out of range",
                    track + 1
                )));
                return;
            }
            let cloned = app.apply_recorded_scene_structure_mutation(
                "Clone track pattern",
                |app| {
                    let cloned = if let Some(source_id) = source_pattern_id {
                        app.state.clone_track_pattern_id_into_current_scene(
                            track,
                            source_id,
                            num_tracks,
                            &app.graph.track_buffer_ids,
                            &app.graph.track_sample_rates,
                            &app.tracks,
                            &app.graph.track_instrument_types,
                        )
                    } else {
                        app.state.clone_current_scene_track_pattern(
                            track,
                            num_tracks,
                            &app.graph.track_buffer_ids,
                            &app.graph.track_sample_rates,
                            &app.tracks,
                            &app.graph.track_instrument_types,
                        )
                    };
                    cloned.ok_or_else(|| format!(
                        "Could not clone track {} pattern", track + 1
                    ))
                },
            );
            let Ok(pattern_id) = cloned else {
                editor.handle_host_event(HostEvent::Status(format!(
                    "Track pattern clone failed for track {}",
                    track + 1
                )));
                return;
            };
            let sample_ids = app.state.effective_pattern_sample_ids(num_tracks);
            app.graph_controller().apply_sample_ids(&sample_ids);
            if let Err(error) = app
                .graph_controller()
                .sync_track_instrument_run_modes_from_live_state()
            {
                app.editor.status_message = Some((
                    format!("Track pattern clone failed: {error}"),
                    Instant::now(),
                ));
            }
            app.push_all_restored_defaults();
            {
                let mut guard = active_delete_target.lock().unwrap();
                *guard = Some(ActiveDeleteTarget::TrackPattern { track, pattern_id });
            }
            active_delete_target_version.fetch_add(1, Ordering::Relaxed);
            ui_epoch.fetch_add(1, Ordering::Relaxed);
            editor.handle_host_event(HostEvent::Status(format!(
                "Cloned track {} pattern {}",
                track + 1,
                pattern_id.0
            )));
        }
        "delete-track-pattern" => {
            let Value::Map(ref map) = payload else {
                editor.handle_host_event(HostEvent::Status(
                    "Track pattern delete failed: invalid payload".to_string(),
                ));
                return;
            };
            let track = map.get("track").and_then(|cell| match &*cell.borrow() {
                Value::Number(n) => Some(*n as usize),
                _ => None,
            });
            let pattern_id = map
                .get("pattern-id")
                .or_else(|| map.get("pattern_id"))
                .and_then(|cell| match &*cell.borrow() {
                    Value::Number(n) if *n >= 1.0 => Some(*n as u64),
                    _ => None,
                });
            let (Some(track), Some(pattern_id)) = (track, pattern_id) else {
                editor.handle_host_event(HostEvent::Status(
                    "Track pattern delete failed: missing track or pattern id"
                        .to_string(),
                ));
                return;
            };
            let num_tracks = app.tracks.len();
            let deleted = app.apply_recorded_scene_structure_mutation(
                "Delete track pattern",
                |app| {
                    app.state.delete_track_pattern(
                        track,
                        PatternId(pattern_id),
                        num_tracks,
                        &app.graph.track_buffer_ids,
                        &app.graph.track_sample_rates,
                        &app.tracks,
                        &app.graph.track_instrument_types,
                    )?;
                    // The live sample arrays must match the restored
                    // replacement pattern before the wrapper
                    // re-snapshots live state into it, or the old
                    // sample clobbers the pattern's sample_id.
                    let sample_ids =
                        app.state.effective_pattern_sample_ids(num_tracks);
                    app.graph_controller().apply_sample_ids(&sample_ids);
                    Ok(())
                },
            );
            if let Err(error) = deleted {
                editor.handle_host_event(HostEvent::Status(format!(
                    "Track pattern delete failed: {error}"
                )));
                return;
            }
            if let Err(error) = app
                .graph_controller()
                .sync_track_instrument_run_modes_from_live_state()
            {
                app.editor.status_message = Some((
                    format!("Track pattern delete failed: {error}"),
                    Instant::now(),
                ));
            }
            app.push_all_restored_defaults();
            editor.handle_host_event(HostEvent::Status(format!(
                "Deleted track {} pattern {}",
                track + 1,
                pattern_id
            )));
        }
        "set-scene-cell" => {
            let Value::Map(ref map) = payload else {
                editor.handle_host_event(HostEvent::Status(
                    "Scene cell share failed: invalid payload".to_string(),
                ));
                return;
            };
            let scene = map.get("scene").and_then(|cell| match &*cell.borrow() {
                Value::Number(n) => Some(*n as usize),
                _ => None,
            });
            let track = map.get("track").and_then(|cell| match &*cell.borrow() {
                Value::Number(n) => Some(*n as usize),
                _ => None,
            });
            let pattern_id = map
                .get("pattern-id")
                .or_else(|| map.get("pattern_id"))
                .and_then(|cell| match &*cell.borrow() {
                    Value::Number(n) if *n >= 1.0 => Some(*n as u64),
                    _ => None,
                });
            let (Some(scene), Some(track), Some(pattern_id)) =
                (scene, track, pattern_id)
            else {
                editor.handle_host_event(HostEvent::Status(
                    "Scene cell share failed: missing scene, track, or pattern id"
                        .to_string(),
                ));
                return;
            };
            // During song playback authority a clip click is a PERFORMANCE,
            // not an edit: assigning the scene cell would retroactively
            // rewrite every committed row that resolves this scene (the
            // launched clip visually appearing in unrelated regions of the
            // timeline). Delegate to the non-destructive override launch —
            // it latches the lane and records into an active capture; the
            // scene's cell assignment is untouched.
            if app.song_playback_authority_active()
                && scene == app.state.current_scene_index()
            {
                handle("launch-track-pattern", payload, app, editor, ctx);
                return;
            }
            // Clip launches follow the same transport launch-quantize as
            // scene launches (`switch-pattern`): assign the cell now (the
            // edit), defer the audible restore to the quantized boundary via
            // a SceneTracks launch — the scheduler applies it with the same
            // chunk split scene launches use. Only a click into the CURRENT
            // scene is a launch; other scenes stay a plain edit.
            let quantize_label = extract_string_from_payload(&payload, "quantize")
                .unwrap_or_else(|| "off".to_string());
            let Some(quantize) =
                sequencer::quantized_launch::LaunchQuantize::from_transport_label(
                    &quantize_label,
                )
            else {
                editor.handle_host_event(HostEvent::Error(format!(
                    "Unknown scene launch quantization: {quantize_label}"
                )));
                return;
            };
            if scene == app.state.current_scene_index()
                && quantize != sequencer::quantized_launch::LaunchQuantize::Off
            {
                let num_tracks = app.tracks.len();
                let queued = app.apply_recorded_scene_structure_mutation(
                    "Assign scene cell",
                    |app| {
                        if !app.state.set_scene_cell_queued(
                            scene,
                            track,
                            PatternId(pattern_id),
                            num_tracks,
                            &app.graph.track_buffer_ids,
                            &app.graph.track_sample_rates,
                            &app.tracks,
                            &app.graph.track_instrument_types,
                        ) {
                            return Err(format!(
                                "Could not assign scene {} track {}",
                                scene + 1,
                                track + 1
                            ));
                        }
                        Ok(())
                    },
                );
                if queued.is_err() {
                    editor.handle_host_event(HostEvent::Status(format!(
                        "Scene cell share failed: scene {}, track {}, pattern {}",
                        scene + 1,
                        track + 1,
                        pattern_id
                    )));
                    return;
                }
                match state.schedule_quantized_pattern_launch(
                    sequencer::quantized_launch::PatternLaunchTarget::SceneTracks {
                        scene,
                        tracks: vec![track],
                    },
                    quantize,
                    sequencer::quantized_launch::QuantizedLaunchOwner::TrackClip(
                        track as u32,
                    ),
                ) {
                    Ok(token) => editor.handle_host_event(HostEvent::Status(format!(
                        "Queued track {} clip {} at {} (launch {})",
                        track + 1,
                        pattern_id,
                        quantize.transport_label(),
                        token
                    ))),
                    Err(error) => editor.handle_host_event(HostEvent::Error(format!(
                        "Could not queue clip launch: {error:?}"
                    ))),
                }
                return;
            }
            let profile = pattern_switch_profile_enabled();
            let profile_started = Instant::now();
            let num_tracks = app.tracks.len();
            let shared = app.apply_recorded_scene_structure_mutation(
                "Assign scene cell",
                |app| {
                    if !app.state.set_scene_cell(
                        scene,
                        track,
                        PatternId(pattern_id),
                        num_tracks,
                        &app.graph.track_buffer_ids,
                        &app.graph.track_sample_rates,
                        &app.tracks,
                        &app.graph.track_instrument_types,
                    ) {
                        return Err(format!(
                            "Could not assign scene {} track {}",
                            scene + 1,
                            track + 1
                        ));
                    }
                    // The live sample arrays must match the restored
                    // pattern before the wrapper re-snapshots live
                    // state into it, or the old sample clobbers the
                    // pattern's sample_id.
                    let sample_ids =
                        app.state.effective_pattern_sample_ids(num_tracks);
                    app.graph_controller().apply_sample_ids(&sample_ids);
                    Ok(())
                },
            );
            if shared.is_err() {
                editor.handle_host_event(HostEvent::Status(format!(
                    "Scene cell share failed: scene {}, track {}, pattern {}",
                    scene + 1,
                    track + 1,
                    pattern_id
                )));
                return;
            }
            let mutation_elapsed = profile_started.elapsed();
            let run_modes_started = Instant::now();
            if let Err(error) = app
                .graph_controller()
                .sync_track_instrument_run_modes_from_live_state()
            {
                app.editor.status_message =
                    Some((format!("Scene cell share failed: {error}"), Instant::now()));
            }
            let run_modes_elapsed = run_modes_started.elapsed();
            let defaults_started = Instant::now();
            app.push_all_restored_defaults();
            let defaults_elapsed = defaults_started.elapsed();
            let launch_observe_started = Instant::now();
            // Clicking a clip in the mixer grid IS the clip-launch gesture
            // (`launch-track-pattern` is a dead command — this is the live
            // path): latch the lane during song playback (takes spec 10,
            // the intentional way to claim a lane, take lanes included)
            // and record the launch into an active arrangement capture.
            // Only an assignment into the CURRENT scene is audible.
            if scene == app.state.current_scene_index() {
                app.observe_manual_clip_launch(track, PatternId(pattern_id));
            }
            let launch_observe_elapsed = launch_observe_started.elapsed();
            let fx_resync_started = Instant::now();
            // Assigning into the current scene live-restores the pattern's
            // params + sample; the generic pattern-epoch sync covers
            // steps/params/mixer but not the fx and instrument-panel
            // values, so bump fx_value_epoch and let the SAME tick cycle
            // carry them via the in-place value patch. (This used to resync
            // inline with its own reactive cycle + side-effects pass when
            // *fx* was visible — a whole extra ~35ms cycle at 20-clip pool
            // scale for surfaces the tick's fx branch republishes anyway.
            // Launches restore VALUES only, never panel structure, so the
            // patch path is safe here; structural edits use fx_epoch.)
            if scene == app.state.current_scene_index() {
                fx_value_epoch.fetch_add(1, Ordering::Relaxed);
            }
            if profile {
                eprintln!(
                    "[set-scene-cell-profile] total={:.2}ms mutation={:.2}ms run_modes={:.2}ms defaults={:.2}ms launch_observe={:.2}ms fx_resync={:.2}ms",
                    duration_ms(profile_started.elapsed()),
                    duration_ms(mutation_elapsed),
                    duration_ms(run_modes_elapsed),
                    duration_ms(defaults_elapsed),
                    duration_ms(launch_observe_elapsed),
                    duration_ms(fx_resync_started.elapsed()),
                );
            }
            editor.handle_host_event(HostEvent::Status(format!(
                "Shared track {} pattern {} into scene {}",
                track + 1,
                pattern_id,
                scene + 1
            )));
        }
        "clear-scene-cell" => {
            let Value::Map(ref map) = payload else {
                editor.handle_host_event(HostEvent::Status(
                    "Scene cell clear failed: invalid payload".to_string(),
                ));
                return;
            };
            let scene = map.get("scene").and_then(|cell| match &*cell.borrow() {
                Value::Number(n) => Some(*n as usize),
                _ => None,
            });
            let track = map.get("track").and_then(|cell| match &*cell.borrow() {
                Value::Number(n) => Some(*n as usize),
                _ => None,
            });
            let (Some(scene), Some(track)) = (scene, track) else {
                editor.handle_host_event(HostEvent::Status(
                    "Scene cell clear failed: missing scene or track".to_string(),
                ));
                return;
            };
            let num_tracks = app.tracks.len();
            let cleared = app.apply_recorded_scene_structure_mutation(
                "Clear scene cell",
                |app| app.state.clear_scene_cell(
                    scene,
                    track,
                    num_tracks,
                    &app.graph.track_buffer_ids,
                    &app.graph.track_sample_rates,
                    &app.tracks,
                    &app.graph.track_instrument_types,
                ).ok_or_else(|| format!(
                    "Could not clear scene {} track {}", scene + 1, track + 1
                )),
            );
            let Ok(pattern_id) = cleared else {
                editor.handle_host_event(HostEvent::Status(format!(
                    "Scene cell clear failed: scene {}, track {}",
                    scene + 1,
                    track + 1
                )));
                return;
            };
            editor.handle_host_event(HostEvent::Status(format!(
                "Cleared scene {} track {} pattern {}",
                scene + 1,
                track + 1,
                pattern_id.0
            )));
        }
        "launch-track-pattern" => {
            // Spec 7.3: manual track-pattern launches are rejected while song
            // playback is the launch authority.
            if let Some(message) = app.manual_launch_rejection() {
                editor.handle_host_event(HostEvent::Error(message.to_string()));
                return;
            }
            let Value::Map(ref map) = payload else {
                editor.handle_host_event(HostEvent::Status(
                    "Track pattern launch failed: invalid payload".to_string(),
                ));
                return;
            };
            let track = map.get("track").and_then(|cell| match &*cell.borrow() {
                Value::Number(n) => Some(*n as usize),
                _ => None,
            });
            let pattern_id = map
                .get("pattern-id")
                .or_else(|| map.get("pattern_id"))
                .and_then(|cell| match &*cell.borrow() {
                    Value::Number(n) if *n >= 1.0 => Some(*n as u64),
                    _ => None,
                });
            let (Some(track), Some(pattern_id)) = (track, pattern_id) else {
                editor.handle_host_event(HostEvent::Status(
                    "Track pattern launch failed: missing track or pattern id"
                        .to_string(),
                ));
                return;
            };
            let num_tracks = app.tracks.len();
            if track >= num_tracks {
                editor.handle_host_event(HostEvent::Status(format!(
                    "Track pattern launch failed: track {} is out of range",
                    track + 1
                )));
                return;
            }
            // Override launches (the clip-click path during song playback
            // authority — `set-scene-cell` delegates here with the click's
            // quantize label) follow the transport launch quantize too. The
            // deferred apply runs through the central launch seam
            // (`apply_pattern_launch_at` via the due drain), which latches
            // the lane and records the capture event at the boundary beat.
            let quantize_label = extract_string_from_payload(&payload, "quantize")
                .unwrap_or_else(|| "off".to_string());
            let Some(quantize) =
                sequencer::quantized_launch::LaunchQuantize::from_transport_label(
                    &quantize_label,
                )
            else {
                editor.handle_host_event(HostEvent::Error(format!(
                    "Unknown scene launch quantization: {quantize_label}"
                )));
                return;
            };
            if quantize != sequencer::quantized_launch::LaunchQuantize::Off {
                match state.schedule_quantized_pattern_launch(
                    sequencer::quantized_launch::PatternLaunchTarget::TrackPattern {
                        track,
                        pattern: pattern_id,
                    },
                    quantize,
                    sequencer::quantized_launch::QuantizedLaunchOwner::TrackClip(
                        track as u32,
                    ),
                ) {
                    Ok(token) => editor.handle_host_event(HostEvent::Status(format!(
                        "Queued track {} clip {} at {} (launch {})",
                        track + 1,
                        pattern_id,
                        quantize.transport_label(),
                        token
                    ))),
                    Err(error) => editor.handle_host_event(HostEvent::Error(format!(
                        "Could not queue clip launch: {error:?}"
                    ))),
                }
                return;
            }
            let launched = app.state.launch_track_pattern(
                track,
                PatternId(pattern_id),
                num_tracks,
                &app.graph.track_buffer_ids,
                &app.graph.track_sample_rates,
                &app.tracks,
                &app.graph.track_instrument_types,
            );
            if !launched {
                editor.handle_host_event(HostEvent::Status(format!(
                    "Track pattern launch failed: pattern id {} is unavailable",
                    pattern_id
                )));
                return;
            }

            let sample_ids = app.state.effective_pattern_sample_ids(num_tracks);
            app.graph_controller().apply_sample_ids(&sample_ids);
            if let Err(error) = app
                .graph_controller()
                .sync_track_instrument_run_modes_from_live_state()
            {
                app.editor.status_message = Some((
                    format!("Track pattern launch failed: {error}"),
                    Instant::now(),
                ));
            }
            app.push_all_restored_defaults();
            // Clip launches are performance gestures too: latch the lane
            // during song playback (takes spec 10 — the intentional way to
            // claim a lane, take lanes included) and record the launch into
            // an active arrangement capture.
            app.observe_manual_clip_launch(track, PatternId(pattern_id));

            let ct = current_track_for_app(&mut app, &current_track).unwrap_or(track);
            let fx_visible = editor_has_visible_buffer(&editor, "*fx*");
            let sequencer_visible = editor_has_visible_buffer(&editor, "*sequencer*");
            let selected_neural_snapshot =
                selected_neural_neurons.lock().unwrap().clone();
            let rt = editor.runtime_mut();
            sync_shared_track_collapsed(&track_collapsed, &app);
            sync_track_name_state(rt, &mut *ctx.track_names, &app);
            sync_pattern_state(rt, &state);
            set_current_track_reactive(rt, app.tracks.len(), ct);
            rt.set_reactive("SEQ", "steps", build_steps_value(&state, ct));
            sync_all_track_sequencer_state(rt, &state, &app, ct, &selected_steps);
            if sequencer_visible {
                let _ = sync_all_expanded_step_viewports(
                    rt,
                    &state,
                    &app,
                    &selected_steps,
                    ct,
                    &expanded_step_projection,
                );
            }
            sync_piano_roll_state(rt, app, &state, ct, &piano_roll_selection);
            sync_step_param_lists(rt, &state, ct);
            sync_track_mixer_state(rt, &app, &state);
            sync_track_peak_fields(rt, &ctx.meters.cached_track_peak_levels);
            if fx_visible {
                rt.set_reactive_value_patch(
                    "SEQ",
                    "effects",
                    build_effects_value(
                        &state,
                        ct,
                        &app.graph.effect_descriptors,
                        &selected_steps,
                    ),
                );
                rt.set_reactive_value_patch(
                    "SEQ",
                    "midi-effects",
                    build_midi_effects_value(&state, ct, &selected_steps),
                );
                rt.set_reactive_value_patch(
                    "SEQ",
                    "instrument-panel",
                    build_instrument_panel_value(&app, ct, &selected_steps),
                );
                *accumulator_names.lock().unwrap() = build_accumulator_names(&app);
            } else {
                fx_value_epoch.fetch_add(1, Ordering::Relaxed);
            }
            sync_track_params_with_neural_selection(
                rt,
                &app,
                &state,
                ct,
                &selected_steps,
                Some(&selected_neural_snapshot),
            );
            sync_fx_param_binding_fields_with_neural_selection(
                rt,
                &app,
                &state,
                ct,
                &selected_steps,
                Some(&selected_neural_snapshot),
            );
            rt.set_reactive(
                "SEQ",
                "step-has-plocks",
                build_step_has_plocks(&state, ct, &app.graph.effect_descriptors),
            );
            sync_sidebar_browser(rt, &app, ct);
            rt.run_reactive_cycle();
            editor.refresh_runtime_side_effects();
            if editor_has_visible_mixer_buffer(editor) {
                refresh_visible_mixer_layouts(editor);
            }
            ctx.frame.prev_pattern_epoch = state.transport.pattern_epoch.load(Ordering::Relaxed);
            ctx.frame.prev_track_button_states = track_button_state_snapshot(&state);
            ctx.frame.prev_track_playheads = track_playheads_snapshot(&state, &app);
            ui_epoch.fetch_add(1, Ordering::Relaxed);
            editor.handle_host_event(HostEvent::Status(format!(
                "Launched track {} pattern {}",
                track + 1,
                pattern_id
            )));
        }
        "switch-pattern" => {
            let profile_switch = pattern_switch_profile_enabled();
            let profile_total_started = Instant::now();
            if let Value::Map(ref map) = payload {
                let idx = map.get("idx").and_then(|cell| match &*cell.borrow() {
                    Value::Number(n) => Some(*n as usize),
                    _ => None,
                });
                if let Some(idx) = idx {
                    // Spec 7.3: the song is the only launch authority during
                    // song playback.
                    if let Some(message) = app.manual_launch_rejection() {
                        editor.handle_host_event(HostEvent::Error(message.to_string()));
                        return;
                    }
                    let quantize_label =
                        extract_string_from_payload(&payload, "quantize")
                            .unwrap_or_else(|| "off".to_string());
                    let Some(quantize) = sequencer::quantized_launch::LaunchQuantize::from_transport_label(&quantize_label) else {
                        editor.handle_host_event(HostEvent::Error(format!(
                            "Unknown scene launch quantization: {quantize_label}"
                        )));
                        return;
                    };
                    if quantize != sequencer::quantized_launch::LaunchQuantize::Off {
                        match state.schedule_quantized_pattern_launch(
                            sequencer::quantized_launch::PatternLaunchTarget::Scene {
                                scene: idx,
                            },
                            quantize,
                            sequencer::quantized_launch::QuantizedLaunchOwner::Transport,
                        ) {
                            Ok(token) => editor.handle_host_event(HostEvent::Status(
                                format!(
                                    "Queued scene {} at {} (launch {})",
                                    idx + 1,
                                    quantize.transport_label(),
                                    token
                                ),
                            )),
                            Err(error) => editor.handle_host_event(HostEvent::Error(
                                format!("Could not queue scene launch: {error:?}"),
                            )),
                        }
                        return;
                    }
                    let switch_bus_elapsed = Duration::ZERO;
                    let state_switch_elapsed;
                    let apply_samples_elapsed = Duration::ZERO;
                    let restored_defaults_elapsed = Duration::ZERO;
                    let mut sync_names_pattern_elapsed = Duration::ZERO;
                    let mut sync_current_steps_elapsed = Duration::ZERO;
                    let mut sync_sequencer_elapsed = Duration::ZERO;
                    let mut sync_expanded_elapsed = Duration::ZERO;
                    let mut sync_piano_elapsed = Duration::ZERO;
                    let mut sync_step_params_elapsed = Duration::ZERO;
                    let mut sync_mixer_elapsed = Duration::ZERO;
                    let mut sync_fx_lists_elapsed = Duration::ZERO;
                    let mut sync_effects_elapsed = Duration::ZERO;
                    let mut sync_midi_effects_elapsed = Duration::ZERO;
                    let mut sync_instrument_panel_elapsed = Duration::ZERO;
                    let mut sync_accumulators_elapsed = Duration::ZERO;
                    let mut sync_track_params_elapsed = Duration::ZERO;
                    let mut sync_fx_bindings_elapsed = Duration::ZERO;
                    let mut sync_plocks_sidebar_elapsed = Duration::ZERO;
                    let mut reactive_elapsed = Duration::ZERO;
                    let mut side_effects_elapsed = Duration::ZERO;
                    let started = Instant::now();
                    let switched = app.apply_manual_pattern_launch(
                        &sequencer::quantized_launch::PatternLaunchTarget::Scene {
                            scene: idx,
                        },
                    );
                    state_switch_elapsed = started.elapsed();
                    let pattern_changed = switched.is_ok();
                    if switched.is_ok() {
                        let ct = current_track.load(Ordering::Relaxed);
                        let fx_visible = editor_has_visible_buffer(&editor, "*fx*");
                        let sequencer_visible =
                            editor_has_visible_buffer(&editor, "*sequencer*");
                        let rt = editor.runtime_mut();
                        let started = Instant::now();
                        sync_shared_track_collapsed(&track_collapsed, &app);
                        sync_track_name_state(rt, &mut *ctx.track_names, &app);
                        sync_pattern_state(rt, &state);
                        sync_names_pattern_elapsed = started.elapsed();
                        let started = Instant::now();
                        rt.set_reactive("SEQ", "steps", build_steps_value(&state, ct));
                        sync_current_steps_elapsed = started.elapsed();
                        let started = Instant::now();
                        sync_all_track_sequencer_state(
                            rt,
                            &state,
                            &app,
                            ct,
                            &selected_steps,
                        );
                        sync_sequencer_elapsed = started.elapsed();
                        let started = Instant::now();
                        if sequencer_visible {
                            let _ = sync_all_expanded_step_viewports(
                                rt,
                                &state,
                                &app,
                                &selected_steps,
                                ct,
                                &expanded_step_projection,
                            );
                        }
                        sync_expanded_elapsed = started.elapsed();
                        let started = Instant::now();
                        sync_piano_roll_state(rt, app, &state, ct, &piano_roll_selection);
                        sync_piano_elapsed = started.elapsed();
                        let started = Instant::now();
                        sync_step_param_lists(rt, &state, ct);
                        sync_step_params_elapsed = started.elapsed();
                        let started = Instant::now();
                        sync_track_mixer_state(rt, &app, &state);
                        sync_bus_mixer_state(rt, &app);
                        sync_track_peak_fields(rt, &ctx.meters.cached_track_peak_levels);
                        sync_bus_peak_fields(rt, &ctx.meters.cached_bus_peak_levels);
                        sync_mixer_elapsed = started.elapsed();
                        let started = Instant::now();
                        if fx_visible {
                            let sub_started = Instant::now();
                            rt.set_reactive_value_patch(
                                "SEQ",
                                "effects",
                                build_effects_value(
                                    &state,
                                    ct,
                                    &app.graph.effect_descriptors,
                                    &selected_steps,
                                ),
                            );
                            sync_effects_elapsed = sub_started.elapsed();

                            let sub_started = Instant::now();
                            rt.set_reactive_value_patch(
                                "SEQ",
                                "midi-effects",
                                build_midi_effects_value(&state, ct, &selected_steps),
                            );
                            sync_midi_effects_elapsed = sub_started.elapsed();

                            let sub_started = Instant::now();
                            rt.set_reactive_value_patch(
                                "SEQ",
                                "instrument-panel",
                                build_instrument_panel_value(&app, ct, &selected_steps),
                            );
                            sync_instrument_panel_elapsed = sub_started.elapsed();

                            let sub_started = Instant::now();
                            *accumulator_names.lock().unwrap() =
                                build_accumulator_names(&app);
                            sync_accumulators_elapsed = sub_started.elapsed();
                        } else {
                            fx_value_epoch.fetch_add(1, Ordering::Relaxed);
                        }
                        sync_fx_lists_elapsed = started.elapsed();
                        let started = Instant::now();
                        let selected_neural_snapshot =
                            selected_neural_neurons.lock().unwrap().clone();
                        sync_track_params_with_neural_selection(
                            rt,
                            &app,
                            &state,
                            ct,
                            &selected_steps,
                            Some(&selected_neural_snapshot),
                        );
                        sync_track_params_elapsed = started.elapsed();
                        let started = Instant::now();
                        sync_fx_param_binding_fields_with_neural_selection(
                            rt,
                            &app,
                            &state,
                            ct,
                            &selected_steps,
                            Some(&selected_neural_snapshot),
                        );
                        sync_fx_bindings_elapsed = started.elapsed();
                        let started = Instant::now();
                        rt.set_reactive(
                            "SEQ",
                            "step-has-plocks",
                            build_step_has_plocks(
                                &state,
                                ct,
                                &app.graph.effect_descriptors,
                            ),
                        );
                        sync_sidebar_browser(rt, &app, ct);
                        sync_plocks_sidebar_elapsed = started.elapsed();
                        let started = Instant::now();
                        rt.run_reactive_cycle();
                        reactive_elapsed = started.elapsed();
                        let started = Instant::now();
                        editor.refresh_runtime_side_effects();
                        side_effects_elapsed = started.elapsed();
                        if editor_has_visible_mixer_buffer(editor) {
                            refresh_visible_mixer_layouts(editor);
                        }
                        ctx.frame.prev_pattern_epoch =
                            state.transport.pattern_epoch.load(Ordering::Relaxed);
                        ctx.frame.prev_track_button_states = track_button_state_snapshot(&state);
                        ctx.frame.prev_track_playheads = track_playheads_snapshot(&state, &app);
                    }
                    if profile_switch {
                        eprintln!(
                            "[pattern-switch-profile][host] idx={} changed={} total={:.2}ms switch_bus={:.2}ms state_switch={:.2}ms apply_samples={:.2}ms defaults={:.2}ms names_pattern={:.2}ms current_steps={:.2}ms sequencer_bindings={:.2}ms expanded_step_viewports={:.2}ms piano={:.2}ms step_params={:.2}ms mixer={:.2}ms fx_lists={:.2}ms effects={:.2}ms midi_effects={:.2}ms instrument_panel={:.2}ms accumulators={:.2}ms track_params={:.2}ms fx_bindings={:.2}ms plocks_sidebar={:.2}ms reactive={:.2}ms side_effects={:.2}ms",
                            idx,
                            pattern_changed,
                            duration_ms(profile_total_started.elapsed()),
                            duration_ms(switch_bus_elapsed),
                            duration_ms(state_switch_elapsed),
                            duration_ms(apply_samples_elapsed),
                            duration_ms(restored_defaults_elapsed),
                            duration_ms(sync_names_pattern_elapsed),
                            duration_ms(sync_current_steps_elapsed),
                            duration_ms(sync_sequencer_elapsed),
                            duration_ms(sync_expanded_elapsed),
                            duration_ms(sync_piano_elapsed),
                            duration_ms(sync_step_params_elapsed),
                            duration_ms(sync_mixer_elapsed),
                            duration_ms(sync_fx_lists_elapsed),
                            duration_ms(sync_effects_elapsed),
                            duration_ms(sync_midi_effects_elapsed),
                            duration_ms(sync_instrument_panel_elapsed),
                            duration_ms(sync_accumulators_elapsed),
                            duration_ms(sync_track_params_elapsed),
                            duration_ms(sync_fx_bindings_elapsed),
                            duration_ms(sync_plocks_sidebar_elapsed),
                            duration_ms(reactive_elapsed),
                            duration_ms(side_effects_elapsed),
                        );
                    }
                }
            }
        }
        "rename-scene" => {
            let Value::Map(ref map) = payload else {
                editor.handle_host_event(HostEvent::Status(
                    "Could not rename scene: invalid payload".to_string(),
                ));
                return;
            };
            let scene = map.get("scene").and_then(|cell| match &*cell.borrow() {
                Value::Number(n) if *n >= 0.0 => Some(*n as usize),
                _ => None,
            });
            let name = map.get("name").and_then(|cell| match &*cell.borrow() {
                Value::String(name) => Some(name.clone()),
                _ => None,
            });
            let renamed = match (scene, name) {
                (Some(scene), Some(name)) => app.apply_recorded_scene_structure_mutation(
                    "Rename scene",
                    |app| app.state.rename_scene(scene, name)
                        .then_some(())
                        .ok_or_else(|| "Scene name or index is invalid".to_string()),
                ),
                _ => Err("Scene or name is missing".to_string()),
            };
            match renamed {
                Ok(()) => {
                    let rt = editor.runtime_mut();
                    sync_pattern_state(rt, &state);
                    rt.run_reactive_cycle();
                    editor.refresh_runtime_side_effects();
                    ui_epoch.fetch_add(1, Ordering::Relaxed);
                }
                Err(error) => editor.handle_host_event(HostEvent::Status(format!(
                    "Could not rename scene: {error}"
                ))),
            }
        }
        "reorder-scene" => {
            let source = extract_usize_from_payload(&payload, "source");
            let target = extract_usize_from_payload(&payload, "target");
            match (source, target) {
                (Some(source), Some(target)) => {
                    let reordered = app.apply_recorded_scene_structure_mutation(
                        "Reorder scene",
                        |app| app.state.reorder_scene(source, target)
                            .ok_or_else(|| "Scene index is out of range".to_string()),
                    );
                    if reordered.is_ok() {
                        let rt = editor.runtime_mut();
                        sync_pattern_state(rt, &state);
                        rt.run_reactive_cycle();
                        editor.refresh_runtime_side_effects();
                        ui_epoch.fetch_add(1, Ordering::Relaxed);
                        editor.handle_host_event(HostEvent::Status(format!(
                            "Moved scene {} to {}",
                            source + 1,
                            target + 1
                        )));
                    } else {
                        editor.handle_host_event(HostEvent::Status(
                            "Could not reorder scenes: scene index out of range"
                                .to_string(),
                        ));
                    }
                }
                _ => editor.handle_host_event(HostEvent::Status(
                    "Could not reorder scenes: invalid drag payload".to_string(),
                )),
            }
        }
        "propagate-current-track-to-all-patterns" => {
            let track = match payload {
                Value::Number(n) => n as usize,
                _ => current_track.load(Ordering::Relaxed),
            };
            let num_patterns = state.scene_count();
            if track >= app.tracks.len() {
                editor.handle_host_event(HostEvent::Status(format!(
                    "Track {} is out of range",
                    track + 1
                )));
            } else if num_patterns <= 1 {
                editor.handle_host_event(HostEvent::Status(
                    "Nothing to propagate: only one pattern exists".to_string(),
                ));
            } else if app.apply_recorded_scene_structure_mutation(
                "Propagate track pattern",
                |app| app.state.propagate_track_to_all_patterns(
                    track,
                    app.tracks.len(),
                    &app.graph.track_buffer_ids,
                    &app.graph.track_sample_rates,
                    &app.tracks,
                    &app.graph.track_instrument_types,
                ).then_some(()).ok_or_else(|| format!(
                    "Could not propagate track {}", track + 1
                )),
            ).is_ok() {
                editor.handle_host_event(HostEvent::Status(format!(
                    "Propagated track {} to {} patterns",
                    track + 1,
                    num_patterns
                )));
            } else {
                editor.handle_host_event(HostEvent::Status(format!(
                    "Failed to propagate track {}",
                    track + 1
                )));
            }
        }
        "clone-pattern" => {
            let num_tracks = app.tracks.len();
            let created = app.apply_recorded_scene_structure_mutation(
                "Create scene",
                |app| {
                    let source_pattern = app.state.current_scene_index();
                    let new_idx = app.state.clone_pattern(
                        num_tracks,
                        &app.graph.track_buffer_ids,
                        &app.graph.track_sample_rates,
                        &app.tracks,
                        &app.graph.track_instrument_types,
                    );
                    app.graph_controller().sync_current_pattern_mod_routes();
                    app.clone_bus_pattern_from_to(source_pattern, new_idx);
                    Ok(new_idx)
                },
            );
            let Ok(new_idx) = created else {
                editor.handle_host_event(HostEvent::Status(
                    "Could not create scene".to_string(),
                ));
                return;
            };
            let rt = editor.runtime_mut();
            sync_pattern_state(rt, &state);
            sync_bus_mixer_state(rt, &app);
            rt.run_reactive_cycle();
            editor.refresh_runtime_side_effects();
            ui_epoch.fetch_add(1, Ordering::Relaxed);
            editor.handle_host_event(HostEvent::Status(format!(
                "Cloned pattern {}",
                new_idx + 1
            )));
        }
        "delete-pattern" => {
            let num_tracks = app.tracks.len();
            let deleted_pattern = app.state.current_scene_index();
            let deleted = app.apply_recorded_scene_structure_mutation(
                "Delete scene",
                |app| {
                    let sample_ids = app.state.delete_pattern(
                        num_tracks,
                        &app.graph.track_buffer_ids,
                        &app.graph.track_sample_rates,
                        &app.tracks,
                        &app.graph.track_instrument_types,
                    )?;
                    app.handle_scene_deleted(deleted_pattern);
                    app.graph_controller().apply_sample_ids(&sample_ids);
                    app.graph_controller().sync_current_pattern_mod_routes();
                    app.push_all_restored_defaults();
                    let new_pattern = app.state.current_scene_index();
                    app.delete_bus_pattern_at(deleted_pattern, new_pattern);
                    Ok(())
                },
            );
            if let Err(error) = &deleted {
                editor.handle_host_event(HostEvent::Status(format!(
                    "Scene delete failed: {error}"
                )));
            }
            if deleted.is_ok() {
                let ct = current_track.load(Ordering::Relaxed);
                let rt = editor.runtime_mut();
                sync_shared_track_collapsed(&track_collapsed, &app);
                sync_track_name_state(rt, &mut *ctx.track_names, &app);
                sync_pattern_state(rt, &state);
                rt.set_reactive("SEQ", "steps", build_steps_value(&state, ct));
                sync_step_param_lists(rt, &state, ct);
                sync_track_mixer_state(rt, &app, &state);
                sync_bus_mixer_state(rt, &app);
                sync_track_peak_fields(rt, &ctx.meters.cached_track_peak_levels);
                sync_bus_peak_fields(rt, &ctx.meters.cached_bus_peak_levels);
                rt.set_reactive_value_patch(
                    "SEQ",
                    "effects",
                    build_effects_value(
                        &state,
                        ct,
                        &app.graph.effect_descriptors,
                        &selected_steps,
                    ),
                );
                rt.set_reactive_value_patch(
                    "SEQ",
                    "midi-effects",
                    build_midi_effects_value(&state, ct, &selected_steps),
                );
                rt.set_reactive_value_patch(
                    "SEQ",
                    "instrument-panel",
                    build_instrument_panel_value(&app, ct, &selected_steps),
                );
                *accumulator_names.lock().unwrap() = build_accumulator_names(&app);
                let selected_neural_snapshot =
                    selected_neural_neurons.lock().unwrap().clone();
                sync_track_params_with_neural_selection(
                    rt,
                    &app,
                    &state,
                    ct,
                    &selected_steps,
                    Some(&selected_neural_snapshot),
                );
                sync_fx_param_binding_fields_with_neural_selection(
                    rt,
                    &app,
                    &state,
                    ct,
                    &selected_steps,
                    Some(&selected_neural_snapshot),
                );
                rt.set_reactive(
                    "SEQ",
                    "step-has-plocks",
                    build_step_has_plocks(&state, ct, &app.graph.effect_descriptors),
                );
                sync_sidebar_browser(rt, &app, ct);
                rt.run_reactive_cycle();
                editor.refresh_runtime_side_effects();
                ui_epoch.fetch_add(1, Ordering::Relaxed);
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::collections::{BTreeSet, HashSet};
    use std::rc::Rc;
    use std::sync::atomic::{AtomicBool, AtomicI32, AtomicUsize};
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};

    fn scene_cell_payload(scene: f64, track: f64, pattern_id: f64, quantize: &str) -> Value {
        Value::Map(
            [
                ("scene".to_string(), Value::Number(scene)),
                ("track".to_string(), Value::Number(track)),
                ("pattern-id".to_string(), Value::Number(pattern_id)),
                ("quantize".to_string(), Value::String(quantize.to_string())),
            ]
            .into_iter()
            .map(|(key, value)| (key, Rc::new(RefCell::new(value))))
            .collect(),
        )
    }

    /// Drives the REAL `dispatch_custom_host_command` -> `scenes::handle`
    /// seam for a mixer clip click (`set-scene-cell` into the current scene)
    /// with the transport launch quantize set: the cell assignment must land
    /// as an edit, the audible restore must NOT happen, and a `SceneTracks`
    /// launch must be pending under the per-track `TrackClip` owner. With
    /// quantize off the legacy immediate restore must be untouched.
    #[test]
    fn quantized_clip_click_defers_the_audible_launch_and_queues_scene_tracks() {
        const TRACK: usize = 0;

        let state = Arc::new(sequencer::sequencer::SequencerState::new(
            1,
            vec![sequencer::sequencer::default_empty_effect_chain()],
        ));
        // Scene 0: empty pattern (current). Scene 1: pattern with step 4 set.
        let first = sequencer::sequencer::PatternSnapshot::new_default(1, &[]);
        let mut second = sequencer::sequencer::PatternSnapshot::new_default(1, &[]);
        second.track_bits[TRACK][0] |= 1u64 << 4;
        state.replace_pattern_repository(vec![first, second], 0);
        state.restore_current_pattern_from_repository().unwrap();
        let target_id = state.scene_track_pattern_id(1, TRACK).unwrap();
        assert!(!state.pattern.patterns[TRACK].is_active(4));

        let (keyboard_tx, _keyboard_rx) = std::sync::mpsc::channel();
        let mut app = app::App::new(
            state.clone(),
            sequencer::audiograph::LiveGraphPtr(std::ptr::null_mut()),
            44_100,
            app::AudioBuses {
                bus_l_id: 0,
                bus_r_id: 0,
                default_bus_nodes: Vec::new(),
                bus_gate_runtime: Arc::new(Mutex::new(Arc::new(Vec::new()))),
                bus_gate_playheads: Arc::new(Mutex::new(Vec::new())),
                reverb_bus_id: 0,
                reverb_node_id: 0,
            },
            Arc::new(sequencer::recorder::MasterRecorder::new(44_100, 2)),
            keyboard_tx.clone(),
        );
        app.tracks = vec!["Track 1".to_string()];
        app.track_registry =
            sequencer::sequencer::TrackRegistry::for_legacy_track_count(1).unwrap();

        let mut runtime = Runtime::new();
        runtime.register_reactive("SEQ", Vec::new(), true);
        let mut editor = Editor::new(runtime, eseqlisp::EditorConfig::default());

        let current_track = Arc::new(AtomicUsize::new(TRACK));
        let sample_db = Rc::new(
            sequencer::sample_db::SampleDb::open_in_memory().expect("open in-memory sample db"),
        );
        let shared = SharedHandles {
            state: state.clone(),
            lg_raw: std::ptr::null_mut(),
            current_track: current_track.clone(),
            selected_tracks: Arc::new(Mutex::new(HashSet::new())),
            selected_steps: Arc::new(Mutex::new(HashSet::new())),
            selected_neural_neurons: Arc::new(Mutex::new(BTreeSet::new())),
            piano_roll_selection: Arc::new(Mutex::new(HashSet::new())),
            piano_roll_move_state: Arc::new(Mutex::new(None)),
            piano_roll_focus: super::super::super::new_shared_piano_roll_focus(),
            step_clipboard: Arc::new(Mutex::new(None)),
            ui_epoch: Arc::new(AtomicUsize::new(0)),
            fx_epoch: Arc::new(AtomicUsize::new(0)),
            fx_value_epoch: Arc::new(AtomicUsize::new(0)),
            ui_invalidations: Arc::new(UiInvalidationQueue::new()),
            expanded_step_projection: Arc::new(ExpandedStepProjectionRegistry::new()),
            active_delete_target: Arc::new(Mutex::new(None)),
            active_delete_target_version: Arc::new(AtomicUsize::new(0)),
            auto_follow_override_until: Arc::new(Mutex::new(None)),
            track_pan_ids: Arc::new(Mutex::new(Vec::new())),
            track_collapsed: Arc::new(Mutex::new(app.track_collapsed.clone())),
            bus_state: Arc::new(Mutex::new(app.buses.clone())),
            bus_node_ids: Arc::new(Mutex::new(app.graph.bus_node_ids.clone())),
            track_groups: Arc::new(Mutex::new(app.groups.clone())),
            record_armed: Arc::new(Mutex::new(vec![false])),
            armed_rack: Arc::new(Mutex::new(None)),
            recording: Arc::new(AtomicBool::new(false)),
            master_recording: Arc::new(AtomicBool::new(false)),
            held_notes: Arc::new(Mutex::new(Vec::new())),
            roll_record: Arc::new(Mutex::new(RollRecordBuffer::default())),
                step_print: Arc::new(Mutex::new(StepPrintState::default())),
            keyboard_octave: Arc::new(AtomicI32::new(0)),
            sample_browser: Rc::new(RefCell::new(DebouncedSampleBrowser::new(
                sample_db,
                Duration::from_millis(100),
            ))),
            keyboard_tx,
            accumulator_names: Arc::new(Mutex::new(Vec::new())),
            piano_roll_clipboard: super::super::super::new_piano_roll_clipboard(),
            arrangement_clipboard: app::song_region::new_arrangement_clipboard(),
            selected_drum_lane_steps: Arc::new(Mutex::new(HashSet::new())),
        };
        let mut sessions = EditSessionState::default();
        let mut frame = FrameDiffState::default();
        let mut gesture = GestureState::default();
        let mut meters = MeterCache {
            cached_peak_l_level: 0.0,
            cached_peak_r_level: 0.0,
            cached_track_peak_levels: vec![0.0],
            cached_rack_slot_peak_levels: Vec::new(),
            cached_bus_peak_levels: Vec::new(),
            cached_modulator_phases: Vec::new(),
            cached_modulator_levels: Vec::new(),
            cached_cpu_load_bits: 0.0f32.to_bits(),
            last_meter_poll_at: Instant::now(),
            last_cpu_ui_poll_at: Instant::now(),
            last_neural_visualization_poll_at: Instant::now(),
            visualization_liveness: VisualizationLiveness::default(),
            last_voice_count_log_at: Instant::now(),
        };
        let mut track_names = vec!["Track 1".to_string()];
        let mut ctx = LoopCtx {
            sessions: &mut sessions,
            meters: &mut meters,
            frame: &mut frame,
            gesture: &mut gesture,
            track_names: &mut track_names,
            shared: &shared,
        };

        dispatch_custom_host_command(
            "set-scene-cell",
            scene_cell_payload(0.0, TRACK as f64, target_id.0 as f64, "1 bar"),
            &mut app,
            &mut editor,
            &mut ctx,
        );

        // The edit landed but nothing sounded: cell assigned, live grid
        // still empty, and the launch is pending under the TrackClip owner.
        assert_eq!(state.scene_track_pattern_id(0, TRACK), Some(target_id));
        assert!(
            !state.pattern.patterns[TRACK].is_active(4),
            "a quantized clip click must not restore the pattern immediately"
        );
        assert_eq!(
            state.quantized_launches().pending_target(
                sequencer::quantized_launch::QuantizedLaunchOwner::TrackClip(TRACK as u32)
            ),
            Some(
                sequencer::quantized_launch::PatternLaunchTarget::SceneTracks {
                    scene: 0,
                    tracks: vec![TRACK],
                }
            ),
            "the clip click must queue a SceneTracks launch under its track's owner"
        );

        // Quantize off: the legacy immediate restore.
        dispatch_custom_host_command(
            "set-scene-cell",
            scene_cell_payload(0.0, TRACK as f64, target_id.0 as f64, "off"),
            &mut app,
            &mut editor,
            &mut ctx,
        );
        assert!(
            state.pattern.patterns[TRACK].is_active(4),
            "an unquantized clip click must restore the pattern immediately"
        );

        // The override-launch path (`launch-track-pattern` — where
        // `set-scene-cell` delegates during song playback authority) must
        // quantize through the same seam: with quantize set the pool
        // pattern is NOT launched immediately and a TrackPattern launch is
        // pending; with quantize off it launches immediately.
        let original_id = {
            let cells = state.track_pattern_cells(TRACK);
            cells
                .iter()
                .map(|cell| cell.pattern_id)
                .find(|id| *id != target_id)
                .expect("the original scene-0 pattern stays in the pool")
        };
        dispatch_custom_host_command(
            "launch-track-pattern",
            scene_cell_payload(0.0, TRACK as f64, original_id.0 as f64, "1 bar"),
            &mut app,
            &mut editor,
            &mut ctx,
        );
        assert!(
            state.pattern.patterns[TRACK].is_active(4),
            "a quantized override launch must not swap the live pattern immediately"
        );
        assert_eq!(
            state.quantized_launches().pending_target(
                sequencer::quantized_launch::QuantizedLaunchOwner::TrackClip(TRACK as u32)
            ),
            Some(
                sequencer::quantized_launch::PatternLaunchTarget::TrackPattern {
                    track: TRACK,
                    pattern: original_id.0,
                }
            ),
            "the override launch must queue a TrackPattern target under its track's owner"
        );
        dispatch_custom_host_command(
            "launch-track-pattern",
            scene_cell_payload(0.0, TRACK as f64, original_id.0 as f64, "off"),
            &mut app,
            &mut editor,
            &mut ctx,
        );
        assert!(
            !state.pattern.patterns[TRACK].is_active(4),
            "an unquantized override launch must swap the live pattern immediately"
        );
    }
}
