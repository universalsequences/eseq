use crate::*;

pub(super) const COMMANDS: &[&str] = &[
    "set-convolution-reverb-ir",
    "set-filter-table-source",
    "set-filter-table-mode",
    "set-filter-table-engine",
    "filter-table-editor-open",
    "filter-table-editor-close",
    "filter-table-editor-op",
    "filter-table-editor-band",
    "filter-table-editor-add-node",
    "filter-table-editor-undo",
    "filter-table-editor-redo",
    "filter-table-editor-frame",
    "filter-table-editor-save",
    "set-effect-param-batch",
    "set-effect-plock-batch",
    "set-effect-param",
    "toggle-effect-param",
    "set-effect-param-option",
    "set-effect-plock-option",
    "set-midi-fx-param",
    "set-midi-fx-plock",
    "set-midi-fx-param-option",
    "set-midi-fx-plock-option",
    "add-effect",
    "add-effect-to-track",
    "add-builtin-effect",
    "add-builtin-effect-to-track",
    "add-midi-fx",
    "add-midi-fx-to-track",
    "insert-builtin-effect-before-slot",
    "insert-effect-before-slot",
    "insert-midi-fx-before-slot",
    "move-effect-slot",
    "copy-effect-values-to-all-scenes",
    "move-midi-fx-slot",
    "copy-selected-effect",
    "paste-effect",
    "delete-effect",
    "delete-midi-fx",
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
    let current_track = ctx.shared.current_track.clone();
    let selected_steps = ctx.shared.selected_steps.clone();
    let selected_neural_neurons = ctx.shared.selected_neural_neurons.clone();
    let ui_epoch = ctx.shared.ui_epoch.clone();
    let fx_epoch = ctx.shared.fx_epoch.clone();
    let ui_invalidations = ctx.shared.ui_invalidations.clone();
    let bus_state = ctx.shared.bus_state.clone();
    match name {
        "set-convolution-reverb-ir" => {
            let path_str = extract_path_from_payload(&payload);
            // bus >= 0 means a bus effect; absent/-1 means a track effect.
            let bus = extract_usize_from_payload(&payload, "bus");
            let track = extract_usize_from_payload(&payload, "track");
            let slot = extract_usize_from_payload(&payload, "slot");
            match (slot, path_str) {
                (Some(slot), Some(path_str)) => {
                    let path = Path::new(&path_str);
                    let reference = path
                        .file_stem()
                        .and_then(|s| s.to_str())
                        .unwrap_or(path_str.as_str())
                        .to_string();
                    let result = if let Some(bus_idx) = bus {
                        app.apply_recorded_bus_effect_value_mutation(
                            bus_idx,
                            slot,
                            "Set bus convolution IR",
                            "convolution-ir",
                            |app| app.set_conv_reverb_ir_bus(
                                bus_idx,
                                slot,
                                path,
                                &reference,
                            ),
                        )
                    } else if let Some(track) = track {
                        app::edit::apply_recorded_track_effect_ir_mutation(
                            &mut app,
                            track,
                            slot,
                            path,
                            &reference,
                        )
                        .map(|_| ())
                        .map_err(|error| format!("{error:?}"))
                    } else {
                        Err("need a track or bus".to_string())
                    };
                    match result {
                        Ok(()) => {
                            // Refresh the relevant effects view so the label updates.
                            let rt = editor.runtime_mut();
                            if bus.is_some() {
                                rt.set_reactive(
                                    "SEQ",
                                    "bus-effects",
                                    build_bus_effects_value_for_selection(
                                        &app,
                                        Some(&selected_steps),
                                    ),
                                );
                            } else if let Some(track) = track {
                                rt.set_reactive(
                                    "SEQ",
                                    "effects",
                                    build_effects_value(
                                        &state,
                                        track,
                                        &app.graph.effect_descriptors,
                                        &selected_steps,
                                    ),
                                );
                            }
                            editor.handle_host_event(HostEvent::Status(format!(
                                "Loaded IR: {reference}"
                            )));
                        }
                        Err(e) => editor.handle_host_event(HostEvent::Status(format!(
                            "Error loading IR: {e}"
                        ))),
                    }
                }
                _ => editor.handle_host_event(HostEvent::Status(
                    "set-convolution-reverb-ir: need slot, path".to_string(),
                )),
            }
        }
        "set-filter-table-source" => {
            // Preset dropdown sends a bare `fltab:<stem>` reference; resolve
            // it to the asset file before the shared path-based flow below.
            let path_str = extract_path_from_payload(&payload);
            let asset_stem = path_str.as_deref().and_then(|path_str| {
                sequencer::effects::filter_table_asset::decode_asset_ref(path_str)
                    .map(str::to_string)
            });
            let path_str = match asset_stem {
                Some(stem) => {
                    let resolved =
                        sequencer::effects::filter_table_asset::resolve_asset_path(&stem);
                    if resolved.is_none() {
                        editor.handle_host_event(HostEvent::Status(format!(
                            "Filter Table preset not found: {stem}"
                        )));
                    }
                    resolved.map(|path| path.to_string_lossy().into_owned())
                }
                None => path_str,
            };
            let bus = extract_usize_from_payload(&payload, "bus");
            let track = extract_usize_from_payload(&payload, "track");
            let rack_slot = extract_usize_from_payload(&payload, "rack-slot");
            let slot = extract_usize_from_payload(&payload, "slot");
            // Optional explicit analysis mode; otherwise the recommendation
            // decides and the stored reference records the chosen mode.
            let requested_mode = extract_string_from_payload(&payload, "mode")
                .and_then(|tag| sequencer::effects::filter_table::AnalysisMode::from_tag(&tag));
            match (slot, path_str) {
                (Some(slot), Some(path_str)) => {
                    let path = Path::new(&path_str);
                    let stem = path.file_stem()
                        .and_then(|stem| stem.to_str())
                        .unwrap_or(path_str.as_str())
                        .to_string();
                    let reference = match requested_mode {
                        Some(mode) => sequencer::effects::filter_table::encode_table_ref(&stem, mode),
                        None => stem,
                    };
                    let result = if let Some(bus_idx) = bus {
                        app.apply_recorded_bus_effect_value_mutation(
                            bus_idx,
                            slot,
                            "Set bus Filter Table",
                            "filter-table-source",
                            |app| app.set_filter_table_source_bus(
                                bus_idx,
                                slot,
                                path,
                                &reference,
                            ),
                        )
                    } else if let (Some(track), Some(rack_slot)) = (track, rack_slot) {
                        app::edit::apply_recorded_rack_filter_table_mutation(
                            &mut app,
                            track,
                            rack_slot,
                            slot,
                            path,
                            &reference,
                        )
                        .map(|_| ())
                        .map_err(|error| format!("{error:?}"))
                    } else if let Some(track) = track {
                        app::edit::apply_recorded_track_filter_table_mutation(
                            &mut app,
                            track,
                            slot,
                            path,
                            &reference,
                        )
                        .map(|_| ())
                        .map_err(|error| format!("{error:?}"))
                    } else {
                        Err("need a track or bus".to_string())
                    };
                    match result {
                        Ok(()) => {
                            if let (Some(track), Some(_)) = (track, rack_slot) {
                                refresh_instrument_panel_reactive(
                                    &mut editor,
                                    &app,
                                    track,
                                    &selected_steps,
                                    &ui_epoch,
                                );
                            } else {
                                queue_effect_panel_tree_invalidation(
                                    &ui_invalidations,
                                    track,
                                    bus,
                                );
                            }
                            let (sample_ref, mode) =
                                sequencer::effects::filter_table::decode_table_ref(&reference);
                            let mode_label = mode
                                .map(|mode| format!(" ({})", mode.label()))
                                .unwrap_or_default();
                            editor.handle_host_event(HostEvent::Status(format!(
                                "Loaded Filter Table: {sample_ref}{mode_label}"
                            )));
                        }
                        Err(error) => editor.handle_host_event(HostEvent::Status(format!(
                            "Error loading Filter Table: {error}"
                        ))),
                    }
                }
                _ => editor.handle_host_event(HostEvent::Status(
                    "set-filter-table-source: need slot, path".to_string(),
                )),
            }
        }
        "set-filter-table-mode" => {
            // Re-analyze the current source under a different mode. "next"
            // cycles; any AnalysisMode tag selects that mode directly.
            let bus = extract_usize_from_payload(&payload, "bus");
            let track = extract_usize_from_payload(&payload, "track");
            let slot = extract_usize_from_payload(&payload, "slot");
            let mode_str = extract_string_from_payload(&payload, "mode");
            let result = (|| -> Result<String, String> {
                let slot = slot.ok_or_else(|| "need a slot".to_string())?;
                let (sample_name, current_mode) = app
                    .filter_table_source_info(track, bus, slot)
                    .ok_or_else(|| "load an audio sample first".to_string())?;
                if sequencer::effects::filter_table_asset::decode_asset_ref(&sample_name).is_some()
                {
                    return Err(
                        "this table is a baked asset; analysis modes apply to audio sources"
                            .to_string(),
                    );
                }
                let mode = match mode_str.as_deref() {
                    Some("next") | None => current_mode
                        .map(sequencer::effects::filter_table::AnalysisMode::next)
                        .unwrap_or(sequencer::effects::filter_table::AnalysisMode::Wavetable),
                    Some(tag) => sequencer::effects::filter_table::AnalysisMode::from_tag(tag)
                        .ok_or_else(|| format!("unknown analysis mode '{tag}'"))?,
                };
                let path = app
                    .resolve_sample_path_by_name(&sample_name)
                    .ok_or_else(|| format!("sample '{sample_name}' could not be resolved"))?;
                let reference =
                    sequencer::effects::filter_table::encode_table_ref(&sample_name, mode);
                if let Some(bus_idx) = bus {
                    app.apply_recorded_bus_effect_value_mutation(
                        bus_idx,
                        slot,
                        "Set bus Filter Table mode",
                        "filter-table-source",
                        |app| {
                            app.set_filter_table_source_bus(bus_idx, slot, &path, &reference)
                        },
                    )?;
                } else if let Some(track) = track {
                    app::edit::apply_recorded_track_filter_table_mutation(
                        &mut app,
                        track,
                        slot,
                        &path,
                        &reference,
                    )
                    .map(|_| ())
                    .map_err(|error| format!("{error:?}"))?;
                } else {
                    return Err("need a track or bus".to_string());
                }
                Ok(format!("Filter Table mode: {}", mode.label()))
            })();
            match result {
                Ok(message) => {
                    queue_effect_panel_tree_invalidation(&ui_invalidations, track, bus);
                    editor.handle_host_event(HostEvent::Status(message));
                }
                Err(error) => editor.handle_host_event(HostEvent::Status(format!(
                    "Error switching Filter Table mode: {error}"
                ))),
            }
        }
        "set-filter-table-engine" => {
            // Swap the DSP engine (spectral STFT vs causal min-phase FIR) for
            // a live Filter Table slot. "toggle" flips; an engine tag selects
            // directly. Recorded as a chain mutation so undo/redo rebuild the
            // node from the retained per-engine source; PDC follows via the
            // per-frame latency refresh.
            let bus = extract_usize_from_payload(&payload, "bus");
            let track = extract_usize_from_payload(&payload, "track");
            let rack_slot = extract_usize_from_payload(&payload, "rack-slot");
            let slot = extract_usize_from_payload(&payload, "slot");
            let engine_str = extract_string_from_payload(&payload, "engine");
            let result = (|| -> Result<String, String> {
                let slot = slot.ok_or_else(|| "need a slot".to_string())?;
                let node_id = if let Some(bus_idx) = bus {
                    app.buses
                        .get(bus_idx)
                        .and_then(|bus| bus.effect_slots.get(slot))
                        .map(|state| state.node_id as i32)
                } else if let (Some(track), Some(rack_slot)) = (track, rack_slot) {
                    app.rack_slot_effect_snapshot(track, rack_slot)
                        .ok()
                        .and_then(|rack| rack.effect_slots.get(slot).map(|state| state.node_id as i32))
                } else if let Some(track) = track {
                    app.state
                        .pattern
                        .effect_chains
                        .get(track)
                        .and_then(|chain| chain.get(slot))
                        .map(|state| state.node_id.load(std::sync::atomic::Ordering::Relaxed) as i32)
                } else {
                    None
                }
                .ok_or_else(|| "need a track, rack slot, or bus".to_string())?;
                let current = sequencer::effects::filter_table::engine_for(node_id);
                let engine = match engine_str.as_deref() {
                    Some("toggle") | None => current.toggled(),
                    Some(tag) => sequencer::effects::filter_table::TableEngine::from_tag(tag)
                        .ok_or_else(|| format!("unknown engine '{tag}'"))?,
                };
                if engine == current {
                    return Ok(format!("Filter Table engine: {}", engine.display_name()));
                }
                if let Some(bus_idx) = bus {
                    app.apply_recorded_bus_effect_chain_mutation(
                        bus_idx,
                        "Set bus Filter Table engine",
                        |app| app.set_bus_filter_table_engine(bus_idx, slot, engine),
                    )?;
                } else if let (Some(track), Some(rack_slot)) = (track, rack_slot) {
                    app.apply_recorded_rack_effect_chain_mutation(
                        track,
                        rack_slot,
                        "Set rack Filter Table engine",
                        |app| app.set_rack_filter_table_engine(track, rack_slot, slot, engine),
                    )?;
                } else if let Some(track) = track {
                    app.apply_recorded_track_effect_chain_mutation(
                        track,
                        "Set Filter Table engine",
                        |app| app.set_track_filter_table_engine(track, slot, engine),
                    )?;
                }
                Ok(format!("Filter Table engine: {}", engine.display_name()))
            })();
            match result {
                Ok(message) => {
                    if let (Some(track), Some(_)) = (track, rack_slot) {
                        refresh_instrument_panel_reactive(
                            &mut editor,
                            &app,
                            track,
                            &selected_steps,
                            &ui_epoch,
                        );
                    } else {
                        queue_effect_panel_tree_invalidation(&ui_invalidations, track, bus);
                    }
                    editor.handle_host_event(HostEvent::Status(message));
                }
                Err(error) => editor.handle_host_event(HostEvent::Status(format!(
                    "Error switching Filter Table engine: {error}"
                ))),
            }
        }
        // ---- Filter Table response editor (eseq-dtx.8) ----------------
        // All arms delegate to the App session methods; the editor's own
        // undo/redo is the document history, while save commits through
        // the recorded-mutation path like any other table load. Band drag
        // :change events preview without touching history or the panel
        // value (the widget renders its own live drag); every other arm
        // rebuilds the effects value so the panel reflects session state.
        "filter-table-editor-open" => {
            use sequencer::effects::filter_table_editor::EditorTarget;
            let bus = extract_usize_from_payload(&payload, "bus");
            let track = extract_usize_from_payload(&payload, "track");
            let slot = extract_usize_from_payload(&payload, "slot");
            let result = (|| -> Result<(), String> {
                let slot = slot.ok_or_else(|| "need a slot".to_string())?;
                let target = if let Some(bus) = bus {
                    EditorTarget::Bus { bus, slot }
                } else if let Some(track) = track {
                    EditorTarget::Track { track, slot }
                } else {
                    return Err("need a track or bus".to_string());
                };
                app.open_filter_table_editor(target)
            })();
            match result {
                Ok(()) => {
                    refresh_filter_table_editor_panels(&ui_invalidations, None);
                    editor.handle_host_event(HostEvent::Status(
                        "Filter Table editor open".to_string(),
                    ));
                }
                Err(error) => editor.handle_host_event(HostEvent::Status(format!(
                    "Error opening Filter Table editor: {error}"
                ))),
            }
        }
        "filter-table-editor-close" => {
            // Closing drops the session, so the panel to rebuild has to be
            // resolved while it is still there.
            let target = sequencer::effects::filter_table_editor::session_ui_state()
                .map(|ui| ui.target);
            match app.close_filter_table_editor() {
                Ok(()) => {
                    refresh_filter_table_editor_panels(&ui_invalidations, target);
                    editor.handle_host_event(HostEvent::Status(
                        "Filter Table editor closed".to_string(),
                    ));
                }
                Err(error) => editor.handle_host_event(HostEvent::Status(format!(
                    "Error closing Filter Table editor: {error}"
                ))),
            }
        }
        "filter-table-editor-op" => {
            let result = filter_table_editor_op_from_payload(&payload)
                .and_then(|op| app.filter_table_editor_apply_op(op, false));
            match result {
                Ok(()) => {
                    refresh_filter_table_editor_panels(&ui_invalidations, None)
                }
                Err(error) => editor.handle_host_event(HostEvent::Status(format!(
                    "Filter Table edit failed: {error}"
                ))),
            }
        }
        "filter-table-editor-band" => {
            use sequencer::effects::filter_table_editor as fte;
            let phase =
                extract_string_from_payload(&payload, "phase").unwrap_or_default();
            let result = (|| -> Result<bool, String> {
                let ui = fte::session_ui_state()
                    .ok_or_else(|| "no Filter Table editor open".to_string())?;
                let node = parametric_node_from_band_payload(&payload)?;
                let op = fte::EditOp::Parametric {
                    frame_start: 0,
                    frame_end: ui.frames - 1,
                    node,
                };
                // The band handle renders from the newest applied op, so a drag
                // on it replaces that op rather than stacking a second copy.
                // Preview and commit must agree, or the audition during the
                // drag is not the table the release lands on.
                let replacing = ui.band.is_some();
                if phase == "commit" {
                    app.filter_table_editor_apply_op(op, replacing)?;
                    Ok(true)
                } else {
                    app.filter_table_editor_preview_op(op, replacing)?;
                    Ok(false)
                }
            })();
            match result {
                Ok(true) => {
                    refresh_filter_table_editor_panels(&ui_invalidations, None)
                }
                Ok(false) => {}
                Err(error) => editor.handle_host_event(HostEvent::Status(format!(
                    "Filter Table node edit failed: {error}"
                ))),
            }
        }
        "filter-table-editor-add-node" => {
            use sequencer::effects::filter_table_editor as fte;
            let result = (|| -> Result<(), String> {
                let ui = fte::session_ui_state()
                    .ok_or_else(|| "no Filter Table editor open".to_string())?;
                let kind = extract_string_from_payload(&payload, "kind")
                    .and_then(|tag| fte::ParametricKind::from_tag(&tag))
                    .ok_or_else(|| "unknown node kind".to_string())?;
                app.filter_table_editor_apply_op(
                    fte::EditOp::Parametric {
                        frame_start: 0,
                        frame_end: ui.frames - 1,
                        node: kind.default_node(),
                    },
                    false,
                )
            })();
            match result {
                Ok(()) => {
                    refresh_filter_table_editor_panels(&ui_invalidations, None)
                }
                Err(error) => editor.handle_host_event(HostEvent::Status(format!(
                    "Filter Table node add failed: {error}"
                ))),
            }
        }
        "filter-table-editor-undo" | "filter-table-editor-redo" => {
            match app.filter_table_editor_history(name == "filter-table-editor-redo") {
                Ok(stepped) => {
                    if stepped {
                        refresh_filter_table_editor_panels(&ui_invalidations, None);
                    }
                }
                Err(error) => editor.handle_host_event(HostEvent::Status(format!(
                    "Filter Table editor history failed: {error}"
                ))),
            }
        }
        "filter-table-editor-frame" => {
            let result = extract_usize_from_payload(&payload, "frame")
                .ok_or_else(|| "need a frame".to_string())
                .and_then(|frame| app.filter_table_editor_select_frame(frame));
            match result {
                Ok(()) => {
                    refresh_filter_table_editor_panels(&ui_invalidations, None)
                }
                Err(error) => editor.handle_host_event(HostEvent::Status(format!(
                    "Filter Table frame select failed: {error}"
                ))),
            }
        }
        "filter-table-editor-save" => {
            let requested = extract_string_from_payload(&payload, "name");
            match app.filter_table_editor_save(requested.as_deref()) {
                Ok(stem) => {
                    refresh_filter_table_editor_panels(&ui_invalidations, None);
                    editor.handle_host_event(HostEvent::Status(format!(
                        "Saved Filter Table '{stem}' to filter-tables/"
                    )));
                }
                Err(error) => editor.handle_host_event(HostEvent::Status(format!(
                    "Filter Table save failed: {error}"
                ))),
            }
        }
        "set-effect-param-batch" | "set-effect-plock-batch" => {
            if let Value::Map(ref map) = payload {
                let slot_idx = map_usize(map, "slot-idx");
                if let (Some(slot_idx), Some(updates)) =
                    (slot_idx, map_param_updates(map))
                {
                    let track = map_usize(map, "track")
                        .unwrap_or_else(|| current_track.load(Ordering::Relaxed));
                    let steps = map_usize_list(map, "steps").unwrap_or_else(|| {
                        selected_steps
                            .lock()
                            .unwrap()
                            .iter()
                            .copied()
                            .collect::<Vec<_>>()
                    });
                    let commands = updates
                        .into_iter()
                        .filter_map(|(param_idx, value)| {
                            let desc = app
                                .graph
                                .effect_descriptors
                                .get(track)?
                                .get(slot_idx)?
                                .params
                                .get(param_idx)?;
                            let value = value.clamp(desc.min, desc.max);
                            Some(if name == "set-effect-plock-batch" {
                                app::AppCommand::SetEffectPlockMulti {
                                    track,
                                    steps: steps.clone(),
                                    slot_idx,
                                    param_idx,
                                    value,
                                }
                            } else {
                                app::AppCommand::SetEffectParam {
                                    track,
                                    slot_idx,
                                    param_idx,
                                    value,
                                }
                            })
                        })
                        .collect::<Vec<_>>();
                    let result = if name == "set-effect-plock-batch" {
                        let gesture = map_string(map, "gesture")
                            .unwrap_or_else(|| "effect-curve".to_string());
                        let label = map_string(map, "label")
                            .unwrap_or_else(|| "Set effect curve".to_string());
                        app::edit::apply_coalesced_device_plock_batch(
                            &mut app,
                            &commands,
                            &gesture,
                            &label,
                        )
                    } else {
                        app::edit::apply_coalesced_device_value_batch(
                            &mut app,
                            &commands,
                            "effect-curve",
                            "Set effect curve",
                        )
                    };
                    if result.is_ok() {
                        let plocks_changed = name == "set-effect-plock-batch";
                        let display_step = if plocks_changed {
                            displayed_plock_step(
                                &state,
                                track,
                                selected_plock_step(&selected_steps),
                            )
                        } else {
                            None
                        };
                        let param_indices = commands
                            .iter()
                            .filter_map(|command| match command {
                                app::AppCommand::SetEffectParam { param_idx, .. }
                                | app::AppCommand::SetEffectPlockMulti {
                                    param_idx, ..
                                } => Some(*param_idx),
                                _ => None,
                            })
                            .collect::<Vec<_>>();
                        let neural_selection =
                            selected_neural_neurons.lock().unwrap().clone();
                        sync_effect_param_batch_display(
                            &mut editor,
                            &app,
                            &neural_selection,
                            track,
                            slot_idx,
                            &param_indices,
                            display_step,
                        );
                    }
                    match result {
                        Ok(_) if map_bool(map, "commit") => {
                            app::edit::finish_active_gesture(&mut app);
                            fx_epoch.fetch_add(1, Ordering::Relaxed);
                            ui_epoch.fetch_add(1, Ordering::Relaxed);
                        }
                        Ok(_) => {}
                        Err(error) => editor.handle_host_event(HostEvent::Error(
                            format!("effect parameter batch failed: {error:?}"),
                        )),
                    }
                }
            }
        }
        "set-effect-param" => {
            if let Value::Map(ref map) = payload {
                let slot_idx =
                    map.get("slot-idx").and_then(|cell| match &*cell.borrow() {
                        Value::Number(n) => Some(*n as usize),
                        _ => None,
                    });
                let target_node_id =
                    map.get("target-node-id").and_then(|cell| match &*cell.borrow() {
                        Value::Number(n) => Some(*n as u32),
                        _ => None,
                    });
                let param_idx =
                    map.get("param-idx").and_then(|cell| match &*cell.borrow() {
                        Value::Number(n) => Some(*n as usize),
                        _ => None,
                    });
                let value = map.get("value").and_then(|cell| match &*cell.borrow() {
                    Value::Number(n) => Some(*n as f32),
                    _ => None,
                });
                if let (Some(slot_idx), Some(param_idx), Some(value)) =
                    (slot_idx, param_idx, value)
                {
                    let track = current_track.load(Ordering::Relaxed);
                    let Some(slot_idx) = state.resolve_effect_slot_target(
                        track,
                        slot_idx,
                        target_node_id,
                    ) else {
                        editor.handle_host_event(HostEvent::Error(
                            "effect parameter target is no longer available".to_string(),
                        ));
                        return;
                    };
                    let desc = app
                        .graph
                        .effect_descriptors
                        .get(track)
                        .and_then(|slots| slots.get(slot_idx))
                        .and_then(|desc| desc.params.get(param_idx))
                        .cloned();
                    let clamped = desc
                        .as_ref()
                        .map(|p| value.clamp(p.min, p.max))
                        .unwrap_or(value);
                    let (neural_selection, wrote_neural_plock, neural_history_before) =
                        record_selected_neural_effect_plock(
                            &mut editor,
                            &state,
                            &selected_neural_neurons,
                            track,
                            slot_idx,
                            param_idx,
                        clamped,
                    );
                    if let Some(before) = neural_history_before {
                        app.commit_applied_scene_structure_mutation(
                            before,
                            "Edit neural override",
                        );
                    }
                    if !wrote_neural_plock {
                        app::apply_command(
                            &mut app,
                            app::AppCommand::SetEffectParam {
                                track,
                                slot_idx,
                                param_idx,
                                value: clamped,
                            },
                        );
                    }
                    sync_effect_param_authoring_display(
                        &mut editor,
                        EffectParamDisplaySync {
                            state: &state,
                            effect_descriptors: &app.graph.effect_descriptors,
                            app: &app,
                            selected_steps: &selected_steps,
                            selection: &neural_selection,
                            track,
                            slot_idx,
                            param_idx,
                            display_step: None,
                            sync_plock_list: wrote_neural_plock,
                        },
                    );
                    if desc.as_ref().is_some_and(param_change_needs_fx_rebuild) {
                        fx_epoch.fetch_add(1, Ordering::Relaxed);
                        ui_epoch.fetch_add(1, Ordering::Relaxed);
                    }
                }
            }
        }
        "toggle-effect-param" => {
            if let Value::Map(ref map) = payload {
                let slot_idx =
                    map.get("slot-idx").and_then(|cell| match &*cell.borrow() {
                        Value::Number(n) => Some(*n as usize),
                        _ => None,
                    });
                let param_idx =
                    map.get("param-idx").and_then(|cell| match &*cell.borrow() {
                        Value::Number(n) => Some(*n as usize),
                        _ => None,
                    });
                if let (Some(slot_idx), Some(param_idx)) = (slot_idx, param_idx) {
                    let selected: Vec<usize> =
                        selected_steps.lock().unwrap().iter().copied().collect();
                    if map_bool(map, "bus-fx") {
                        let bus_idx =
                            map.get("bus").and_then(|cell| match &*cell.borrow() {
                                Value::Number(n) => Some(*n as usize),
                                _ => None,
                            });
                        if let Some(bus_idx) = bus_idx {
                            let desc = app
                                .buses
                                .get(bus_idx)
                                .and_then(|bus| bus.effect_descriptors.get(slot_idx))
                                .and_then(|desc| desc.params.get(param_idx))
                                .cloned();
                            if let Some(desc) = desc {
                                let current = app
                                    .buses
                                    .get(bus_idx)
                                    .and_then(|bus| bus.effect_slots.get(slot_idx))
                                    .map(|slot| {
                                        let default = slot
                                            .defaults
                                            .get(param_idx)
                                            .copied()
                                            .unwrap_or(desc.default);
                                        selected
                                            .iter()
                                            .copied()
                                            .min()
                                            .and_then(|step| {
                                                slot.plocks
                                                    .get(step)
                                                    .and_then(|step_plocks| {
                                                        step_plocks.get(param_idx)
                                                    })
                                                    .copied()
                                                    .flatten()
                                            })
                                            .unwrap_or(default)
                                    })
                                    .unwrap_or(desc.default);
                                let next =
                                    desc.clamp(if current > 0.5 { 0.0 } else { 1.0 });
                                if selected.is_empty() {
                                    match app.apply_recorded_bus_effect_value_mutation(
                                        bus_idx,
                                        slot_idx,
                                        "Set bus effect parameter",
                                        format!("param:{param_idx}"),
                                        |app| app.set_bus_effect_param(
                                            bus_idx, slot_idx, param_idx, next,
                                        ),
                                    ) {
                                        Ok(()) => {
                                            app.publish_bus_effect_runtime();
                                            *bus_state.lock().unwrap() =
                                                app.buses.clone();
                                            if sync_bus_effect_param_value_field(
                                                editor.runtime_mut(),
                                                &app,
                                                bus_idx,
                                                slot_idx,
                                                param_idx,
                                            ) {
                                                editor.mark_needs_redraw();
                                            }
                                        }
                                        Err(error) => {
                                            editor.handle_host_event(
                                                HostEvent::Status(format!(
                                                    "Error toggling bus effect param: {error}"
                                                )),
                                            );
                                            return;
                                        }
                                    }
                                } else {
                                    let result = app.apply_recorded_bus_effect_value_mutation(
                                        bus_idx,
                                        slot_idx,
                                        "Set bus effect p-lock",
                                        format!("plock:param:{param_idx}"),
                                        |app| {
                                            for step in selected {
                                                app.set_bus_effect_plock(
                                                    bus_idx, slot_idx, step, param_idx, next,
                                                )?;
                                            }
                                            Ok(())
                                        },
                                    );
                                    if result.is_ok() {
                                        app.publish_bus_effect_runtime();
                                        *bus_state.lock().unwrap() = app.buses.clone();
                                    } else if let Err(error) = result {
                                        editor.handle_host_event(HostEvent::Status(format!(
                                            "Error toggling bus effect p-lock: {error}"
                                        )));
                                    }
                                }
                                fx_epoch.fetch_add(1, Ordering::Relaxed);
                                ui_epoch.fetch_add(1, Ordering::Relaxed);
                            }
                        }
                    } else if map_bool(map, "midi-fx") {
                        let track = current_track.load(Ordering::Relaxed);
                        let chain = state.pattern.track_params[track].midi_fx_chain();
                        let desc = chain
                            .get(slot_idx)
                            .and_then(|name| {
                                sequencer::lisp_host::load_midi_fx_descriptor(name)
                            })
                            .and_then(|desc| desc.params.get(param_idx).cloned());
                        if let Some(desc) = desc {
                            if let Some(slot) = state
                                .pattern
                                .midi_fx_slots
                                .get(track)
                                .and_then(|slots| slots.get(slot_idx))
                            {
                                let default = slot.defaults.get(param_idx);
                                let current = selected
                                    .iter()
                                    .copied()
                                    .min()
                                    .and_then(|step| slot.plocks.get(step, param_idx))
                                    .unwrap_or(default);
                                let next =
                                    desc.clamp(if current > 0.5 { 0.0 } else { 1.0 });
                                if selected.is_empty() {
                                    app::apply_command(
                                        &mut app,
                                        app::AppCommand::SetMidiFxParam {
                                            track,
                                            slot_idx,
                                            param_idx,
                                            value: next,
                                        },
                                    );
                                    if sync_midi_fx_param_value_field(
                                        editor.runtime_mut(),
                                        &state,
                                        track,
                                        slot_idx,
                                        param_idx,
                                        None,
                                    ) {
                                        editor.mark_needs_redraw();
                                    }
                                } else {
                                    app::apply_command(
                                        &mut app,
                                        app::AppCommand::SetMidiFxPlockMulti {
                                            track,
                                            steps: selected,
                                            slot_idx,
                                            param_idx,
                                            value: next,
                                        },
                                    );
                                }
                                fx_epoch.fetch_add(1, Ordering::Relaxed);
                                ui_epoch.fetch_add(1, Ordering::Relaxed);
                            }
                        }
                    } else {
                        let track = current_track.load(Ordering::Relaxed);
                        let desc = app
                            .graph
                            .effect_descriptors
                            .get(track)
                            .and_then(|slots| slots.get(slot_idx))
                            .and_then(|desc| desc.params.get(param_idx))
                            .cloned();
                        if let Some(desc) = desc {
                            let chain = &state.pattern.effect_chains[track];
                            let neural_selection =
                                selected_neural_neurons.lock().unwrap().clone();
                            let current = chain
                                .get(slot_idx)
                                .map(|slot| {
                                    let default = slot.defaults.get(param_idx);
                                    sequencer::lisp_host::selected_neural_effect_plock_value(
                                        &state,
                                        &neural_selection,
                                        track,
                                        slot_idx,
                                        param_idx,
                                    )
                                    .or_else(|| {
                                        selected
                                            .iter()
                                            .copied()
                                            .min()
                                            .and_then(|step| {
                                                slot.plocks.get(step, param_idx)
                                            })
                                    })
                                    .unwrap_or(default)
                                })
                                .unwrap_or(desc.default);
                            let next =
                                desc.clamp(if current > 0.5 { 0.0 } else { 1.0 });
                            let neural_history_before = (!neural_selection.is_empty())
                                .then(|| state.capture_project_scenes());
                            let wrote_neural_plock = write_selected_neural_effect_plock(
                                &mut editor,
                                &state,
                                &neural_selection,
                                track,
                                slot_idx,
                                param_idx,
                                next,
                            );
                            if let Some(before) =
                                neural_history_before.filter(|_| wrote_neural_plock)
                            {
                                app.commit_applied_scene_structure_mutation(
                                    before,
                                    "Edit neural override",
                                );
                            }
                            if wrote_neural_plock {
                                sync_effect_param_authoring_display(
                                    &mut editor,
                                    EffectParamDisplaySync {
                                        state: &state,
                                        effect_descriptors: &app
                                            .graph
                                            .effect_descriptors,
                                        app: &app,
                                        selected_steps: &selected_steps,
                                        selection: &neural_selection,
                                        track,
                                        slot_idx,
                                        param_idx,
                                        display_step: None,
                                        sync_plock_list: true,
                                    },
                                );
                            } else if selected.is_empty() {
                                app::apply_command(
                                    &mut app,
                                    app::AppCommand::SetEffectParam {
                                        track,
                                        slot_idx,
                                        param_idx,
                                        value: next,
                                    },
                                );
                                if sync_track_effect_param_value_field(
                                    editor.runtime_mut(),
                                    &app,
                                    track,
                                    slot_idx,
                                    param_idx,
                                    None,
                                ) {
                                    editor.mark_needs_redraw();
                                }
                            } else {
                                app::apply_command(
                                    &mut app,
                                    app::AppCommand::SetEffectPlockMulti {
                                        track,
                                        slot_idx,
                                        steps: selected,
                                        param_idx,
                                        value: next,
                                    },
                                );
                            }
                            fx_epoch.fetch_add(1, Ordering::Relaxed);
                            ui_epoch.fetch_add(1, Ordering::Relaxed);
                        }
                    }
                }
            }
        }
        "set-effect-param-option" => {
            if let Value::Map(ref map) = payload {
                let slot_idx =
                    map.get("slot-idx").and_then(|cell| match &*cell.borrow() {
                        Value::Number(n) => Some(*n as usize),
                        _ => None,
                    });
                let target_node_id =
                    map.get("target-node-id").and_then(|cell| match &*cell.borrow() {
                        Value::Number(n) => Some(*n as u32),
                        _ => None,
                    });
                let param_idx =
                    map.get("param-idx").and_then(|cell| match &*cell.borrow() {
                        Value::Number(n) => Some(*n as usize),
                        _ => None,
                    });
                let label = map.get("label").and_then(|cell| match &*cell.borrow() {
                    Value::String(s) => Some(s.clone()),
                    _ => None,
                });
                if let (Some(slot_idx), Some(param_idx), Some(label)) =
                    (slot_idx, param_idx, label)
                {
                    let track = current_track.load(Ordering::Relaxed);
                    let Some(slot_idx) = state.resolve_effect_slot_target(
                        track,
                        slot_idx,
                        target_node_id,
                    ) else {
                        editor.handle_host_event(HostEvent::Error(
                            "effect parameter target is no longer available".to_string(),
                        ));
                        return;
                    };
                    let selected_idx = app
                        .graph
                        .effect_descriptors
                        .get(track)
                        .and_then(|d| d.get(slot_idx))
                        .and_then(|d| d.params.get(param_idx))
                        .and_then(|p| match &p.kind {
                            sequencer::effects::ParamKind::Enum { labels } => {
                                labels.iter().position(|item| item == &label)
                            }
                            _ => None,
                        })
                        .or_else(|| {
                            let is_delay_time = app
                                .graph
                                .effect_descriptors
                                .get(track)
                                .and_then(|d| d.get(slot_idx))
                                .map(|d| d.name == "Delay")
                                .unwrap_or(false)
                                && param_idx == 2;
                            is_delay_time.then(|| {
                                sequencer::effects::SyncDivision::ALL
                                    .iter()
                                    .position(|div| div.label() == label)
                            })?
                        });
                    if let Some(selected_idx) = selected_idx {
                        let is_host_sidechain = matches!(
                            app.graph
                                .effect_descriptors
                                .get(track)
                                .and_then(|d| d.get(slot_idx))
                                .and_then(|d| d.params.get(param_idx))
                                .and_then(|p| p.host_control.as_ref()),
                            Some(sequencer::effects::HostControl::FxSidechain { .. })
                        );
                        if is_host_sidechain {
                            app::apply_command(
                                &mut app,
                                app::AppCommand::SetEffectParam {
                                    track,
                                    slot_idx,
                                    param_idx,
                                    value: selected_idx as f32,
                                },
                            );
                        } else {
                            let value = selected_idx as f32;
                            let (neural_selection, wrote_neural_plock, neural_history_before) =
                                record_selected_neural_effect_plock(
                                    &mut editor,
                                    &state,
                                    &selected_neural_neurons,
                                    track,
                                    slot_idx,
                                    param_idx,
                                value,
                            );
                            if let Some(before) = neural_history_before {
                                app.commit_applied_scene_structure_mutation(
                                    before,
                                    "Edit neural override",
                                );
                            }
                            if !wrote_neural_plock {
                                app::apply_command(
                                    &mut app,
                                    app::AppCommand::SetEffectParam {
                                        track,
                                        slot_idx,
                                        param_idx,
                                        value,
                                    },
                                );
                            }
                            sync_effect_param_authoring_display(
                                &mut editor,
                                EffectParamDisplaySync {
                                    state: &state,
                                    effect_descriptors: &app.graph.effect_descriptors,
                                    app: &app,
                                    selected_steps: &selected_steps,
                                    selection: &neural_selection,
                                    track,
                                    slot_idx,
                                    param_idx,
                                    display_step: None,
                                    sync_plock_list: wrote_neural_plock,
                                },
                            );
                        }
                        fx_epoch.fetch_add(1, Ordering::Relaxed);
                        ui_epoch.fetch_add(1, Ordering::Relaxed);
                    }
                }
            }
        }
        "set-effect-plock-option" => {
            if let Value::Map(ref map) = payload {
                let slot_idx =
                    map.get("slot-idx").and_then(|cell| match &*cell.borrow() {
                        Value::Number(n) => Some(*n as usize),
                        _ => None,
                    });
                let target_node_id =
                    map.get("target-node-id").and_then(|cell| match &*cell.borrow() {
                        Value::Number(n) => Some(*n as u32),
                        _ => None,
                    });
                let param_idx =
                    map.get("param-idx").and_then(|cell| match &*cell.borrow() {
                        Value::Number(n) => Some(*n as usize),
                        _ => None,
                    });
                let label = map.get("label").and_then(|cell| match &*cell.borrow() {
                    Value::String(s) => Some(s.clone()),
                    _ => None,
                });
                if let (Some(slot_idx), Some(param_idx), Some(label)) =
                    (slot_idx, param_idx, label)
                {
                    let track = current_track.load(Ordering::Relaxed);
                    let Some(slot_idx) = state.resolve_effect_slot_target(
                        track,
                        slot_idx,
                        target_node_id,
                    ) else {
                        editor.handle_host_event(HostEvent::Error(
                            "effect parameter target is no longer available".to_string(),
                        ));
                        return;
                    };
                    let selected_idx = app
                        .graph
                        .effect_descriptors
                        .get(track)
                        .and_then(|d| d.get(slot_idx))
                        .and_then(|d| d.params.get(param_idx))
                        .and_then(|p| match &p.kind {
                            sequencer::effects::ParamKind::Enum { labels } => {
                                labels.iter().position(|item| item == &label)
                            }
                            _ => None,
                        })
                        .or_else(|| {
                            let is_delay_time = app
                                .graph
                                .effect_descriptors
                                .get(track)
                                .and_then(|d| d.get(slot_idx))
                                .map(|d| d.name == "Delay")
                                .unwrap_or(false)
                                && param_idx == 2;
                            is_delay_time.then(|| {
                                sequencer::effects::SyncDivision::ALL
                                    .iter()
                                    .position(|div| div.label() == label)
                            })?
                        });
                    if let Some(selected_idx) = selected_idx {
                        let is_host_sidechain = matches!(
                            app.graph
                                .effect_descriptors
                                .get(track)
                                .and_then(|d| d.get(slot_idx))
                                .and_then(|d| d.params.get(param_idx))
                                .and_then(|p| p.host_control.as_ref()),
                            Some(sequencer::effects::HostControl::FxSidechain { .. })
                        );
                        if is_host_sidechain {
                            app::apply_command(
                                &mut app,
                                app::AppCommand::SetEffectParam {
                                    track,
                                    slot_idx,
                                    param_idx,
                                    value: selected_idx as f32,
                                },
                            );
                        } else {
                            let value = selected_idx as f32;
                            let (neural_selection, wrote_neural_plock, neural_history_before) =
                                record_selected_neural_effect_plock(
                                    &mut editor,
                                    &state,
                                    &selected_neural_neurons,
                                    track,
                                    slot_idx,
                                    param_idx,
                                value,
                            );
                            if let Some(before) = neural_history_before {
                                app.commit_applied_scene_structure_mutation(
                                    before,
                                    "Edit neural override",
                                );
                            }
                            if !wrote_neural_plock {
                                let steps: Vec<usize> = selected_steps
                                    .lock()
                                    .unwrap()
                                    .iter()
                                    .copied()
                                    .collect();
                                app::apply_command(
                                    &mut app,
                                    app::AppCommand::SetEffectPlockMulti {
                                        track,
                                        slot_idx,
                                        steps,
                                        param_idx,
                                        value,
                                    },
                                );
                            }
                            sync_effect_param_authoring_display(
                                &mut editor,
                                EffectParamDisplaySync {
                                    state: &state,
                                    effect_descriptors: &app.graph.effect_descriptors,
                                    app: &app,
                                    selected_steps: &selected_steps,
                                    selection: &neural_selection,
                                    track,
                                    slot_idx,
                                    param_idx,
                                    display_step: None,
                                    sync_plock_list: wrote_neural_plock,
                                },
                            );
                        }
                        fx_epoch.fetch_add(1, Ordering::Relaxed);
                        ui_epoch.fetch_add(1, Ordering::Relaxed);
                    }
                }
            }
        }
        "set-midi-fx-param" => {
            if let Value::Map(ref map) = payload {
                let slot_idx =
                    map.get("slot-idx").and_then(|cell| match &*cell.borrow() {
                        Value::Number(n) => Some(*n as usize),
                        _ => None,
                    });
                let param_idx =
                    map.get("param-idx").and_then(|cell| match &*cell.borrow() {
                        Value::Number(n) => Some(*n as usize),
                        _ => None,
                    });
                let value = map.get("value").and_then(|cell| match &*cell.borrow() {
                    Value::Number(n) => Some(*n as f32),
                    _ => None,
                });
                if let (Some(slot_idx), Some(param_idx), Some(value)) =
                    (slot_idx, param_idx, value)
                {
                    let track = current_track.load(Ordering::Relaxed);
                    let chain = state.pattern.track_params[track].midi_fx_chain();
                    let desc = chain
                        .get(slot_idx)
                        .and_then(|name| {
                            sequencer::lisp_host::load_midi_fx_descriptor(name)
                        })
                        .and_then(|desc| desc.params.get(param_idx).cloned());
                    let clamped = desc
                        .as_ref()
                        .map(|p| value.clamp(p.min, p.max))
                        .unwrap_or(value);
                    if let Some(_slot) = state
                        .pattern
                        .midi_fx_slots
                        .get(track)
                        .and_then(|slots| slots.get(slot_idx))
                    {
                        app::apply_command(
                            &mut app,
                            app::AppCommand::SetMidiFxParam {
                                track,
                                slot_idx,
                                param_idx,
                                value: clamped,
                            },
                        );
                        sync_midi_fx_param_value_field(
                            editor.runtime_mut(),
                            &state,
                            track,
                            slot_idx,
                            param_idx,
                            None,
                        );
                        if desc.as_ref().is_some_and(param_change_needs_fx_rebuild) {
                            fx_epoch.fetch_add(1, Ordering::Relaxed);
                            ui_epoch.fetch_add(1, Ordering::Relaxed);
                        }
                    }
                }
            }
        }
        "set-midi-fx-plock" => {
            if let Value::Map(ref map) = payload {
                let slot_idx =
                    map.get("slot-idx").and_then(|cell| match &*cell.borrow() {
                        Value::Number(n) => Some(*n as usize),
                        _ => None,
                    });
                let param_idx =
                    map.get("param-idx").and_then(|cell| match &*cell.borrow() {
                        Value::Number(n) => Some(*n as usize),
                        _ => None,
                    });
                let value = map.get("value").and_then(|cell| match &*cell.borrow() {
                    Value::Number(n) => Some(*n as f32),
                    _ => None,
                });
                if let (Some(slot_idx), Some(param_idx), Some(value)) =
                    (slot_idx, param_idx, value)
                {
                    let track = current_track.load(Ordering::Relaxed);
                    let chain = state.pattern.track_params[track].midi_fx_chain();
                    let clamped = chain
                        .get(slot_idx)
                        .and_then(|name| {
                            sequencer::lisp_host::load_midi_fx_descriptor(name)
                        })
                        .and_then(|desc| desc.params.get(param_idx).cloned())
                        .map(|p| value.clamp(p.min, p.max))
                        .unwrap_or(value);
                    if let Some(_slot) = state
                        .pattern
                        .midi_fx_slots
                        .get(track)
                        .and_then(|slots| slots.get(slot_idx))
                    {
                        let steps: Vec<usize> =
                            selected_steps.lock().unwrap().iter().copied().collect();
                        app::apply_command(
                            &mut app,
                            app::AppCommand::SetMidiFxPlockMulti {
                                track,
                                steps,
                                slot_idx,
                                param_idx,
                                value: clamped,
                            },
                        );
                        fx_epoch.fetch_add(1, Ordering::Relaxed);
                        ui_epoch.fetch_add(1, Ordering::Relaxed);
                    }
                }
            }
        }
        "set-midi-fx-param-option" => {
            if let Value::Map(ref map) = payload {
                let slot_idx =
                    map.get("slot-idx").and_then(|cell| match &*cell.borrow() {
                        Value::Number(n) => Some(*n as usize),
                        _ => None,
                    });
                let param_idx =
                    map.get("param-idx").and_then(|cell| match &*cell.borrow() {
                        Value::Number(n) => Some(*n as usize),
                        _ => None,
                    });
                let label = map.get("label").and_then(|cell| match &*cell.borrow() {
                    Value::String(s) => Some(s.clone()),
                    _ => None,
                });
                if let (Some(slot_idx), Some(param_idx), Some(label)) =
                    (slot_idx, param_idx, label)
                {
                    let track = current_track.load(Ordering::Relaxed);
                    let chain = state.pattern.track_params[track].midi_fx_chain();
                    if let Some(selected_idx) = chain
                        .get(slot_idx)
                        .and_then(|name| midi_fx_option_index(name, param_idx, &label))
                    {
                        if let Some(_slot) = state
                            .pattern
                            .midi_fx_slots
                            .get(track)
                            .and_then(|slots| slots.get(slot_idx))
                        {
                            app::apply_command(
                                &mut app,
                                app::AppCommand::SetMidiFxParam {
                                    track,
                                    slot_idx,
                                    param_idx,
                                    value: selected_idx as f32,
                                },
                            );
                            fx_epoch.fetch_add(1, Ordering::Relaxed);
                            ui_epoch.fetch_add(1, Ordering::Relaxed);
                        }
                    }
                }
            }
        }
        "set-midi-fx-plock-option" => {
            if let Value::Map(ref map) = payload {
                let slot_idx =
                    map.get("slot-idx").and_then(|cell| match &*cell.borrow() {
                        Value::Number(n) => Some(*n as usize),
                        _ => None,
                    });
                let param_idx =
                    map.get("param-idx").and_then(|cell| match &*cell.borrow() {
                        Value::Number(n) => Some(*n as usize),
                        _ => None,
                    });
                let label = map.get("label").and_then(|cell| match &*cell.borrow() {
                    Value::String(s) => Some(s.clone()),
                    _ => None,
                });
                if let (Some(slot_idx), Some(param_idx), Some(label)) =
                    (slot_idx, param_idx, label)
                {
                    let track = current_track.load(Ordering::Relaxed);
                    let chain = state.pattern.track_params[track].midi_fx_chain();
                    if let Some(selected_idx) = chain
                        .get(slot_idx)
                        .and_then(|name| midi_fx_option_index(name, param_idx, &label))
                    {
                        if let Some(_slot) = state
                            .pattern
                            .midi_fx_slots
                            .get(track)
                            .and_then(|slots| slots.get(slot_idx))
                        {
                            let steps: Vec<usize> = selected_steps
                                .lock()
                                .unwrap()
                                .iter()
                                .copied()
                                .collect();
                            app::apply_command(
                                &mut app,
                                app::AppCommand::SetMidiFxPlockMulti {
                                    track,
                                    steps,
                                    slot_idx,
                                    param_idx,
                                    value: selected_idx as f32,
                                },
                            );
                            fx_epoch.fetch_add(1, Ordering::Relaxed);
                            ui_epoch.fetch_add(1, Ordering::Relaxed);
                        }
                    }
                }
            }
        }
        "add-effect" => {
            if let Value::Map(ref map) = payload {
                if let Some(cell) = map.get("name") {
                    if let Value::String(effect_name) = &*cell.borrow() {
                        let effect_name = effect_name.clone();
                        app.ui.cursor_track = current_track.load(Ordering::Relaxed);
                        if let Some(slot_idx) = app.next_free_custom_slot() {
                            app.start_effect_compile(&effect_name, slot_idx);
                            editor.runtime_mut().set_reactive(
                                "SEQ",
                                "compiling",
                                Value::Bool(true),
                            );
                        } else {
                            editor.handle_host_event(HostEvent::Status(
                                "No free effect slots available".to_string(),
                            ));
                        }
                    }
                }
            }
        }
        "add-effect-to-track" => {
            let track = extract_usize_from_payload(&payload, "track");
            let effect_name = extract_string_from_payload(&payload, "name");
            if let (Some(track), Some(effect_name)) = (track, effect_name) {
                if track >= app.tracks.len() {
                    editor.handle_host_event(HostEvent::Status(format!(
                        "Track {} does not exist",
                        track + 1
                    )));
                    return;
                }
                current_track.store(track, Ordering::Relaxed);
                app.ui.cursor_track = track;
                if let Some(slot_idx) = app.next_free_custom_slot() {
                    app.start_effect_compile(&effect_name, slot_idx);
                    let rt = editor.runtime_mut();
                    set_current_track_reactive(rt, app.tracks.len(), track);
                    rt.set_reactive("SEQ", "compiling", Value::Bool(true));
                    sync_track_mixer_state(rt, &app, &state);
                    sync_sidebar_browser(rt, &app, track);
                    rt.run_reactive_cycle();
                    editor.refresh_runtime_side_effects();
                    ui_epoch.fetch_add(1, Ordering::Relaxed);
                    editor.handle_host_event(HostEvent::Status(format!(
                        "Adding effect '{}' to track {}",
                        effect_name,
                        track + 1
                    )));
                } else {
                    editor.handle_host_event(HostEvent::Status(
                        "No free effect slots available".to_string(),
                    ));
                }
            }
        }
        "add-builtin-effect" => {
            if let Value::Map(ref map) = payload {
                if let Some(cell) = map.get("name") {
                    if let Value::String(effect_name) = &*cell.borrow() {
                        let effect_name = effect_name.clone();
                        let track = current_track.load(Ordering::Relaxed);
                        app.ui.cursor_track = track;
                        match app.apply_recorded_track_effect_chain_mutation(
                            track,
                            "Add audio effect",
                            |app| app.add_builtin_effect_sync(track, &effect_name),
                        ) {
                            Ok(slot_idx) => {
                                let rt = editor.runtime_mut();
                                rt.set_reactive(
                                    "SEQ",
                                    "effects",
                                    build_effects_value(
                                        &state,
                                        track,
                                        &app.graph.effect_descriptors,
                                        &selected_steps,
                                    ),
                                );
                                rt.set_reactive(
                                    "SEQ",
                                    "step-has-plocks",
                                    build_step_has_plocks(
                                        &state,
                                        track,
                                        &app.graph.effect_descriptors,
                                    ),
                                );
                                rt.run_reactive_cycle();
                                editor.refresh_runtime_side_effects();
                                editor.reset_widget_scroll_for_buffer_named("*fx*");
                                fx_epoch.fetch_add(1, Ordering::Relaxed);
                                ui_epoch.fetch_add(1, Ordering::Relaxed);
                                editor.handle_host_event(HostEvent::Status(format!(
                                    "Added built-in effect '{}' to slot {}",
                                    effect_name,
                                    slot_idx + 1
                                )));
                            }
                            Err(error) => editor.handle_host_event(HostEvent::Status(
                                format!("Error adding built-in effect: {error}"),
                            )),
                        }
                    }
                }
            }
        }
        "add-builtin-effect-to-track" => {
            let track = extract_usize_from_payload(&payload, "track");
            let effect_name = extract_string_from_payload(&payload, "name");
            if let (Some(track), Some(effect_name)) = (track, effect_name) {
                if track >= app.tracks.len() {
                    editor.handle_host_event(HostEvent::Status(format!(
                        "Track {} does not exist",
                        track + 1
                    )));
                    return;
                }
                current_track.store(track, Ordering::Relaxed);
                app.ui.cursor_track = track;
                match app.apply_recorded_track_effect_chain_mutation(
                    track,
                    "Add audio effect",
                    |app| app.add_builtin_effect_sync(track, &effect_name),
                ) {
                    Ok(slot_idx) => {
                        let rt = editor.runtime_mut();
                        set_current_track_reactive(rt, app.tracks.len(), track);
                        rt.set_reactive(
                            "SEQ",
                            "effects",
                            build_effects_value(
                                &state,
                                track,
                                &app.graph.effect_descriptors,
                                &selected_steps,
                            ),
                        );
                        rt.set_reactive(
                            "SEQ",
                            "step-has-plocks",
                            build_step_has_plocks(
                                &state,
                                track,
                                &app.graph.effect_descriptors,
                            ),
                        );
                        sync_track_mixer_state(rt, &app, &state);
                        sync_sidebar_browser(rt, &app, track);
                        rt.run_reactive_cycle();
                        editor.refresh_runtime_side_effects();
                        editor.reset_widget_scroll_for_buffer_named("*fx*");
                        fx_epoch.fetch_add(1, Ordering::Relaxed);
                        ui_epoch.fetch_add(1, Ordering::Relaxed);
                        editor.handle_host_event(HostEvent::Status(format!(
                            "Added built-in effect '{}' to track {} slot {}",
                            effect_name,
                            track + 1,
                            slot_idx + 1
                        )));
                    }
                    Err(error) => editor.handle_host_event(HostEvent::Status(format!(
                        "Error adding built-in effect: {error}"
                    ))),
                }
            }
        }
        "add-midi-fx" => {
            if let Value::Map(ref map) = payload {
                if let Some(cell) = map.get("name") {
                    if let Value::String(fx_name) = &*cell.borrow() {
                        let fx_name = fx_name.clone();
                        let track = current_track.load(Ordering::Relaxed);
                        match app.apply_recorded_track_midi_fx_chain_mutation(
                            track,
                            "Add MIDI FX",
                            |app| app.add_midi_fx_to_track_sync(track, &fx_name),
                        ) {
                            Ok(slot_idx) => {
                                let rt = editor.runtime_mut();
                                rt.set_reactive(
                                    "SEQ",
                                    "midi-effects",
                                    build_midi_effects_value(
                                        &state,
                                        track,
                                        &selected_steps,
                                    ),
                                );
                                rt.set_reactive(
                                    "SEQ",
                                    "step-has-plocks",
                                    build_step_has_plocks(
                                        &state,
                                        track,
                                        &app.graph.effect_descriptors,
                                    ),
                                );
                                rt.run_reactive_cycle();
                                editor.refresh_runtime_side_effects();
                                fx_epoch.fetch_add(1, Ordering::Relaxed);
                                ui_epoch.fetch_add(1, Ordering::Relaxed);
                                editor.handle_host_event(HostEvent::Status(format!(
                                    "Added MIDI FX '{}' to slot {}",
                                    fx_name,
                                    slot_idx + 1
                                )));
                            }
                            Err(e) => editor.handle_host_event(HostEvent::Status(
                                format!("Error adding MIDI FX: {e}"),
                            )),
                        }
                    }
                }
            }
        }
        "add-midi-fx-to-track" => {
            let track = extract_usize_from_payload(&payload, "track");
            let fx_name = extract_string_from_payload(&payload, "name");
            if let (Some(track), Some(fx_name)) = (track, fx_name) {
                if track >= app.tracks.len() {
                    editor.handle_host_event(HostEvent::Status(format!(
                        "Track {} does not exist",
                        track + 1
                    )));
                    return;
                }
                current_track.store(track, Ordering::Relaxed);
                app.ui.cursor_track = track;
                match app.apply_recorded_track_midi_fx_chain_mutation(
                    track,
                    "Add MIDI FX",
                    |app| app.add_midi_fx_to_track_sync(track, &fx_name),
                ) {
                    Ok(slot_idx) => {
                        let rt = editor.runtime_mut();
                        set_current_track_reactive(rt, app.tracks.len(), track);
                        rt.set_reactive(
                            "SEQ",
                            "midi-effects",
                            build_midi_effects_value(&state, track, &selected_steps),
                        );
                        rt.set_reactive(
                            "SEQ",
                            "step-has-plocks",
                            build_step_has_plocks(
                                &state,
                                track,
                                &app.graph.effect_descriptors,
                            ),
                        );
                        sync_track_mixer_state(rt, &app, &state);
                        sync_sidebar_browser(rt, &app, track);
                        rt.run_reactive_cycle();
                        editor.refresh_runtime_side_effects();
                        editor.reset_widget_scroll_for_buffer_named("*fx*");
                        fx_epoch.fetch_add(1, Ordering::Relaxed);
                        ui_epoch.fetch_add(1, Ordering::Relaxed);
                        editor.handle_host_event(HostEvent::Status(format!(
                            "Added MIDI FX '{}' to track {} slot {}",
                            fx_name,
                            track + 1,
                            slot_idx + 1
                        )));
                    }
                    Err(error) => editor.handle_host_event(HostEvent::Status(format!(
                        "Error adding MIDI FX: {error}"
                    ))),
                }
            }
        }
        "insert-builtin-effect-before-slot" => {
            let track = extract_usize_from_payload(&payload, "track");
            let slot = extract_usize_from_payload(&payload, "slot");
            let effect_name = extract_string_from_payload(&payload, "name");
            if let (Some(track), Some(slot), Some(effect_name)) =
                (track, slot, effect_name)
            {
                current_track.store(track, Ordering::Relaxed);
                app.ui.cursor_track = track;
                match app.apply_recorded_track_effect_chain_mutation(
                    track,
                    "Insert audio effect",
                    |app| app.insert_builtin_effect_before_slot_sync(
                        track,
                        slot,
                        &effect_name,
                    ),
                ) {
                    Ok(slot_idx) => {
                        let rt = editor.runtime_mut();
                        set_current_track_reactive(rt, app.tracks.len(), track);
                        rt.set_reactive(
                            "SEQ",
                            "effects",
                            build_effects_value(
                                &state,
                                track,
                                &app.graph.effect_descriptors,
                                &selected_steps,
                            ),
                        );
                        rt.set_reactive(
                            "SEQ",
                            "step-has-plocks",
                            build_step_has_plocks(
                                &state,
                                track,
                                &app.graph.effect_descriptors,
                            ),
                        );
                        sync_track_mixer_state(rt, &app, &state);
                        sync_sidebar_browser(rt, &app, track);
                        rt.run_reactive_cycle();
                        editor.refresh_runtime_side_effects();
                        fx_epoch.fetch_add(1, Ordering::Relaxed);
                        ui_epoch.fetch_add(1, Ordering::Relaxed);
                        editor.handle_host_event(HostEvent::Status(format!(
                            "Inserted built-in effect '{}' at slot {}",
                            effect_name,
                            slot_idx + 1
                        )));
                    }
                    Err(error) => editor.handle_host_event(HostEvent::Status(format!(
                        "Error inserting built-in effect: {error}"
                    ))),
                }
            }
        }
        "insert-effect-before-slot" => {
            let track = extract_usize_from_payload(&payload, "track");
            let slot = extract_usize_from_payload(&payload, "slot");
            let effect_name = extract_string_from_payload(&payload, "name");
            if let (Some(track), Some(slot), Some(effect_name)) =
                (track, slot, effect_name)
            {
                current_track.store(track, Ordering::Relaxed);
                app.ui.cursor_track = track;
                match app.apply_recorded_track_effect_chain_mutation(
                    track,
                    "Insert audio effect",
                    |app| app.insert_saved_effect_before_slot_sync(
                        track,
                        slot,
                        &effect_name,
                    ),
                ) {
                    Ok(slot_idx) => {
                        let rt = editor.runtime_mut();
                        set_current_track_reactive(rt, app.tracks.len(), track);
                        rt.set_reactive(
                            "SEQ",
                            "effects",
                            build_effects_value(
                                &state,
                                track,
                                &app.graph.effect_descriptors,
                                &selected_steps,
                            ),
                        );
                        rt.set_reactive(
                            "SEQ",
                            "step-has-plocks",
                            build_step_has_plocks(
                                &state,
                                track,
                                &app.graph.effect_descriptors,
                            ),
                        );
                        sync_track_mixer_state(rt, &app, &state);
                        sync_sidebar_browser(rt, &app, track);
                        rt.run_reactive_cycle();
                        editor.refresh_runtime_side_effects();
                        fx_epoch.fetch_add(1, Ordering::Relaxed);
                        ui_epoch.fetch_add(1, Ordering::Relaxed);
                        editor.handle_host_event(HostEvent::Status(format!(
                            "Inserted effect '{}' at slot {}",
                            effect_name,
                            slot_idx + 1
                        )));
                    }
                    Err(error) => editor.handle_host_event(HostEvent::Status(format!(
                        "Error inserting effect: {error}"
                    ))),
                }
            }
        }
        "insert-midi-fx-before-slot" => {
            let track = extract_usize_from_payload(&payload, "track");
            let slot = extract_usize_from_payload(&payload, "slot");
            let fx_name = extract_string_from_payload(&payload, "name");
            if let (Some(track), Some(slot), Some(fx_name)) = (track, slot, fx_name) {
                current_track.store(track, Ordering::Relaxed);
                app.ui.cursor_track = track;
                match app.apply_recorded_track_midi_fx_chain_mutation(
                    track,
                    "Insert MIDI FX",
                    |app| app.insert_midi_fx_before_slot_sync(track, slot, &fx_name),
                ) {
                    Ok(slot_idx) => {
                        let rt = editor.runtime_mut();
                        set_current_track_reactive(rt, app.tracks.len(), track);
                        rt.set_reactive(
                            "SEQ",
                            "midi-effects",
                            build_midi_effects_value(&state, track, &selected_steps),
                        );
                        rt.set_reactive(
                            "SEQ",
                            "step-has-plocks",
                            build_step_has_plocks(
                                &state,
                                track,
                                &app.graph.effect_descriptors,
                            ),
                        );
                        sync_track_mixer_state(rt, &app, &state);
                        sync_sidebar_browser(rt, &app, track);
                        rt.run_reactive_cycle();
                        editor.refresh_runtime_side_effects();
                        fx_epoch.fetch_add(1, Ordering::Relaxed);
                        ui_epoch.fetch_add(1, Ordering::Relaxed);
                        editor.handle_host_event(HostEvent::Status(format!(
                            "Inserted MIDI FX '{}' at slot {}",
                            fx_name,
                            slot_idx + 1
                        )));
                    }
                    Err(error) => editor.handle_host_event(HostEvent::Status(format!(
                        "Error inserting MIDI FX: {error}"
                    ))),
                }
            }
        }
        "move-effect-slot" => {
            let source_track = extract_usize_from_payload(&payload, "source-track");
            let source_slot = extract_usize_from_payload(&payload, "source-slot");
            let target_track = extract_usize_from_payload(&payload, "target-track");
            let target_slot = extract_usize_from_payload(&payload, "target-slot");
            if let (Some(source_track), Some(source_slot), Some(target_track)) =
                (source_track, source_slot, target_track)
            {
                if source_track != target_track {
                    editor.handle_host_event(HostEvent::Status(
                        "Move audio effects within the same track for now".to_string(),
                    ));
                    return;
                }
                current_track.store(target_track, Ordering::Relaxed);
                app.ui.cursor_track = target_track;
                match app.apply_recorded_track_effect_chain_mutation(
                    target_track,
                    "Move audio effect",
                    |app| app.move_effect_slot_sync(
                        target_track,
                        source_slot,
                        target_slot,
                    ),
                ) {
                    Ok(slot_idx) => {
                        let rt = editor.runtime_mut();
                        set_current_track_reactive(rt, app.tracks.len(), target_track);
                        rt.set_reactive(
                            "SEQ",
                            "effects",
                            build_effects_value(
                                &state,
                                target_track,
                                &app.graph.effect_descriptors,
                                &selected_steps,
                            ),
                        );
                        rt.set_reactive(
                            "SEQ",
                            "step-has-plocks",
                            build_step_has_plocks(
                                &state,
                                target_track,
                                &app.graph.effect_descriptors,
                            ),
                        );
                        sync_track_mixer_state(rt, &app, &state);
                        rt.run_reactive_cycle();
                        editor.refresh_runtime_side_effects();
                        fx_epoch.fetch_add(1, Ordering::Relaxed);
                        ui_epoch.fetch_add(1, Ordering::Relaxed);
                        editor.handle_host_event(HostEvent::Status(format!(
                            "Moved effect to slot {}",
                            slot_idx + 1
                        )));
                    }
                    Err(error) => editor.handle_host_event(HostEvent::Status(format!(
                        "Error moving effect: {error}"
                    ))),
                }
            }
        }
        "copy-effect-values-to-all-scenes" => {
            let chain = extract_string_from_payload(&payload, "chain");
            let track = extract_usize_from_payload(&payload, "track");
            let bus_idx = extract_usize_from_payload(&payload, "bus");
            let slot_idx = extract_usize_from_payload(&payload, "slot");
            let updated = match (chain.as_deref(), track, bus_idx, slot_idx) {
                (Some("audio"), Some(track), _, Some(slot_idx)) => state
                    .copy_current_effect_values_to_all_track_patterns(track, slot_idx),
                (Some("midi"), Some(track), _, Some(slot_idx)) => state
                    .copy_current_midi_fx_values_to_all_track_patterns(track, slot_idx),
                (Some("bus"), _, Some(bus_idx), Some(slot_idx)) => {
                    app.copy_bus_effect_values_to_all_scenes(bus_idx, slot_idx)
                }
                _ => 0,
            };
            if updated > 0 {
                editor.handle_host_event(HostEvent::Status(format!(
                    "Copied effect values to {updated} patterns/scenes"
                )));
            } else {
                editor.handle_host_event(HostEvent::Status(
                    "Could not copy effect values: invalid effect target".to_string(),
                ));
            }
        }
        "move-midi-fx-slot" => {
            let source_track = extract_usize_from_payload(&payload, "source-track");
            let source_slot = extract_usize_from_payload(&payload, "source-slot");
            let target_track = extract_usize_from_payload(&payload, "target-track");
            let target_slot = extract_usize_from_payload(&payload, "target-slot");
            if let (Some(source_track), Some(source_slot), Some(target_track)) =
                (source_track, source_slot, target_track)
            {
                if source_track != target_track {
                    editor.handle_host_event(HostEvent::Status(
                        "Move MIDI effects within the same track for now".to_string(),
                    ));
                    return;
                }
                current_track.store(target_track, Ordering::Relaxed);
                app.ui.cursor_track = target_track;
                match app.apply_recorded_track_midi_fx_chain_mutation(
                    target_track,
                    "Move MIDI FX",
                    |app| app.move_midi_fx_slot_sync(
                        target_track,
                        source_slot,
                        target_slot,
                    ),
                ) {
                    Ok(slot_idx) => {
                        let rt = editor.runtime_mut();
                        set_current_track_reactive(rt, app.tracks.len(), target_track);
                        rt.set_reactive(
                            "SEQ",
                            "midi-effects",
                            build_midi_effects_value(
                                &state,
                                target_track,
                                &selected_steps,
                            ),
                        );
                        rt.set_reactive(
                            "SEQ",
                            "step-has-plocks",
                            build_step_has_plocks(
                                &state,
                                target_track,
                                &app.graph.effect_descriptors,
                            ),
                        );
                        sync_track_mixer_state(rt, &app, &state);
                        rt.run_reactive_cycle();
                        editor.refresh_runtime_side_effects();
                        fx_epoch.fetch_add(1, Ordering::Relaxed);
                        ui_epoch.fetch_add(1, Ordering::Relaxed);
                        editor.handle_host_event(HostEvent::Status(format!(
                            "Moved MIDI FX to slot {}",
                            slot_idx + 1
                        )));
                    }
                    Err(error) => editor.handle_host_event(HostEvent::Status(format!(
                        "Error moving MIDI FX: {error}"
                    ))),
                }
            }
        }
        "copy-selected-effect" => {
            let chain = extract_string_from_payload(&payload, "chain");
            let track = extract_usize_from_payload(&payload, "track");
            let bus = extract_usize_from_payload(&payload, "bus");
            let rack_slot = extract_usize_from_payload(&payload, "rack-slot");
            let slot = extract_usize_from_payload(&payload, "slot");
            let result = match (chain.as_deref(), track, bus, rack_slot, slot) {
                (Some("audio"), Some(track), _, _, Some(slot)) => {
                    app.copy_track_effect_to_clipboard(track, slot)
                }
                (Some("midi"), Some(track), _, _, Some(slot)) => {
                    app.copy_midi_effect_to_clipboard(track, slot)
                }
                (Some("bus"), _, Some(bus), _, Some(slot)) => {
                    app.copy_bus_effect_to_clipboard(bus, slot)
                }
                (Some("rack"), Some(track), _, Some(rack_slot), Some(slot)) => {
                    app.copy_rack_effect_to_clipboard(track, rack_slot, slot)
                }
                _ => Err("No effect selected to copy".to_string()),
            };
            match result {
                Ok(effect_name) => editor.handle_host_event(HostEvent::Status(format!(
                    "Copied effect '{effect_name}'"
                ))),
                Err(error) => editor.handle_host_event(HostEvent::Status(error)),
            }
        }
        "paste-effect" => {
            let track = current_track.load(Ordering::Relaxed);
            match app.paste_effect_clipboard_to_track(track) {
                Ok(message) => {
                    let rt = editor.runtime_mut();
                    rt.set_reactive(
                        "SEQ",
                        "effects",
                        build_effects_value(
                            &state,
                            track,
                            &app.graph.effect_descriptors,
                            &selected_steps,
                        ),
                    );
                    rt.set_reactive(
                        "SEQ",
                        "midi-effects",
                        build_midi_effects_value(&state, track, &selected_steps),
                    );
                    rt.set_reactive(
                        "SEQ",
                        "step-has-plocks",
                        build_step_has_plocks(
                            &state,
                            track,
                            &app.graph.effect_descriptors,
                        ),
                    );
                    rt.run_reactive_cycle();
                    editor.refresh_runtime_side_effects();
                    editor.reset_widget_scroll_for_buffer_named("*fx*");
                    fx_epoch.fetch_add(1, Ordering::Relaxed);
                    ui_epoch.fetch_add(1, Ordering::Relaxed);
                    editor.handle_host_event(HostEvent::Status(message));
                }
                Err(error) => editor.handle_host_event(HostEvent::Status(error)),
            }
        }
        "delete-effect" => {
            let slot_idx = match &payload {
                Value::Map(map) => {
                    map.get("slot").and_then(|cell| match &*cell.borrow() {
                        Value::Number(n) => Some(*n as usize),
                        _ => None,
                    })
                }
                Value::Number(n) => Some(*n as usize),
                _ => None,
            };
            let Some(slot_idx) = slot_idx else {
                editor.handle_host_event(HostEvent::Status(
                    "No effect selected".to_string(),
                ));
                return;
            };
            let track = current_track.load(Ordering::Relaxed);
            match app.apply_recorded_track_effect_chain_mutation(
                track,
                "Delete audio effect",
                |app| app.graph_controller().delete_custom_effect_slot(track, slot_idx),
            ) {
                Ok(()) => {
                    let rt = editor.runtime_mut();
                    rt.set_reactive(
                        "SEQ",
                        "effects",
                        build_effects_value(
                            &state,
                            track,
                            &app.graph.effect_descriptors,
                            &selected_steps,
                        ),
                    );
                    rt.set_reactive(
                        "SEQ",
                        "midi-effects",
                        build_midi_effects_value(&state, track, &selected_steps),
                    );
                    rt.set_reactive(
                        "SEQ",
                        "step-has-plocks",
                        build_step_has_plocks(
                            &state,
                            track,
                            &app.graph.effect_descriptors,
                        ),
                    );
                    rt.run_reactive_cycle();
                    editor.refresh_runtime_side_effects();
                    fx_epoch.fetch_add(1, Ordering::Relaxed);
                    ui_epoch.fetch_add(1, Ordering::Relaxed);
                    editor.handle_host_event(HostEvent::Status(format!(
                        "Deleted effect slot {}",
                        slot_idx + 1
                    )));
                }
                Err(e) => {
                    editor.handle_host_event(HostEvent::Status(format!(
                        "Error deleting effect: {e}"
                    )));
                }
            }
        }
        "delete-midi-fx" => {
            let slot_idx = match &payload {
                Value::Map(map) => {
                    map.get("slot").and_then(|cell| match &*cell.borrow() {
                        Value::Number(n) => Some(*n as usize),
                        _ => None,
                    })
                }
                Value::Number(n) => Some(*n as usize),
                _ => None,
            };
            let Some(slot_idx) = slot_idx else {
                editor.handle_host_event(HostEvent::Status(
                    "No MIDI FX selected".to_string(),
                ));
                return;
            };
            let track = current_track.load(Ordering::Relaxed);
            match app.apply_recorded_track_midi_fx_chain_mutation(
                track,
                "Delete MIDI FX",
                |app| app.delete_midi_fx_slot(track, slot_idx),
            ) {
                Ok(()) => {
                    let rt = editor.runtime_mut();
                    rt.set_reactive(
                        "SEQ",
                        "midi-effects",
                        build_midi_effects_value(&state, track, &selected_steps),
                    );
                    rt.set_reactive(
                        "SEQ",
                        "step-has-plocks",
                        build_step_has_plocks(
                            &state,
                            track,
                            &app.graph.effect_descriptors,
                        ),
                    );
                    rt.run_reactive_cycle();
                    editor.refresh_runtime_side_effects();
                    fx_epoch.fetch_add(1, Ordering::Relaxed);
                    ui_epoch.fetch_add(1, Ordering::Relaxed);
                    editor.handle_host_event(HostEvent::Status(format!(
                        "Deleted MIDI FX slot {}",
                        slot_idx + 1
                    )));
                }
                Err(e) => editor.handle_host_event(HostEvent::Status(format!(
                    "Error deleting MIDI FX: {e}"
                ))),
            }
        }
        _ => {}
    }
}

/// Queue the panel-tree rebuild through the normal post-event invalidation
/// pass. Directly writing `SEQ.effects` here leaves its Lisp subscribers
/// pending; while transport is stopped there may be no other reactive delta
/// to run that cycle, so controlled widgets retain their previous props.
fn queue_effect_panel_tree_invalidation(
    invalidations: &UiInvalidationQueue,
    track: Option<usize>,
    bus: Option<usize>,
) {
    if let Some(bus) = bus {
        invalidations.push(UiInvalidation::BusFx {
            bus,
            change: BusFxInvalidation::PanelTree,
        });
    } else if let Some(track) = track {
        invalidations.push(UiInvalidation::TrackFx {
            track,
            change: TrackFxInvalidation::PanelTree,
        });
    }
}

/// Rebuild the panel the active Filter Table editor session is displayed in,
/// through the same queued invalidation every other fx mutation uses — a
/// direct `SEQ.effects` write leaves the editor's controlled widgets (frame
/// counter, undo/redo enablement, dirty marker, band handle) showing stale
/// props whenever the gesture is the only reactive delta that cycle.
///
/// `target` is only needed by the close arm, which runs after the session is
/// gone; every other arm can leave it `None` and be resolved from the live
/// session.
fn refresh_filter_table_editor_panels(
    invalidations: &UiInvalidationQueue,
    target: Option<sequencer::effects::filter_table_editor::EditorTarget>,
) {
    use sequencer::effects::filter_table_editor::{session_ui_state, EditorTarget};
    let target = target.or_else(|| session_ui_state().map(|ui| ui.target));
    match target {
        Some(EditorTarget::Track { track, .. }) => {
            queue_effect_panel_tree_invalidation(invalidations, Some(track), None);
        }
        Some(EditorTarget::Bus { bus, .. }) => {
            queue_effect_panel_tree_invalidation(invalidations, None, Some(bus));
        }
        None => {}
    }
}

/// Translate a `filter-table-editor-band` payload (response-curve-editor
/// axes: freq = harmonic-bin position, q = 2/width) into a parametric node.
fn parametric_node_from_band_payload(
    payload: &Value,
) -> Result<sequencer::effects::filter_table_editor::ParametricNode, String> {
    use sequencer::effects::filter_table::REFERENCE_HARMONIC;
    use sequencer::effects::filter_table_editor::ParametricKind;
    let kind = extract_string_from_payload(payload, "kind")
        .and_then(|tag| ParametricKind::from_tag(&tag))
        .ok_or_else(|| "unknown node kind".to_string())?;
    let freq = extract_f32_from_payload(payload, "freq")
        .ok_or_else(|| "need freq".to_string())?
        .clamp(1.0, 1024.0);
    let gain_db = extract_f32_from_payload(payload, "gain")
        .ok_or_else(|| "need gain".to_string())?;
    let q = extract_f32_from_payload(payload, "q")
        .unwrap_or(2.0)
        .clamp(0.25, 16.0);
    Ok(sequencer::effects::filter_table_editor::ParametricNode {
        kind,
        center_oct: (freq / REFERENCE_HARMONIC as f32).log2(),
        width_oct: 2.0 / q,
        gain_db,
    })
}

/// Translate a `filter-table-editor-op` payload into an [`EditOp`]. Ops act
/// on the session's selected frame (frame ops) or on an explicit/implied
/// frame range (`:frame-start`/`:frame-end`, defaulting to every frame).
fn filter_table_editor_op_from_payload(
    payload: &Value,
) -> Result<sequencer::effects::filter_table_editor::EditOp, String> {
    use sequencer::effects::filter_table_editor::{session_ui_state, DrawPoint, EditOp};
    let ui = session_ui_state().ok_or_else(|| "no Filter Table editor open".to_string())?;
    let kind = extract_string_from_payload(payload, "kind")
        .ok_or_else(|| "need an op kind".to_string())?;
    let selected = ui.selected_frame;
    let frame_start = extract_usize_from_payload(payload, "frame-start").unwrap_or(0);
    let frame_end =
        extract_usize_from_payload(payload, "frame-end").unwrap_or(ui.frames.saturating_sub(1));
    let value = extract_f32_from_payload(payload, "value");
    Ok(match kind.as_str() {
        "smooth-spectral" => EditOp::SmoothSpectral {
            frame_start,
            frame_end,
            radius: extract_usize_from_payload(payload, "radius").unwrap_or(4),
        },
        "smooth-temporal" => EditOp::SmoothTemporal {
            frame_start,
            frame_end,
            radius: extract_usize_from_payload(payload, "radius").unwrap_or(2),
        },
        "normalize" => EditOp::Normalize {
            frame_start,
            frame_end,
        },
        "tilt" => EditOp::Tilt {
            frame_start,
            frame_end,
            db_per_octave: value.unwrap_or(3.0),
        },
        "shift" => EditOp::ShiftOctaves {
            frame_start,
            frame_end,
            octaves: value.unwrap_or(0.5),
        },
        "stretch" => EditOp::StretchOctaves {
            frame_start,
            frame_end,
            factor: value.unwrap_or(1.25),
        },
        "insert-frame" => EditOp::InsertFrame { at: selected },
        "duplicate-frame" => EditOp::DuplicateFrame { at: selected },
        "delete-frame" => EditOp::DeleteFrame { at: selected },
        "move-frame" => EditOp::MoveFrame {
            from: selected,
            to: extract_usize_from_payload(payload, "to")
                .ok_or_else(|| "need a destination frame".to_string())?,
        },
        "interpolate" => EditOp::InterpolateFrames {
            start: extract_usize_from_payload(payload, "start").unwrap_or(0),
            end: extract_usize_from_payload(payload, "end")
                .unwrap_or(ui.frames.saturating_sub(1)),
        },
        "draw" => {
            let points_json = extract_string_from_payload(payload, "points-json")
                .ok_or_else(|| "draw needs points-json".to_string())?;
            let points: Vec<DrawPoint> = serde_json::from_str(&points_json)
                .map_err(|error| format!("draw points are malformed: {error}"))?;
            EditOp::Draw {
                frame: extract_usize_from_payload(payload, "frame").unwrap_or(selected),
                points,
            }
        }
        other => return Err(format!("unknown Filter Table editor op '{other}'")),
    })
}
