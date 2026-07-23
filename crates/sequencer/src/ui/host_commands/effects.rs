use crate::*;

pub(super) const COMMANDS: &[&str] = &[
    "set-convolution-reverb-ir",
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
                                            app.publish_bus_gate_runtime();
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
                                        app.publish_bus_gate_runtime();
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
