use crate::*;

pub(super) const COMMANDS: &[&str] = &[
    "set-track-output",
    "set-mod-route",
    "delete-mod-route",
    "refresh-mixer-ui",
    "set-track-bus-send",
    "set-bus-effect-param",
    "set-bus-effect-plock",
    "set-bus-effect-param-option",
    "set-bus-effect-plock-option",
    "add-bus-effect",
    "add-builtin-bus-effect",
    "insert-builtin-bus-effect-before-slot",
    "insert-bus-effect-before-slot",
    "move-bus-effect-slot",
    "delete-bus-effect",
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
    let ui_epoch = ctx.shared.ui_epoch.clone();
    let fx_epoch = ctx.shared.fx_epoch.clone();
    let ui_invalidations = ctx.shared.ui_invalidations.clone();
    let bus_state = ctx.shared.bus_state.clone();
    match name {
        "set-track-output" => {
            if let Value::Map(ref map) = payload {
                let label = map.get("label").and_then(|cell| match &*cell.borrow() {
                    Value::String(s) => Some(s.clone()),
                    _ => None,
                });
                let payload_track =
                    map.get("track").and_then(|cell| match &*cell.borrow() {
                        Value::Number(n) => Some(*n as usize),
                        _ => None,
                    });
                if let Some(label) = label {
                    let track = payload_track
                        .unwrap_or_else(|| current_track.load(Ordering::Relaxed));
                    let output = if label == "main" {
                        Some(TrackOutput::Mix)
                    } else if label == "sends only" {
                        Some(TrackOutput::None)
                    } else {
                        app.buses
                            .iter()
                            .filter(|bus| bus.id != sequencer::sequencer::BusId::MIX)
                            .find(|bus| bus.name == label)
                            .map(|bus| TrackOutput::Bus(bus.id))
                    };
                    if let Some(output) = output {
                        app::apply_command(
                            &mut app,
                            app::AppCommand::SetTrackOutput { track, output },
                        );
                        let rt = editor.runtime_mut();
                        sync_track_mixer_state(rt, &app, &state);
                        if track == current_track.load(Ordering::Relaxed) {
                            let selected_neural_snapshot =
                                selected_neural_neurons.lock().unwrap().clone();
                            sync_track_params_with_neural_selection(
                                rt,
                                &app,
                                &state,
                                track,
                                &selected_steps,
                                Some(&selected_neural_snapshot),
                            );
                            sync_fx_param_binding_fields_with_neural_selection(
                                rt,
                                &app,
                                &state,
                                track,
                                &selected_steps,
                                Some(&selected_neural_snapshot),
                            );
                        }
                        rt.run_reactive_cycle();
                        editor.refresh_runtime_side_effects();
                        ui_epoch.fetch_add(1, Ordering::Relaxed);
                    }
                }
            }
        }
        "set-mod-route" => {
            if let Value::Map(ref map) = payload {
                let source = map.get("source").and_then(|cell| match &*cell.borrow() {
                    Value::Number(n) => Some(*n as usize),
                    _ => None,
                });
                let dest = map.get("dest").and_then(|cell| match &*cell.borrow() {
                    Value::Number(n) => Some(*n as usize),
                    _ => None,
                });
                let destination =
                    match map.get("dest-kind").and_then(|cell| match &*cell.borrow() {
                        Value::String(kind) => Some(kind.clone()),
                        _ => None,
                    }) {
                        Some(kind) if kind == "bus" => dest.map(|id| {
                            sequencer::sequencer::ModDestination::Bus(
                                sequencer::sequencer::BusId(id as u64),
                            )
                        }),
                        _ => dest.map(sequencer::sequencer::ModDestination::Track),
                    };
                let input = map
                    .get("input")
                    .and_then(|cell| match &*cell.borrow() {
                        Value::Number(n) => Some(*n as usize),
                        _ => None,
                    })
                    .unwrap_or(0);
                if let (Some(source), Some(destination)) = (source, destination) {
                    match app.apply_recorded_scene_structure_mutation(
                        "Connect modulation route",
                        |app| app.graph_controller().set_mod_route_to_destination(
                            source,
                            destination,
                            input,
                        ),
                    ) {
                        Ok(()) => {
                            let dest_label =
                                mod_route_destination_status_label(&app, destination);
                            let message = format!(
                                "Connected mod route: track {} out -> {} Ext{}",
                                source + 1,
                                dest_label,
                                input + 1
                            );
                            eprintln!("[mod-route] {message}");
                            let rt = editor.runtime_mut();
                            sync_track_mixer_state(rt, &app, &state);
                            rt.run_reactive_cycle();
                            editor.refresh_runtime_side_effects();
                            ui_epoch.fetch_add(1, Ordering::Relaxed);
                            editor.handle_host_event(HostEvent::Status(message));
                        }
                        Err(error) => {
                            eprintln!(
                                "[mod-route] rejected connect {} -> {:?}: {}",
                                source + 1,
                                destination,
                                error
                            );
                            editor.handle_host_event(HostEvent::Status(error));
                        }
                    }
                }
            }
        }
        "delete-mod-route" => {
            if let Value::Map(ref map) = payload {
                let source = map.get("source").and_then(|cell| match &*cell.borrow() {
                    Value::Number(n) => Some(*n as usize),
                    _ => None,
                });
                let dest = map.get("dest").and_then(|cell| match &*cell.borrow() {
                    Value::Number(n) => Some(*n as usize),
                    _ => None,
                });
                let destination =
                    match map.get("dest-kind").and_then(|cell| match &*cell.borrow() {
                        Value::String(kind) => Some(kind.clone()),
                        _ => None,
                    }) {
                        Some(kind) if kind == "bus" => dest.map(|id| {
                            sequencer::sequencer::ModDestination::Bus(
                                sequencer::sequencer::BusId(id as u64),
                            )
                        }),
                        _ => dest.map(sequencer::sequencer::ModDestination::Track),
                    };
                let input = map
                    .get("input")
                    .and_then(|cell| match &*cell.borrow() {
                        Value::Number(n) => Some(*n as usize),
                        _ => None,
                    })
                    .unwrap_or(0);
                if let (Some(source), Some(destination)) = (source, destination) {
                    match app.apply_recorded_scene_structure_mutation(
                        "Delete modulation route",
                        |app| app.graph_controller().delete_mod_route_to_destination(
                            source,
                            destination,
                            input,
                        ),
                    ) {
                        Ok(()) => {
                            let dest_label =
                                mod_route_destination_status_label(&app, destination);
                            let message = format!(
                                "Disconnected mod route: track {} out -> {} Ext{}",
                                source + 1,
                                dest_label,
                                input + 1
                            );
                            eprintln!("[mod-route] {message}");
                            let rt = editor.runtime_mut();
                            sync_track_mixer_state(rt, &app, &state);
                            rt.run_reactive_cycle();
                            editor.refresh_runtime_side_effects();
                            ui_epoch.fetch_add(1, Ordering::Relaxed);
                            editor.handle_host_event(HostEvent::Status(message));
                        }
                        Err(error) => {
                            eprintln!(
                                "[mod-route] rejected disconnect {} -> {:?}: {}",
                                source + 1,
                                destination,
                                error
                            );
                            editor.handle_host_event(HostEvent::Status(error));
                        }
                    }
                }
            }
        }
        "refresh-mixer-ui" => {
            let rt = editor.runtime_mut();
            sync_track_mixer_state(rt, &app, &state);
            rt.run_reactive_cycle();
            editor.refresh_runtime_side_effects();
            refresh_visible_mixer_layouts(editor);
            ui_epoch.fetch_add(1, Ordering::Relaxed);
        }
        "set-track-bus-send" => {
            if let Value::Map(ref map) = payload {
                let bus_idx = map.get("bus").and_then(|cell| match &*cell.borrow() {
                    Value::Number(n) => Some(*n as usize),
                    _ => None,
                });
                let amount = map.get("amount").and_then(|cell| match &*cell.borrow() {
                    Value::Number(n) => Some(*n as f32),
                    _ => None,
                });
                let payload_track =
                    map.get("track").and_then(|cell| match &*cell.borrow() {
                        Value::Number(n) => Some(*n as usize),
                        _ => None,
                    });
                if let (Some(bus_idx), Some(amount)) = (bus_idx, amount) {
                    let Some(bus_id) = app.buses.get(bus_idx).map(|bus| bus.id) else {
                        return;
                    };
                    if bus_id == sequencer::sequencer::BusId::MIX {
                        return;
                    }
                    let track = payload_track
                        .unwrap_or_else(|| current_track.load(Ordering::Relaxed));
                    if track >= state.active_track_count() {
                        return;
                    }
                    let selected: Vec<usize> = selected_steps.lock().unwrap()
                        .iter()
                        .copied()
                        .collect();
                    let has_selection = !selected.is_empty();
                    if !has_selection {
                        let mut sends = app.state.pattern.track_params[track].sends();
                        if let Some(send) =
                            sends.iter_mut().find(|send| send.destination == bus_id)
                        {
                            send.amount = amount;
                        } else {
                            sends.push(TrackSendSnapshot {
                                destination: bus_id,
                                amount,
                            });
                        }
                        app::apply_command(
                            &mut app,
                            app::AppCommand::SetTrackSends { track, sends },
                        );
                    } else {
                        // A zero baseline still needs a persistent graph edge so the
                        // realtime scheduler can address this destination at a lock.
                        let mut sends = app.state.pattern.track_params[track].sends();
                        if !sends.iter().any(|send| send.destination == bus_id) {
                            sends.push(TrackSendSnapshot {
                                destination: bus_id,
                                amount: 0.0,
                            });
                            app::apply_command(
                                &mut app,
                                app::AppCommand::SetTrackSends { track, sends },
                            );
                        }
                        for step in &selected {
                            app::apply_command(
                                &mut app,
                                app::AppCommand::SetTrackBusSendPlock {
                                    track,
                                    step: *step,
                                    destination: bus_id,
                                    value: Some(amount),
                                },
                            );
                        }
                        ui_invalidations.push(UiInvalidation::StepBatch {
                            track,
                            steps: selected.clone(),
                        });
                        fx_epoch.fetch_add(1, Ordering::Relaxed);
                    }
                    let rt = editor.runtime_mut();
                    let current = current_track.load(Ordering::Relaxed);
                    if has_selection {
                        // The persisted baseline intentionally did not change. Publish
                        // the edited lock value instead of immediately snapping both
                        // controls back to that baseline.
                        rt.set_reactive(
                            "SEQ",
                            &track_bus_send_field(track, bus_idx),
                            Value::Number(amount as f64),
                        );
                        if track == current {
                            rt.set_reactive(
                                "SEQ",
                                &current_track_bus_send_field(bus_idx),
                                Value::Number(amount as f64),
                            );
                        }
                    } else {
                        sync_track_bus_send_binding_field(rt, &app, &state, track, bus_idx);
                        if track == current {
                            sync_current_track_bus_send_binding_field(
                                rt, &app, &state, track, bus_idx,
                            );
                        }
                    }
                    rt.run_reactive_cycle();
                    editor.refresh_runtime_side_effects();
                }
            }
        }
        "set-bus-effect-param" => {
            if let Value::Map(ref map) = payload {
                let bus_idx = map.get("bus").and_then(|cell| match &*cell.borrow() {
                    Value::Number(n) => Some(*n as usize),
                    _ => None,
                });
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
                if let (Some(bus_idx), Some(slot_idx), Some(param_idx), Some(value)) =
                    (bus_idx, slot_idx, param_idx, value)
                {
                    let desc = app
                        .buses
                        .get(bus_idx)
                        .and_then(|bus| bus.effect_descriptors.get(slot_idx))
                        .and_then(|desc| desc.params.get(param_idx))
                        .cloned();
                    let stored = desc.as_ref().map(|param| param.clamp(value)).unwrap_or(value);
                    let printable = desc.as_ref().is_some_and(|param| {
                        !matches!(
                            param.host_control,
                            Some(sequencer::effects::HostControl::FxSidechain { .. })
                        ) && !sequencer::instruments::voice_modulator::is_envelope_source_param_value(
                            param.node_param_idx,
                            stored,
                        )
                    });
                    let track = current_track.load(Ordering::Relaxed);
                    let print_gesture = printable
                        && try_latch_param_print(
                            ctx.shared,
                            &mut *editor,
                            &app,
                            track,
                            &[(PrintTarget::BusEffect {
                                bus_idx,
                                slot_idx,
                                param_idx,
                            }, stored)],
                        );
                    if !print_gesture {
                        match app.apply_recorded_bus_effect_value_mutation(
                            bus_idx,
                            slot_idx,
                            "Set bus effect parameter",
                            format!("param:{param_idx}"),
                            |app| app.set_bus_effect_param(
                                bus_idx, slot_idx, param_idx, stored,
                            ),
                        ) {
                            Ok(()) => {
                                app.publish_bus_effect_runtime();
                                *bus_state.lock().unwrap() = app.buses.clone();
                                sync_bus_effect_param_value_field(
                                    editor.runtime_mut(),
                                    &app,
                                    bus_idx,
                                    slot_idx,
                                    param_idx,
                                );
                                if desc.as_ref().is_some_and(param_change_needs_fx_rebuild)
                                {
                                    fx_epoch.fetch_add(1, Ordering::Relaxed);
                                    ui_epoch.fetch_add(1, Ordering::Relaxed);
                                }
                            }
                            Err(error) => editor.handle_host_event(HostEvent::Status(
                                format!("Error setting bus effect param: {error}"),
                            )),
                        }
                    }
                }
            }
        }
        "set-bus-effect-plock" => {
            if let Value::Map(ref map) = payload {
                let bus_idx = map.get("bus").and_then(|cell| match &*cell.borrow() {
                    Value::Number(n) => Some(*n as usize),
                    _ => None,
                });
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
                if let (Some(bus_idx), Some(slot_idx), Some(param_idx), Some(value)) =
                    (bus_idx, slot_idx, param_idx, value)
                {
                    let steps: Vec<usize> =
                        selected_steps.lock().unwrap().iter().copied().collect();
                    let result = app.apply_recorded_bus_effect_value_mutation(
                        bus_idx,
                        slot_idx,
                        "Set bus effect p-lock",
                        format!("plock:param:{param_idx}"),
                        |app| {
                            for step in steps {
                                app.set_bus_effect_plock(
                                    bus_idx, slot_idx, step, param_idx, value,
                                )?;
                            }
                            Ok(())
                        },
                    );
                    match result {
                        Ok(()) => {
                            app.publish_bus_effect_runtime();
                            *bus_state.lock().unwrap() = app.buses.clone();
                            let rt = editor.runtime_mut();
                            sync_bus_mixer_state(rt, &app);
                            rt.run_reactive_cycle();
                            editor.refresh_runtime_side_effects();
                            fx_epoch.fetch_add(1, Ordering::Relaxed);
                            ui_epoch.fetch_add(1, Ordering::Relaxed);
                        }
                        Err(error) => editor.handle_host_event(HostEvent::Status(
                            format!("Error setting bus effect p-lock: {error}"),
                        )),
                    }
                }
            }
        }
        "set-bus-effect-param-option" => {
            if let Value::Map(ref map) = payload {
                let bus_idx = map.get("bus").and_then(|cell| match &*cell.borrow() {
                    Value::Number(n) => Some(*n as usize),
                    _ => None,
                });
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
                if let (Some(bus_idx), Some(slot_idx), Some(param_idx), Some(label)) =
                    (bus_idx, slot_idx, param_idx, label)
                {
                    if let Some(selected_idx) = app.bus_effect_param_option_index(
                        bus_idx, slot_idx, param_idx, &label,
                    ) {
                        let is_host_sidechain = matches!(
                            app.buses
                                .get(bus_idx)
                                .and_then(|bus| bus.effect_descriptors.get(slot_idx))
                                .and_then(|desc| desc.params.get(param_idx))
                                .and_then(|param| param.host_control.as_ref()),
                            Some(sequencer::effects::HostControl::FxSidechain { .. })
                        );
                        let value = selected_idx as f32;
                        let printable = !is_host_sidechain
                            && app.buses
                                .get(bus_idx)
                                .and_then(|bus| bus.effect_descriptors.get(slot_idx))
                                .and_then(|desc| desc.params.get(param_idx))
                                .is_some_and(|param| {
                                    !sequencer::instruments::voice_modulator::is_envelope_source_param_value(
                                        param.node_param_idx,
                                        value,
                                    )
                                });
                        let track = current_track.load(Ordering::Relaxed);
                        let print_gesture = printable
                            && try_latch_param_print(
                                ctx.shared,
                                &mut *editor,
                                &app,
                                track,
                                &[(PrintTarget::BusEffect {
                                    bus_idx,
                                    slot_idx,
                                    param_idx,
                                }, value)],
                            );
                        if !print_gesture {
                            match app.apply_recorded_bus_effect_value_mutation(
                                bus_idx,
                                slot_idx,
                                "Set bus effect option",
                                format!("param:{param_idx}"),
                                |app| {
                                    if is_host_sidechain {
                                        app.apply_bus_effect_sidechain_selection(
                                            bus_idx, slot_idx, param_idx, selected_idx,
                                        );
                                    }
                                    app.set_bus_effect_param(
                                        bus_idx, slot_idx, param_idx, value,
                                    )
                                },
                            ) {
                                Ok(()) => {
                                    app.publish_bus_effect_runtime();
                                    *bus_state.lock().unwrap() = app.buses.clone();
                                    let rt = editor.runtime_mut();
                                    sync_bus_mixer_state(rt, &app);
                                    rt.set_reactive(
                                        "SEQ",
                                        "bus-effects",
                                        build_bus_effects_value_for_selection(
                                            &app,
                                            Some(&selected_steps),
                                        ),
                                    );
                                    rt.run_reactive_cycle();
                                    editor.refresh_runtime_side_effects();
                                    fx_epoch.fetch_add(1, Ordering::Relaxed);
                                    ui_epoch.fetch_add(1, Ordering::Relaxed);
                                }
                                Err(error) => editor.handle_host_event(HostEvent::Status(
                                    format!("Error setting bus effect option: {error}"),
                                )),
                            }
                        }
                    }
                }
            }
        }
        "set-bus-effect-plock-option" => {
            if let Value::Map(ref map) = payload {
                let bus_idx = map.get("bus").and_then(|cell| match &*cell.borrow() {
                    Value::Number(n) => Some(*n as usize),
                    _ => None,
                });
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
                if let (Some(bus_idx), Some(slot_idx), Some(param_idx), Some(label)) =
                    (bus_idx, slot_idx, param_idx, label)
                {
                    if let Some(selected_idx) = app.bus_effect_param_option_index(
                        bus_idx, slot_idx, param_idx, &label,
                    ) {
                        let steps: Vec<usize> =
                            selected_steps.lock().unwrap().iter().copied().collect();
                        let result = app.apply_recorded_bus_effect_value_mutation(
                            bus_idx,
                            slot_idx,
                            "Set bus effect p-lock option",
                            format!("plock:param:{param_idx}"),
                            |app| {
                                for step in steps {
                                    app.set_bus_effect_plock(
                                        bus_idx,
                                        slot_idx,
                                        step,
                                        param_idx,
                                        selected_idx as f32,
                                    )?;
                                }
                                Ok(())
                            },
                        );
                        match result {
                            Ok(()) => {
                                app.publish_bus_effect_runtime();
                                *bus_state.lock().unwrap() = app.buses.clone();
                                let rt = editor.runtime_mut();
                                sync_bus_mixer_state(rt, &app);
                                rt.run_reactive_cycle();
                                editor.refresh_runtime_side_effects();
                                fx_epoch.fetch_add(1, Ordering::Relaxed);
                                ui_epoch.fetch_add(1, Ordering::Relaxed);
                            }
                            Err(error) => {
                                editor.handle_host_event(HostEvent::Status(format!(
                                    "Error setting bus effect p-lock option: {error}"
                                )))
                            }
                        }
                    }
                }
            }
        }
        "add-bus-effect" => {
            if let Value::Map(ref map) = payload {
                let bus_idx = map.get("bus").and_then(|cell| match &*cell.borrow() {
                    Value::Number(n) => Some(*n as usize),
                    _ => None,
                });
                let effect_name =
                    map.get("name").and_then(|cell| match &*cell.borrow() {
                        Value::String(s) => Some(s.clone()),
                        _ => None,
                    });
                if let (Some(bus_idx), Some(effect_name)) = (bus_idx, effect_name) {
                    match app.apply_recorded_bus_effect_chain_mutation(
                        bus_idx,
                        "Add bus effect",
                        |app| app.add_bus_effect_sync(bus_idx, &effect_name),
                    ) {
                        Ok(slot_idx) => {
                            app.publish_bus_effect_runtime();
                            *bus_state.lock().unwrap() = app.buses.clone();
                            let rt = editor.runtime_mut();
                            sync_bus_mixer_state(rt, &app);
                            rt.run_reactive_cycle();
                            editor.refresh_runtime_side_effects();
                            editor.reset_widget_scroll_for_buffer_named("*fx*");
                            let fx_render_status =
                                editor.runtime_mut().take_status_message();
                            fx_epoch.fetch_add(1, Ordering::Relaxed);
                            ui_epoch.fetch_add(1, Ordering::Relaxed);
                            if let Some(status) = fx_render_status {
                                editor.handle_host_event(HostEvent::Status(format!(
                                    "FX UI error after adding bus effect: {status}"
                                )));
                            } else {
                                editor.handle_host_event(HostEvent::Status(format!(
                                    "Added bus effect '{}' to slot {}",
                                    effect_name,
                                    slot_idx + 1
                                )));
                            }
                        }
                        Err(error) => editor.handle_host_event(HostEvent::Status(
                            format!("Error adding bus effect: {error}"),
                        )),
                    }
                }
            }
        }
        "add-builtin-bus-effect" => {
            if let Value::Map(ref map) = payload {
                let bus_idx = map.get("bus").and_then(|cell| match &*cell.borrow() {
                    Value::Number(n) => Some(*n as usize),
                    _ => None,
                });
                let effect_name =
                    map.get("name").and_then(|cell| match &*cell.borrow() {
                        Value::String(s) => Some(s.clone()),
                        _ => None,
                    });
                if let (Some(bus_idx), Some(effect_name)) = (bus_idx, effect_name) {
                    match app.apply_recorded_bus_effect_chain_mutation(
                        bus_idx,
                        "Add bus effect",
                        |app| app.add_builtin_bus_effect_sync(bus_idx, &effect_name),
                    ) {
                        Ok(slot_idx) => {
                            app.publish_bus_effect_runtime();
                            *bus_state.lock().unwrap() = app.buses.clone();
                            let rt = editor.runtime_mut();
                            sync_bus_mixer_state(rt, &app);
                            rt.run_reactive_cycle();
                            editor.refresh_runtime_side_effects();
                            editor.reset_widget_scroll_for_buffer_named("*fx*");
                            fx_epoch.fetch_add(1, Ordering::Relaxed);
                            ui_epoch.fetch_add(1, Ordering::Relaxed);
                            editor.handle_host_event(HostEvent::Status(format!(
                                "Added built-in bus effect '{}' to slot {}",
                                effect_name,
                                slot_idx + 1
                            )));
                        }
                        Err(error) => editor.handle_host_event(HostEvent::Status(
                            format!("Error adding built-in bus effect: {error}"),
                        )),
                    }
                }
            }
        }
        "insert-builtin-bus-effect-before-slot" => {
            let bus_idx = extract_usize_from_payload(&payload, "bus");
            let slot = extract_usize_from_payload(&payload, "slot");
            let effect_name = extract_string_from_payload(&payload, "name");
            if let (Some(bus_idx), Some(slot), Some(effect_name)) =
                (bus_idx, slot, effect_name)
            {
                match app.apply_recorded_bus_effect_chain_mutation(
                    bus_idx,
                    "Insert bus effect",
                    |app| app.insert_builtin_bus_effect_before_slot_sync(
                        bus_idx,
                        slot,
                        &effect_name,
                    ),
                ) {
                    Ok(slot_idx) => {
                        app.publish_bus_effect_runtime();
                        *bus_state.lock().unwrap() = app.buses.clone();
                        let rt = editor.runtime_mut();
                        sync_bus_mixer_state(rt, &app);
                        rt.run_reactive_cycle();
                        editor.refresh_runtime_side_effects();
                        fx_epoch.fetch_add(1, Ordering::Relaxed);
                        ui_epoch.fetch_add(1, Ordering::Relaxed);
                        editor.handle_host_event(HostEvent::Status(format!(
                            "Inserted built-in bus effect '{}' at slot {}",
                            effect_name,
                            slot_idx + 1
                        )));
                    }
                    Err(error) => editor.handle_host_event(HostEvent::Status(format!(
                        "Error inserting built-in bus effect: {error}"
                    ))),
                }
            }
        }
        "insert-bus-effect-before-slot" => {
            let bus_idx = extract_usize_from_payload(&payload, "bus");
            let slot = extract_usize_from_payload(&payload, "slot");
            let effect_name = extract_string_from_payload(&payload, "name");
            if let (Some(bus_idx), Some(slot), Some(effect_name)) =
                (bus_idx, slot, effect_name)
            {
                match app.apply_recorded_bus_effect_chain_mutation(
                    bus_idx,
                    "Insert bus effect",
                    |app| app.insert_bus_effect_before_slot_sync(
                        bus_idx,
                        slot,
                        &effect_name,
                    ),
                ) {
                    Ok(slot_idx) => {
                        app.publish_bus_effect_runtime();
                        *bus_state.lock().unwrap() = app.buses.clone();
                        let rt = editor.runtime_mut();
                        sync_bus_mixer_state(rt, &app);
                        rt.run_reactive_cycle();
                        editor.refresh_runtime_side_effects();
                        fx_epoch.fetch_add(1, Ordering::Relaxed);
                        ui_epoch.fetch_add(1, Ordering::Relaxed);
                        editor.handle_host_event(HostEvent::Status(format!(
                            "Inserted bus effect '{}' at slot {}",
                            effect_name,
                            slot_idx + 1
                        )));
                    }
                    Err(error) => editor.handle_host_event(HostEvent::Status(format!(
                        "Error inserting bus effect: {error}"
                    ))),
                }
            }
        }
        "move-bus-effect-slot" => {
            let bus_idx = extract_usize_from_payload(&payload, "bus");
            let source_slot = extract_usize_from_payload(&payload, "source-slot");
            let target_slot = extract_usize_from_payload(&payload, "target-slot");
            if let (Some(bus_idx), Some(source_slot)) = (bus_idx, source_slot) {
                match app.apply_recorded_bus_effect_chain_mutation(
                    bus_idx,
                    "Move bus effect",
                    |app| app.move_bus_effect_slot_sync(bus_idx, source_slot, target_slot),
                ) {
                    Ok(slot_idx) => {
                        app.publish_bus_effect_runtime();
                        *bus_state.lock().unwrap() = app.buses.clone();
                        let rt = editor.runtime_mut();
                        sync_bus_mixer_state(rt, &app);
                        rt.run_reactive_cycle();
                        editor.refresh_runtime_side_effects();
                        fx_epoch.fetch_add(1, Ordering::Relaxed);
                        ui_epoch.fetch_add(1, Ordering::Relaxed);
                        editor.handle_host_event(HostEvent::Status(format!(
                            "Moved bus effect to slot {}",
                            slot_idx + 1
                        )));
                    }
                    Err(error) => editor.handle_host_event(HostEvent::Status(format!(
                        "Error moving bus effect: {error}"
                    ))),
                }
            }
        }
        "delete-bus-effect" => {
            let bus_idx = match &payload {
                Value::Map(map) => {
                    map.get("bus").and_then(|cell| match &*cell.borrow() {
                        Value::Number(n) => Some(*n as usize),
                        _ => None,
                    })
                }
                _ => None,
            };
            let slot_idx = match &payload {
                Value::Map(map) => {
                    map.get("slot").and_then(|cell| match &*cell.borrow() {
                        Value::Number(n) => Some(*n as usize),
                        _ => None,
                    })
                }
                _ => None,
            };
            if let (Some(bus_idx), Some(slot_idx)) = (bus_idx, slot_idx) {
                match app.apply_recorded_bus_effect_chain_mutation(
                    bus_idx,
                    "Delete bus effect",
                    |app| app.delete_bus_effect_slot(bus_idx, slot_idx),
                ) {
                    Ok(()) => {
                        app.publish_bus_effect_runtime();
                        *bus_state.lock().unwrap() = app.buses.clone();
                        let rt = editor.runtime_mut();
                        sync_bus_mixer_state(rt, &app);
                        rt.run_reactive_cycle();
                        editor.refresh_runtime_side_effects();
                        fx_epoch.fetch_add(1, Ordering::Relaxed);
                        ui_epoch.fetch_add(1, Ordering::Relaxed);
                        editor.handle_host_event(HostEvent::Status(format!(
                            "Deleted bus effect slot {}",
                            slot_idx + 1
                        )));
                    }
                    Err(error) => editor.handle_host_event(HostEvent::Status(format!(
                        "Error deleting bus effect: {error}"
                    ))),
                }
            }
        }
        _ => {}
    }
}
