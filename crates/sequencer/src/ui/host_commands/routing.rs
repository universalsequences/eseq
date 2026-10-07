use crate::*;

pub(super) const COMMANDS: &[&str] = &[
    "set-track-output",
    "add-bus",
    "set-bus-output",
    "set-mod-route",
    "delete-mod-route",
    "refresh-mixer-ui",
    "set-track-bus-send",
    "set-track-send-base",
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

/// Set `track`'s own send level to `bus` (adding the send when missing).
fn set_track_send_base(
    app: &mut app::App,
    track: usize,
    bus: sequencer::sequencer::BusId,
    amount: f32,
) {
    let mut sends = app.state.pattern.track_params[track].sends();
    if let Some(send) = sends.iter_mut().find(|send| send.destination == bus) {
        send.amount = amount;
    } else {
        sends.push(TrackSendSnapshot {
            destination: bus,
            amount,
        });
    }
    app::apply_command(app, app::AppCommand::SetTrackSends { track, sends });
}

/// After a bus effect's param value landed (`set-bus-effect-param`, the
/// host kinds' `param.base` of a bus effect): publish the bus runtime and
/// the shared bus copy the natives and the host kinds read, refresh the
/// legacy value field, and rebuild the panels when the param redefines them
/// (an enum, a boolean).
pub(super) fn bus_effect_param_applied(
    app: &mut app::App,
    editor: &mut Editor,
    shared: &SharedHandles,
    (bus, slot, param): (usize, usize, usize),
    pdesc: Option<&sequencer::effects::ParamDescriptor>,
) {
    app.publish_bus_effect_runtime();
    *shared.bus_state.lock().unwrap() = app.buses.clone();
    sync_bus_effect_param_value_field(editor.runtime_mut(), app, bus, slot, param);
    if let Some(pdesc) = pdesc {
        super::rebuild_panel_if_needed(shared, pdesc);
    }
}

/// After a track's output changed (`set-track-output`, the host kinds'
/// `track.output`): refresh the mixer, and the track panel when it is the
/// current track.
pub(super) fn track_output_applied(
    app: &app::App,
    editor: &mut Editor,
    ctx: &LoopCtx<'_>,
    track: usize,
) {
    let shared = ctx.shared;
    let rt = editor.runtime_mut();
    sync_track_mixer_state(rt, app, &shared.state);
    if track == shared.current_track.load(Ordering::Relaxed) {
        let selected_neural_snapshot = shared.selected_neural_neurons.lock().unwrap().clone();
        let selected_steps = &shared.selected_steps;
        let neural = Some(&selected_neural_snapshot);
        sync_track_params_with_neural_selection(
            rt,
            app,
            &shared.state,
            track,
            selected_steps,
            neural,
        );
        sync_fx_param_binding_fields_with_neural_selection(
            rt,
            app,
            &shared.state,
            track,
            selected_steps,
            neural,
        );
    }
    rt.run_reactive_cycle();
    editor.refresh_runtime_side_effects();
    shared.ui_epoch.fetch_add(1, Ordering::Relaxed);
}

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
    let ui_epoch = ctx.shared.ui_epoch.clone();
    let fx_epoch = ctx.shared.fx_epoch.clone();
    let ui_invalidations = ctx.shared.ui_invalidations.clone();
    let bus_state = ctx.shared.bus_state.clone();
    match name {
        "add-bus" | "set-bus-output" => {
            match apply_bus_routing_command(name, &payload, app) {
                Ok(()) => {
                    *bus_state.lock().unwrap() = app.buses.clone();
                    *ctx.shared.bus_node_ids.lock().unwrap() = app.graph.bus_node_ids.clone();
                    let rt = editor.runtime_mut();
                    sync_bus_mixer_state(rt, app);
                    sync_track_mixer_state(rt, app, &state);
                    rt.run_reactive_cycle();
                    editor.refresh_runtime_side_effects();
                    ui_epoch.fetch_add(1, Ordering::Relaxed);
                }
                Err(error) => app.editor.status_message = Some((error, Instant::now())),
            }
        }
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
                // By label (the output dropdown) or by `:bus-id`.
                let output = match (label, map_usize(map, "bus-id")) {
                    (Some(label), _) => track_output_named(app, &label),
                    (None, Some(bus)) => {
                        track_output_for_bus(app, Some(sequencer::sequencer::BusId(bus as u64)))
                    }
                    (None, None) => None,
                };
                if let Some(output) = output {
                    let track =
                        payload_track.unwrap_or_else(|| current_track.load(Ordering::Relaxed));
                    app::apply_command(&mut app, app::AppCommand::SetTrackOutput { track, output });
                    track_output_applied(app, editor, ctx, track);
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
                    // The step selection belongs to the current track: a
                    // send of another track sets its own level.
                    let current = current_track.load(Ordering::Relaxed);
                    let selected: Vec<usize> = if track == current {
                        selected_steps.lock().unwrap().iter().copied().collect()
                    } else {
                        Vec::new()
                    };
                    let has_selection = !selected.is_empty();
                    if !has_selection {
                        set_track_send_base(app, track, bus_id, amount);
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
                    // The send controls read eseq.kinds `send.display` (the
                    // edited lock at the selected step, else the base).
                    let rt = editor.runtime_mut();
                    rt.run_reactive_cycle();
                    editor.refresh_runtime_side_effects();
                }
            }
        }
        // `send.amount` :set: the track's own send level, never a p-lock;
        // the bus is named by id, so a reorder before this lands cannot
        // retarget it.
        "set-track-send-base" => {
            let Value::Map(ref map) = payload else {
                return;
            };
            let number = |key: &str| {
                map.get(key).and_then(|cell| match &*cell.borrow() {
                    Value::Number(n) => Some(*n),
                    _ => None,
                })
            };
            let (Some(track), Some(bus_id), Some(amount)) =
                (number("track"), number("bus-id"), number("amount"))
            else {
                return;
            };
            let (track, bus_id) = (track as usize, sequencer::sequencer::BusId(bus_id as u64));
            if !app.buses.iter().any(|bus| bus.id == bus_id) {
                return;
            }
            if bus_id == sequencer::sequencer::BusId::MIX || track >= state.active_track_count() {
                return;
            }
            set_track_send_base(app, track, bus_id, amount.clamp(0.0, 1.0) as f32);
            let rt = editor.runtime_mut();
            rt.run_reactive_cycle();
            editor.refresh_runtime_side_effects();
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
                            Ok(()) => bus_effect_param_applied(
                                app,
                                editor,
                                ctx.shared,
                                (bus_idx, slot_idx, param_idx),
                                desc.as_ref(),
                            ),
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

/// Shared by interactive dispatch and the production headless capture path.
pub(crate) fn apply_bus_routing_command(name: &str, payload: &Value, app: &mut app::App) -> Result<(), String> {
    if name == "add-bus" {
        let mut number = 3;
        let name = loop {
            let candidate = format!("Bus {number}");
            if !app.buses.iter().any(|bus| bus.name == candidate) { break candidate; }
            number += 1;
        };
        app.add_bus_recorded(name)?;
        return Ok(());
    }
    let source = extract_usize_from_payload(payload, "bus-id").ok_or("Missing source bus id")?;
    let target = extract_usize_from_payload(payload, "destination-id").ok_or("Missing destination bus id")?;
    app.set_bus_output_recorded(sequencer::sequencer::BusId(source as u64), sequencer::sequencer::BusId(target as u64))
}
