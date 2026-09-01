use crate::*;

pub(super) const COMMANDS: &[&str] = &[
    "add-rack-sample-slot",
    "replace-rack-slot-sample",
    "delete-rack-slot",
    "add-rack-slot-effect",
    "insert-rack-slot-effect-before-slot",
    "delete-rack-slot-effect",
    "move-rack-slot-effect",
    "set-rack-slot-effect-param",
    "set-rack-slot-effect-plock",
    "set-rack-slot-effect-param-option",
    "set-rack-slot-effect-plock-option",
    "group-track-to-instrument-rack",
    "add-rack-instrument-slot",
    "replace-rack-slot-instrument",
    "select-rack-slot",
    "set-rack-slot-gain",
    "set-rack-slot-pan",
    "set-rack-slot-mute",
    "set-rack-slot-solo",
    "set-rack-slot-max-polyphony",
    "set-rack-slot-choke-group",
    "set-rack-slot-base-note",
    "set-rack-slot-param-plock",
    "set-rack-macro-value",
    "rename-rack-macro",
    "set-rack-macro-plock",
    "map-rack-macro-param",
    "unmap-rack-macro-param",
    "set-rack-macro-range",
    "set-rack-macro-curve",
    "set-rack-slot-instrument-param",
    "set-rack-slot-instrument-plock",
    "set-rack-slot-instrument-param-batch",
    "set-rack-slot-instrument-plock-batch",
    "toggle-rack-slot-instrument-param",
    "toggle-rack-slot-instrument-plock",
    "set-rack-slot-instrument-param-option",
    "set-rack-slot-instrument-plock-option",
    "set-instrument-param-batch",
    "set-instrument-plock-batch",
    "set-instrument-key-lock-batch",
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
    let selected_steps = ctx.shared.selected_steps.clone();
    let selected_neural_neurons = ctx.shared.selected_neural_neurons.clone();
    let ui_epoch = ctx.shared.ui_epoch.clone();
    let fx_epoch = ctx.shared.fx_epoch.clone();
    let track_pan_ids = ctx.shared.track_pan_ids.clone();
    let record_armed = ctx.shared.record_armed.clone();
    let accumulator_names = ctx.shared.accumulator_names.clone();
    match name {
        "add-rack-sample-slot" => {
            let path_str = extract_path_from_payload(&payload);
            let track = extract_usize_from_payload(&payload, "track")
                .or_else(|| current_track_for_app(&mut app, &current_track));
            let preserve_browser_context =
                extract_bool_from_payload(&payload, "preserve-browser-context");
            match (track, path_str) {
                (Some(track), Some(path_str)) => {
                    if preserve_browser_context {
                        preserve_sample_browser_context_for_loaded_sample(
                            &mut editor,
                            &path_str,
                        );
                    }
                    let path = Path::new(&path_str);
                    match app.apply_recorded_rack_slot_add(
                        track,
                        "Add rack sample",
                        |app| app.graph_controller().add_sampler_slot_to_rack(track, path),
                    ) {
                        Ok(slot_idx) => {
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
                            editor.handle_host_event(HostEvent::Status(format!(
                                "Added rack layer {}",
                                slot_idx + 1
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
                                "Error adding rack layer: {e}"
                            )));
                        }
                    }
                }
                _ => {
                    editor.handle_host_event(HostEvent::Status(
                        "Rack layer is missing a track or sample path".to_string(),
                    ));
                }
            }
        }
        "replace-rack-slot-sample" => {
            let path_str = extract_path_from_payload(&payload);
            let track = extract_usize_from_payload(&payload, "track")
                .or_else(|| current_track_for_app(&mut app, &current_track));
            let slot = extract_usize_from_payload(&payload, "slot");
            let preserve_browser_context =
                extract_bool_from_payload(&payload, "preserve-browser-context");
            match (track, slot, path_str) {
                (Some(track), Some(slot), Some(path_str)) => {
                    if preserve_browser_context {
                        preserve_sample_browser_context_for_loaded_sample(
                            &mut editor,
                            &path_str,
                        );
                    }
                    match app.apply_recorded_rack_slot_source_replacement(
                        track,
                        slot,
                        "Replace rack sample",
                        |app| app.graph_controller().replace_rack_slot_with_sampler(
                            track,
                            slot,
                            Path::new(&path_str),
                        ),
                    ) {
                        Ok(()) => {
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
                            editor.handle_host_event(HostEvent::Status(format!(
                                "Replaced rack layer {} with sample",
                                slot + 1
                            )));
                        }
                        Err(error) => {
                            if preserve_browser_context {
                                preserve_sample_browser_context_for_loaded_sample(
                                    &mut editor,
                                    "",
                                );
                            }
                            editor.handle_host_event(HostEvent::Status(format!(
                                "Error replacing rack layer: {error}"
                            )));
                        }
                    }
                }
                _ => editor.handle_host_event(HostEvent::Status(
                    "Rack layer replacement is missing a track, slot, or sample path"
                        .to_string(),
                )),
            }
        }
        "delete-rack-slot" => {
            let track = extract_usize_from_payload(&payload, "track")
                .or_else(|| current_track_for_app(&mut app, &current_track));
            let slot_idx = extract_usize_from_payload(&payload, "slot");
            match (track, slot_idx) {
                (Some(track), Some(slot_idx)) => {
                    match app.apply_recorded_instrument_binding_mutation(
                        track,
                        "Delete rack layer",
                        |app| app.graph_controller().delete_rack_slot(track, slot_idx),
                    ) {
                        Ok(()) => {
                            refresh_instrument_panel_reactive(
                                &mut editor,
                                &app,
                                track,
                                &selected_steps,
                                &ui_epoch,
                            );
                            fx_epoch.fetch_add(1, Ordering::Relaxed);
                            editor.handle_host_event(HostEvent::Status(format!(
                                "Deleted rack layer {}",
                                slot_idx + 1
                            )));
                        }
                        Err(error) => {
                            editor.handle_host_event(HostEvent::Status(format!(
                                "Error deleting rack layer: {error}"
                            )));
                        }
                    }
                }
                _ => editor.handle_host_event(HostEvent::Status(
                    "Rack layer deletion is missing a track or layer".to_string(),
                )),
            }
        }
        "add-rack-slot-effect" => {
            let track = extract_usize_from_payload(&payload, "track");
            let rack_slot = extract_usize_from_payload(&payload, "rack-slot");
            let name = extract_string_from_payload(&payload, "name");
            let builtin = extract_bool_from_payload(&payload, "builtin");
            match (track, rack_slot, name) {
                (Some(track), Some(rack_slot), Some(name)) => {
                    let is_builtin = builtin
                        || sequencer::effects::EffectDescriptor::builtin_insert(&name)
                            .is_some()
                        || sequencer::effects::dgen_builtin::contains(&name);
                    let result = app.apply_recorded_rack_effect_chain_mutation(
                        track,
                        rack_slot,
                        "Add rack-slot effect",
                        |app| if is_builtin {
                            app.add_builtin_rack_slot_effect_sync(track, rack_slot, &name)
                        } else {
                            app.add_rack_slot_effect_sync(track, rack_slot, &name)
                        },
                    );
                    match result {
                        Ok(_) => {
                            refresh_instrument_panel_reactive(
                                &mut editor,
                                &app,
                                track,
                                &selected_steps,
                                &ui_epoch,
                            );
                            fx_epoch.fetch_add(1, Ordering::Relaxed);
                        }
                        Err(error) => editor.handle_host_event(HostEvent::Status(
                            format!("Error adding rack-slot effect: {error}"),
                        )),
                    }
                }
                _ => editor.handle_host_event(HostEvent::Status(
                    "Rack-slot effect drop is incomplete".to_string(),
                )),
            }
        }
        "insert-rack-slot-effect-before-slot" => {
            let track = extract_usize_from_payload(&payload, "track");
            let rack_slot = extract_usize_from_payload(&payload, "rack-slot");
            let target_slot = extract_usize_from_payload(&payload, "slot");
            let name = extract_string_from_payload(&payload, "name");
            let builtin = extract_bool_from_payload(&payload, "builtin");
            match (track, rack_slot, target_slot, name) {
                (Some(track), Some(rack_slot), Some(target_slot), Some(name)) => {
                    let result = app.apply_recorded_rack_effect_chain_mutation(
                        track,
                        rack_slot,
                        "Insert rack-slot effect",
                        |app| if builtin {
                            app.insert_builtin_rack_slot_effect_before_slot_sync(
                                track,
                                rack_slot,
                                target_slot,
                                &name,
                            )
                        } else {
                            app.insert_rack_slot_effect_before_slot_sync(
                                track,
                                rack_slot,
                                target_slot,
                                &name,
                            )
                        },
                    );
                    match result {
                        Ok(_) => {
                            refresh_instrument_panel_reactive(
                                &mut editor,
                                &app,
                                track,
                                &selected_steps,
                                &ui_epoch,
                            );
                            fx_epoch.fetch_add(1, Ordering::Relaxed);
                        }
                        Err(error) => editor.handle_host_event(HostEvent::Status(
                            format!("Error inserting rack-slot effect: {error}"),
                        )),
                    }
                }
                _ => editor.handle_host_event(HostEvent::Status(
                    "Rack-slot effect insert is incomplete".to_string(),
                )),
            }
        }
        "delete-rack-slot-effect" => {
            let track = extract_usize_from_payload(&payload, "track");
            let rack_slot = extract_usize_from_payload(&payload, "rack-slot");
            let effect_slot = extract_usize_from_payload(&payload, "effect-slot");
            match (track, rack_slot, effect_slot) {
                (Some(track), Some(rack_slot), Some(effect_slot)) => match app
                    .apply_recorded_rack_effect_chain_mutation(
                        track,
                        rack_slot,
                        "Delete rack-slot effect",
                        |app| app.delete_rack_slot_effect_slot(
                            track, rack_slot, effect_slot,
                        ),
                    )
                {
                    Ok(()) => {
                        refresh_instrument_panel_reactive(
                            &mut editor,
                            &app,
                            track,
                            &selected_steps,
                            &ui_epoch,
                        );
                        fx_epoch.fetch_add(1, Ordering::Relaxed);
                    }
                    Err(error) => editor.handle_host_event(HostEvent::Status(format!(
                        "Error deleting rack-slot effect: {error}"
                    ))),
                },
                _ => editor.handle_host_event(HostEvent::Status(
                    "Rack-slot effect deletion is incomplete".to_string(),
                )),
            }
        }
        "move-rack-slot-effect" => {
            let track = extract_usize_from_payload(&payload, "track");
            let rack_slot = extract_usize_from_payload(&payload, "rack-slot");
            let source_slot = extract_usize_from_payload(&payload, "source-slot");
            let requested_target = extract_usize_from_payload(&payload, "target-slot");
            let position = extract_string_from_payload(&payload, "position");
            match (track, rack_slot, source_slot) {
                (Some(track), Some(rack_slot), Some(source_slot)) => {
                    let target_slot = if position.as_deref() == Some("append") {
                        rack_slot_snapshot_for_host(&state, track, rack_slot).and_then(
                            |slot| {
                                slot.effect_slots
                                    .iter()
                                    .rposition(|effect| effect.node_id != 0)
                            },
                        )
                    } else {
                        requested_target.map(|target| {
                            if source_slot < target {
                                target.saturating_sub(1)
                            } else {
                                target
                            }
                        })
                    };
                    if let Some(target_slot) = target_slot {
                        match app.apply_recorded_rack_effect_chain_mutation(
                            track,
                            rack_slot,
                            "Move rack-slot effect",
                            |app| app.move_rack_slot_effect_slot_sync(
                                track,
                                rack_slot,
                                source_slot,
                                target_slot,
                            ),
                        ) {
                            Ok(()) => {
                                refresh_instrument_panel_reactive(
                                    &mut editor,
                                    &app,
                                    track,
                                    &selected_steps,
                                    &ui_epoch,
                                );
                                fx_epoch.fetch_add(1, Ordering::Relaxed);
                            }
                            Err(error) => editor.handle_host_event(HostEvent::Status(
                                format!("Error moving rack-slot effect: {error}"),
                            )),
                        }
                    } else {
                        editor.handle_host_event(HostEvent::Status(
                            "Rack-slot FX move is missing a destination".to_string(),
                        ));
                    }
                }
                _ => editor.handle_host_event(HostEvent::Status(
                    "Rack-slot effect move is missing its source".to_string(),
                )),
            }
        }
        "set-rack-slot-effect-param" => {
            let track = extract_usize_from_payload(&payload, "track");
            let rack_slot = extract_usize_from_payload(&payload, "rack-slot");
            let effect_slot = extract_usize_from_payload(&payload, "effect-slot");
            let param = extract_usize_from_payload(&payload, "param");
            let value = extract_f32_from_payload(&payload, "value");
            match (track, rack_slot, effect_slot, param, value) {
                (
                    Some(track),
                    Some(rack_slot),
                    Some(effect_slot),
                    Some(param),
                    Some(value),
                ) => {
                    let print_value = rack_slot_effect_print_value(
                        &app,
                        track,
                        rack_slot,
                        effect_slot,
                        param,
                        value,
                    );
                    let value = print_value.unwrap_or(value);
                    if print_value.is_some()
                        && try_latch_param_print(
                            ctx.shared,
                            &mut editor,
                            &app,
                            track,
                            &[(PrintTarget::RackSlotEffect {
                                rack_slot_idx: rack_slot,
                                effect_slot_idx: effect_slot,
                                param_idx: param,
                            }, value)],
                        )
                    {
                        return;
                    }
                    let outcome = app::try_apply_command(
                        &mut app,
                        app::AppCommand::SetRackSlotEffectParam {
                            track,
                            rack_slot_idx: rack_slot,
                            effect_slot_idx: effect_slot,
                            param_idx: param,
                            value,
                        },
                    );
                    if outcome.is_ok() {
                        ctx.gesture.rack_control_snapshot_dirty = true;
                        if rack_slot_effect_param_needs_panel_rebuild(
                            &state,
                            track,
                            rack_slot,
                            effect_slot,
                            param,
                        ) {
                            refresh_instrument_panel_reactive(
                                &mut editor,
                                &app,
                                track,
                                &selected_steps,
                                &ui_epoch,
                            );
                        } else {
                            refresh_rack_direct_param_reactive(
                                &mut editor,
                                &app,
                                &state,
                                track,
                                RackDirectDisplayTarget::EffectParam {
                                    rack_slot,
                                    effect_slot,
                                    param_idx: param,
                                },
                                &selected_steps,
                                false,
                                &ui_epoch,
                            );
                        }
                    } else if let Err(error) = outcome {
                        editor.handle_host_event(HostEvent::Status(format!(
                            "Error setting rack-slot effect parameter: {error:?}"
                        )));
                    }
                }
                _ => editor.handle_host_event(HostEvent::Status(
                    "Rack-slot effect parameter edit is incomplete".to_string(),
                )),
            }
        }
        "set-rack-slot-effect-plock" => {
            let track = extract_usize_from_payload(&payload, "track");
            let rack_slot = extract_usize_from_payload(&payload, "rack-slot");
            let effect_slot = extract_usize_from_payload(&payload, "effect-slot");
            let param = extract_usize_from_payload(&payload, "param");
            let value = extract_f32_from_payload(&payload, "value");
            match (track, rack_slot, effect_slot, param, value) {
                (
                    Some(track),
                    Some(rack_slot),
                    Some(effect_slot),
                    Some(param),
                    Some(value),
                ) => {
                    let steps: Vec<usize> =
                        selected_steps.lock().unwrap().iter().copied().collect();
                    let outcome = app::try_apply_command(
                        &mut app,
                        app::AppCommand::SetRackSlotEffectPlockMulti {
                            track,
                            steps,
                            rack_slot_idx: rack_slot,
                            effect_slot_idx: effect_slot,
                            param_idx: param,
                            value,
                        },
                    );
                    if !outcome
                        .is_ok_and(|outcome| outcome != app::edit::EditOutcome::NoOp)
                    {
                        editor.handle_host_event(HostEvent::Status(format!(
                            "Rack-slot effect parameter locks were not changed"
                        )));
                    } else {
                        ctx.gesture.rack_control_snapshot_dirty = true;
                        if rack_slot_effect_param_needs_panel_rebuild(
                            &state,
                            track,
                            rack_slot,
                            effect_slot,
                            param,
                        ) {
                            sync_rack_slot_instrument_authoring_display(
                                &mut editor,
                                &app,
                                &state,
                                track,
                                &selected_steps,
                            );
                            ui_epoch.fetch_add(1, Ordering::Relaxed);
                        } else {
                            refresh_rack_direct_param_reactive(
                                &mut editor,
                                &app,
                                &state,
                                track,
                                RackDirectDisplayTarget::EffectParam {
                                    rack_slot,
                                    effect_slot,
                                    param_idx: param,
                                },
                                &selected_steps,
                                true,
                                &ui_epoch,
                            );
                        }
                        fx_epoch.fetch_add(1, Ordering::Relaxed);
                    }
                }
                _ => editor.handle_host_event(HostEvent::Status(
                    "Rack-slot effect parameter-lock edit is incomplete".to_string(),
                )),
            }
        }
        "set-rack-slot-effect-param-option" | "set-rack-slot-effect-plock-option" => {
            let write_plock = name == "set-rack-slot-effect-plock-option";
            let track = extract_usize_from_payload(&payload, "track");
            let rack_slot = extract_usize_from_payload(&payload, "rack-slot");
            let effect_slot = extract_usize_from_payload(&payload, "effect-slot");
            let param_idx = extract_usize_from_payload(&payload, "param");
            let label = extract_string_from_payload(&payload, "label");
            match (track, rack_slot, effect_slot, param_idx, label) {
                (
                    Some(track),
                    Some(rack_slot),
                    Some(effect_slot),
                    Some(param_idx),
                    Some(label),
                ) => {
                    let option_value = app.rack_slot_effect_option_value(
                        track, rack_slot, effect_slot, param_idx, &label,
                    );
                    if !write_plock {
                        if let Ok(value) = option_value.as_ref() {
                            if let Some(value) = rack_slot_effect_print_value(
                                &app,
                                track,
                                rack_slot,
                                effect_slot,
                                param_idx,
                                *value,
                            ) {
                                if try_latch_param_print(
                                    ctx.shared,
                                    &mut editor,
                                    &app,
                                    track,
                                    &[(PrintTarget::RackSlotEffect {
                                        rack_slot_idx: rack_slot,
                                        effect_slot_idx: effect_slot,
                                        param_idx,
                                    }, value)],
                                ) {
                                    return;
                                }
                            }
                        }
                    }
                    let result = if write_plock {
                        let steps: Vec<usize> =
                            selected_steps.lock().unwrap().iter().copied().collect();
                        option_value.and_then(|value| {
                            let outcome = app::try_apply_command(
                                &mut app,
                                app::AppCommand::SetRackSlotEffectPlockMulti {
                                    track,
                                    steps,
                                    rack_slot_idx: rack_slot,
                                    effect_slot_idx: effect_slot,
                                    param_idx,
                                    value,
                                },
                            );
                            outcome
                                .map_err(|error| format!("{error:?}"))
                                .and_then(|outcome| {
                                    (outcome != app::edit::EditOutcome::NoOp)
                                        .then_some(())
                                        .ok_or_else(|| {
                                        "Rack-slot effect parameter locks were not changed"
                                            .to_string()
                                        })
                                })
                        })
                    } else {
                        option_value.and_then(|value| {
                            app::try_apply_command(
                                &mut app,
                                app::AppCommand::SetRackSlotEffectParam {
                                    track,
                                    rack_slot_idx: rack_slot,
                                    effect_slot_idx: effect_slot,
                                    param_idx,
                                    value,
                                },
                            )
                            .map(|_| ())
                            .map_err(|error| format!("{error:?}"))
                        })
                    };
                    match result {
                        Ok(()) => {
                            refresh_instrument_panel_reactive(
                                &mut editor,
                                &app,
                                track,
                                &selected_steps,
                                &ui_epoch,
                            );
                            fx_epoch.fetch_add(1, Ordering::Relaxed);
                            ui_epoch.fetch_add(1, Ordering::Relaxed);
                        }
                        Err(error) => editor.handle_host_event(HostEvent::Status(
                            format!("Error setting rack-slot effect option: {error}"),
                        )),
                    }
                }
                _ => editor.handle_host_event(HostEvent::Status(
                    "Rack-slot effect option edit is incomplete".to_string(),
                )),
            }
        }
        "group-track-to-instrument-rack" => {
            let track = extract_usize_from_payload(&payload, "track")
                .or_else(|| current_track_for_app(&mut app, &current_track));
            match track {
                Some(track) => {
                    match app.group_track_to_instrument_rack_recorded(track) {
                        Ok(()) => {
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
                                "Grouped track to Instrument Rack".to_string(),
                            ));
                        }
                        Err(error) => editor.handle_host_event(HostEvent::Status(
                            format!("Could not group track: {error}"),
                        )),
                    }
                }
                None => editor.handle_host_event(HostEvent::Status(
                    "No track selected for grouping".to_string(),
                )),
            }
        }
        "add-rack-instrument-slot" => {
            let track = extract_usize_from_payload(&payload, "track")
                .or_else(|| current_track_for_app(&mut app, &current_track));
            let name = extract_string_from_payload(&payload, "name");
            match (track, name) {
                (Some(track), Some(name)) => {
                    match app.add_saved_instrument_slot_to_rack_sync(track, &name) {
                        Ok(slot_idx) => {
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
                            editor.handle_host_event(HostEvent::Status(format!(
                                "Added rack instrument layer {}: {}",
                                slot_idx + 1,
                                name
                            )));
                        }
                        Err(error) => {
                            editor.handle_host_event(HostEvent::Status(format!(
                                "Error adding rack instrument layer: {error}"
                            )));
                        }
                    }
                }
                _ => {
                    editor.handle_host_event(HostEvent::Status(
                        "Rack instrument layer is missing a track or instrument name"
                            .to_string(),
                    ));
                }
            }
        }
        "replace-rack-slot-instrument" => {
            let track = extract_usize_from_payload(&payload, "track")
                .or_else(|| current_track_for_app(&mut app, &current_track));
            let slot = extract_usize_from_payload(&payload, "slot");
            let name = extract_string_from_payload(&payload, "name");
            match (track, slot, name) {
                (Some(track), Some(slot), Some(name)) => {
                    match app.replace_rack_slot_with_saved_instrument_sync(
                        track, slot, &name,
                    ) {
                        Ok(()) => {
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
                            editor.handle_host_event(HostEvent::Status(format!(
                                "Replaced rack layer {} with {}",
                                slot + 1,
                                name
                            )));
                        }
                        Err(error) => editor.handle_host_event(HostEvent::Status(
                            format!("Error replacing rack instrument layer: {error}"),
                        )),
                    }
                }
                _ => editor.handle_host_event(HostEvent::Status(
                    "Rack instrument replacement is missing a track, slot, or instrument name"
                        .to_string(),
                )),
            }
        }
        "select-rack-slot" => {
            if let Value::Map(ref map) = payload {
                if let (Some(track), Some(slot_idx)) =
                    (map_usize(map, "track"), map_usize(map, "slot"))
                {
                    let rack = {
                        app.state
                            .pattern
                            .rack_tracks
                            .lock()
                            .unwrap()
                            .get(track)
                            .cloned()
                            .flatten()
                    };
                    let selected = rack.is_some_and(|rack| {
                        app.select_rack_slot(track, &rack, slot_idx)
                    });
                    if selected {
                        refresh_instrument_panel_reactive(
                            &mut editor,
                            &app,
                            track,
                            &selected_steps,
                            &ui_epoch,
                        );
                    }
                }
            }
        }
        "set-rack-slot-gain" => {
            if let Value::Map(ref map) = payload {
                if let (Some(track), Some(slot_idx), Some(value)) = (
                    map_usize(map, "track"),
                    map_usize(map, "slot"),
                    map_number(map, "value").map(|value| value as f32),
                ) {
                    if try_latch_rack_slot_param_print(
                        ctx.shared,
                        &mut editor,
                        &app,
                        track,
                        slot_idx,
                        RackSlotParam::Gain,
                        value,
                    ) {
                        return;
                    }
                    app::apply_command(
                        &mut app,
                        app::AppCommand::SetRackSlotGain {
                            track,
                            slot_idx,
                            value,
                        },
                    );
                    ctx.gesture.rack_control_snapshot_dirty = true;
                    refresh_rack_direct_param_reactive(
                        &mut editor,
                        &app,
                        &state,
                        track,
                        RackDirectDisplayTarget::SlotParam {
                            slot_idx,
                            param: RackSlotParam::Gain,
                        },
                        &selected_steps,
                        false,
                        &ui_epoch,
                    );
                }
            }
        }
        "set-rack-slot-pan" => {
            if let Value::Map(ref map) = payload {
                if let (Some(track), Some(slot_idx), Some(value)) = (
                    map_usize(map, "track"),
                    map_usize(map, "slot"),
                    map_number(map, "value").map(|value| value as f32),
                ) {
                    if try_latch_rack_slot_param_print(
                        ctx.shared,
                        &mut editor,
                        &app,
                        track,
                        slot_idx,
                        RackSlotParam::Pan,
                        value,
                    ) {
                        return;
                    }
                    app::apply_command(
                        &mut app,
                        app::AppCommand::SetRackSlotPan {
                            track,
                            slot_idx,
                            value,
                        },
                    );
                    ctx.gesture.rack_control_snapshot_dirty = true;
                    refresh_rack_direct_param_reactive(
                        &mut editor,
                        &app,
                        &state,
                        track,
                        RackDirectDisplayTarget::SlotParam {
                            slot_idx,
                            param: RackSlotParam::Pan,
                        },
                        &selected_steps,
                        false,
                        &ui_epoch,
                    );
                }
            }
        }
        "set-rack-slot-mute" => {
            if let Value::Map(ref map) = payload {
                if let (Some(track), Some(slot_idx)) =
                    (map_usize(map, "track"), map_usize(map, "slot"))
                {
                    let value = map_bool(map, "value");
                    if try_latch_rack_slot_param_print(
                        ctx.shared,
                        &mut editor,
                        &app,
                        track,
                        slot_idx,
                        RackSlotParam::Mute,
                        if value { 1.0 } else { 0.0 },
                    ) {
                        return;
                    }
                    app::apply_command(
                        &mut app,
                        app::AppCommand::SetRackSlotMute {
                            track,
                            slot_idx,
                            value,
                        },
                    );
                    // Without republishing the scheduler snapshot,
                    // per-trigger panner pushes clobber the new
                    // mute with the stale snapshot's value.
                    ctx.gesture.rack_control_snapshot_dirty = true;
                    refresh_rack_direct_param_reactive(
                        &mut editor,
                        &app,
                        &state,
                        track,
                        RackDirectDisplayTarget::SlotParam {
                            slot_idx,
                            param: RackSlotParam::Mute,
                        },
                        &selected_steps,
                        false,
                        &ui_epoch,
                    );
                    // The rack panel's pad/slot dicts carry mute as a
                    // plain value, so rebuild them or the panel shows
                    // stale M/S state.
                    sync_rack_slot_instrument_authoring_display(
                        &mut editor,
                        &app,
                        &state,
                        track,
                        &selected_steps,
                    );
                }
            }
        }
        "set-rack-slot-solo" => {
            if let Value::Map(ref map) = payload {
                if let (Some(track), Some(slot_idx)) =
                    (map_usize(map, "track"), map_usize(map, "slot"))
                {
                    let value = map_bool(map, "value");
                    if try_latch_rack_slot_param_print(
                        ctx.shared,
                        &mut editor,
                        &app,
                        track,
                        slot_idx,
                        RackSlotParam::Solo,
                        if value { 1.0 } else { 0.0 },
                    ) {
                        return;
                    }
                    app::apply_command(
                        &mut app,
                        app::AppCommand::SetRackSlotSolo {
                            track,
                            slot_idx,
                            value,
                        },
                    );
                    ctx.gesture.rack_control_snapshot_dirty = true;
                    refresh_rack_direct_param_reactive(
                        &mut editor,
                        &app,
                        &state,
                        track,
                        RackDirectDisplayTarget::SlotParam {
                            slot_idx,
                            param: RackSlotParam::Solo,
                        },
                        &selected_steps,
                        false,
                        &ui_epoch,
                    );
                    sync_rack_slot_instrument_authoring_display(
                        &mut editor,
                        &app,
                        &state,
                        track,
                        &selected_steps,
                    );
                }
            }
        }
        "set-rack-slot-max-polyphony" => {
            if let Value::Map(ref map) = payload {
                if let (Some(track), Some(slot_idx), Some(value)) = (
                    map_usize(map, "track"),
                    map_usize(map, "slot"),
                    map_usize(map, "value"),
                ) {
                    if try_latch_rack_slot_param_print(
                        ctx.shared,
                        &mut editor,
                        &app,
                        track,
                        slot_idx,
                        RackSlotParam::MaxPolyphony,
                        value as f32,
                    ) {
                        return;
                    }
                    app::apply_command(
                        &mut app,
                        app::AppCommand::SetRackSlotMaxPolyphony {
                            track,
                            slot_idx,
                            value,
                        },
                    );
                    refresh_rack_direct_param_reactive(
                        &mut editor,
                        &app,
                        &state,
                        track,
                        RackDirectDisplayTarget::SlotParam {
                            slot_idx,
                            param: RackSlotParam::MaxPolyphony,
                        },
                        &selected_steps,
                        false,
                        &ui_epoch,
                    );
                }
            }
        }
        "set-rack-slot-choke-group" => {
            if let Value::Map(ref map) = payload {
                if let (Some(track), Some(slot_idx), Some(value)) = (
                    map_usize(map, "track"),
                    map_usize(map, "slot"),
                    map_number(map, "value")
                        .map(|value| value.round().clamp(0.0, u8::MAX as f64) as u8),
                ) {
                    app::apply_command(
                        &mut app,
                        app::AppCommand::SetRackSlotChokeGroup {
                            track,
                            slot_idx,
                            value,
                        },
                    );
                    refresh_rack_direct_param_reactive(
                        &mut editor,
                        &app,
                        &state,
                        track,
                        RackDirectDisplayTarget::SlotParam {
                            slot_idx,
                            param: RackSlotParam::BaseNote,
                        },
                        &selected_steps,
                        false,
                        &ui_epoch,
                    );
                }
            }
        }
        "set-rack-slot-base-note" => {
            if let Value::Map(ref map) = payload {
                if let (Some(track), Some(slot_idx), Some(value)) = (
                    map_usize(map, "track"),
                    map_usize(map, "slot"),
                    map_number(map, "value").map(|value| value as f32),
                ) {
                    if try_latch_rack_slot_param_print(
                        ctx.shared,
                        &mut editor,
                        &app,
                        track,
                        slot_idx,
                        RackSlotParam::BaseNote,
                        value,
                    ) {
                        return;
                    }
                    app::apply_command(
                        &mut app,
                        app::AppCommand::SetRackSlotBaseNoteOffset {
                            track,
                            slot_idx,
                            value,
                        },
                    );
                    refresh_instrument_panel_reactive(
                        &mut editor,
                        &app,
                        track,
                        &selected_steps,
                        &ui_epoch,
                    );
                }
            }
        }
        "set-rack-slot-param-plock" => {
            if let Value::Map(ref map) = payload {
                let track = map_usize(map, "track");
                let slot_idx = map_usize(map, "slot");
                let param = map_string(map, "param")
                    .and_then(|name| RackSlotParam::from_name(&name));
                let value = map_number_or_bool(map, "value").map(|value| value as f32);
                if let (Some(track), Some(slot_idx), Some(param), Some(value)) =
                    (track, slot_idx, param, value)
                {
                    let steps: Vec<usize> =
                        selected_steps.lock().unwrap().iter().copied().collect();
                    app::apply_command(
                        &mut app,
                        app::AppCommand::SetRackSlotParamPlockMulti {
                            track,
                            slot_idx,
                            steps,
                            param,
                            value,
                        },
                    );
                    ctx.gesture.rack_control_snapshot_dirty = true;
                    refresh_rack_direct_param_reactive(
                        &mut editor,
                        &app,
                        &state,
                        track,
                        RackDirectDisplayTarget::SlotParam { slot_idx, param },
                        &selected_steps,
                        true,
                        &ui_epoch,
                    );
                    fx_epoch.fetch_add(1, Ordering::Relaxed);
                }
            }
        }
        "set-rack-macro-value" => {
            if let Value::Map(ref map) = payload {
                if let (Some(track), Some(macro_idx), Some(value)) = (
                    map_usize(map, "track"),
                    map_usize(map, "id"),
                    map_number(map, "value").map(|value| value as f32),
                ) {
                    if try_latch_param_print(
                        ctx.shared,
                        &mut editor,
                        &app,
                        track,
                        &[(PrintTarget::RackMacro { macro_idx }, value.clamp(0.0, 1.0))],
                    ) {
                        return;
                    }
                }
                apply_rack_macro_host_command(
                    &name,
                    map,
                    &mut editor,
                    &mut app,
                    &state,
                    &selected_steps,
                    &ui_epoch,
                    &fx_epoch,
                );
            }
        }
        "rename-rack-macro" => {
            if let Value::Map(ref map) = payload {
                if let (Some(track), Some(id), Some(name)) = (
                    map_usize(map, "track"),
                    map_usize(map, "id"),
                    map_string(map, "name"),
                ) {
                    if let Some(id) = sequencer::sequencer::RackMacroId::from_index(id)
                    {
                        app.rename_rack_macro(track, id, name);
                        refresh_instrument_panel_reactive(
                            &mut editor,
                            &app,
                            track,
                            &selected_steps,
                            &ui_epoch,
                        );
                    }
                }
            }
        }
        "set-rack-macro-plock" => {
            if let Value::Map(ref map) = payload {
                apply_rack_macro_host_command(
                    &name,
                    map,
                    &mut editor,
                    &mut app,
                    &state,
                    &selected_steps,
                    &ui_epoch,
                    &fx_epoch,
                );
            }
        }
        "map-rack-macro-param" => {
            if let Value::Map(ref map) = payload {
                let track = map_usize(map, "track");
                let id = map_usize(map, "id")
                    .and_then(sequencer::sequencer::RackMacroId::from_index);
                let kind = map_string(map, "kind");
                let rack_slot = map_usize(map, "rack-slot");
                let param = map_string(map, "param");
                let param_index = map_usize(map, "param-idx");
                let min = map_number(map, "min").map(|value| value as f32);
                let max = map_number(map, "max").map(|value| value as f32);
                if let (
                    Some(track),
                    Some(id),
                    Some(kind),
                    Some(rack_slot),
                    Some(param),
                    Some(param_index),
                    Some(min),
                    Some(max),
                ) = (track, id, kind, rack_slot, param, param_index, min, max)
                {
                    let resolved = if kind == "rack-slot-instrument" {
                        rack_slot_snapshot_for_host(&state, track, rack_slot)
                            .and_then(|slot| app.rack_slot_instrument_descriptor(&slot))
                            .and_then(|descriptor| descriptor.params.get(param_index).cloned())
                            .map(|descriptor| (sequencer::sequencer::RackMacroTarget::SlotInstrumentParam {
                                slot: rack_slot, param, param_index,
                            }, descriptor.user_input_to_stored(min), descriptor.user_input_to_stored(max)))
                    } else if kind == "rack-slot-effect" {
                        map_usize(map, "effect-slot").and_then(|effect_slot| {
                            let descriptor = rack_slot_snapshot_for_host(&state, track, rack_slot)?
                                .effect_descriptors.get(effect_slot)?.params.get(param_index)?.clone();
                            Some((sequencer::sequencer::RackMacroTarget::SlotEffectParam {
                                slot: rack_slot, effect_slot, param, param_index,
                            }, descriptor.user_input_to_stored(min), descriptor.user_input_to_stored(max)))
                        })
                    } else {
                        None
                    };
                    if let Some((target, range_min, range_max)) = resolved {
                        let mapping = sequencer::sequencer::RackMacroMapping {
                            target,
                            range_min,
                            range_max,
                            curve: sequencer::sequencer::RackMacroCurve::Linear,
                        };
                        if let Err(error) = app.map_rack_macro(track, id, mapping) {
                            eprintln!("rack macro mapping failed: {error}");
                        } else {
                            app.set_rack_macro_value(track, id, 0.0);
                        }
                        refresh_instrument_panel_reactive(
                            &mut editor,
                            &app,
                            track,
                            &selected_steps,
                            &ui_epoch,
                        );
                    }
                }
            }
        }
        "unmap-rack-macro-param" => {
            if let Value::Map(ref map) = payload {
                if let (Some(track), Some(id), Some(mapping_idx)) = (
                    map_usize(map, "track"),
                    map_usize(map, "id")
                        .and_then(sequencer::sequencer::RackMacroId::from_index),
                    map_usize(map, "mapping-idx"),
                ) {
                    app.unmap_rack_macro(track, id, mapping_idx);
                    refresh_instrument_panel_reactive(
                        &mut editor,
                        &app,
                        track,
                        &selected_steps,
                        &ui_epoch,
                    );
                }
            }
        }
        "set-rack-macro-range" => {
            if let Value::Map(ref map) = payload {
                if let (
                    Some(track),
                    Some(id),
                    Some(mapping_idx),
                    Some(range_min),
                    Some(range_max),
                ) = (
                    map_usize(map, "track"),
                    map_usize(map, "id")
                        .and_then(sequencer::sequencer::RackMacroId::from_index),
                    map_usize(map, "mapping-idx"),
                    map_number(map, "min").map(|value| value as f32),
                    map_number(map, "max").map(|value| value as f32),
                ) {
                    app.set_rack_macro_mapping_range(
                        track,
                        id,
                        mapping_idx,
                        range_min,
                        range_max,
                    );
                    refresh_instrument_panel_reactive(
                        &mut editor,
                        &app,
                        track,
                        &selected_steps,
                        &ui_epoch,
                    );
                }
            }
        }
        "set-rack-macro-curve" => {
            if let Value::Map(ref map) = payload {
                let curve =
                    map_string(map, "curve").and_then(|curve| match curve.as_str() {
                        "linear" => Some(sequencer::sequencer::RackMacroCurve::Linear),
                        "exp" => Some(sequencer::sequencer::RackMacroCurve::Exp),
                        "log" => Some(sequencer::sequencer::RackMacroCurve::Log),
                        _ => None,
                    });
                if let (Some(track), Some(id), Some(mapping_idx), Some(curve)) = (
                    map_usize(map, "track"),
                    map_usize(map, "id")
                        .and_then(sequencer::sequencer::RackMacroId::from_index),
                    map_usize(map, "mapping-idx"),
                    curve,
                ) {
                    app.set_rack_macro_mapping_curve(track, id, mapping_idx, curve);
                    refresh_instrument_panel_reactive(
                        &mut editor,
                        &app,
                        track,
                        &selected_steps,
                        &ui_epoch,
                    );
                }
            }
        }
        "set-rack-slot-instrument-param-batch"
        | "set-rack-slot-instrument-plock-batch" => {
            if let Value::Map(ref map) = payload {
                let track = map_usize(map, "track");
                let slot_idx = map_usize(map, "slot");
                let updates = map_param_updates(map);
                if let (Some(track), Some(slot_idx), Some(updates)) =
                    (track, slot_idx, updates)
                {
                    let steps = map_usize_list(map, "steps").unwrap_or_else(|| {
                        selected_steps
                            .lock()
                            .unwrap()
                            .iter()
                            .copied()
                            .collect::<Vec<_>>()
                    });
                    let commands = rack_slot_snapshot_for_host(&state, track, slot_idx)
                        .and_then(|slot| app.rack_slot_instrument_descriptor(&slot))
                        .map(|descriptor| {
                            updates.into_iter().filter_map(|(param_idx, user_value)| {
                                let param = descriptor.params.get(param_idx)?;
                                let value = param.clamp(param.user_input_to_stored(user_value));
                                Some(if name == "set-rack-slot-instrument-plock-batch" {
                                    app::AppCommand::SetRackSlotInstrumentPlockMulti {
                                        track,
                                        slot_idx,
                                        steps: steps.clone(),
                                        param_idx,
                                        value,
                                    }
                                } else {
                                    app::AppCommand::SetRackSlotInstrumentParam {
                                        track,
                                        slot_idx,
                                        param_idx,
                                        value,
                                    }
                                })
                            }).collect::<Vec<_>>()
                        })
                        .unwrap_or_default();
                    let gesture = map_string(map, "gesture")
                        .unwrap_or_else(|| "rack-instrument".to_string());
                    let label = map_string(map, "label")
                        .unwrap_or_else(|| "Set rack instrument parameters".to_string());
                    if name == "set-rack-slot-instrument-param-batch" {
                        let targets = commands
                            .iter()
                            .filter_map(|command| match command {
                                app::AppCommand::SetRackSlotInstrumentParam {
                                    slot_idx,
                                    param_idx,
                                    value,
                                    ..
                                } => Some((PrintTarget::RackSlotInstrument {
                                    slot_idx: *slot_idx,
                                    param_idx: *param_idx,
                                }, *value)),
                                _ => None,
                            })
                            .collect::<Vec<_>>();
                        if try_latch_param_print(ctx.shared, &mut editor, &app, track, &targets) {
                            return;
                        }
                    }
                    let result = if name == "set-rack-slot-instrument-plock-batch" {
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
                            &gesture,
                            &label,
                        )
                    };
                    match result {
                        Ok(_) => {
                            ctx.gesture.rack_control_snapshot_dirty = true;
                            refresh_instrument_panel_reactive(
                                &mut editor,
                                &app,
                                track,
                                &selected_steps,
                                &ui_epoch,
                            );
                        }
                        Err(error) => editor.handle_host_event(HostEvent::Error(
                            format!("rack instrument parameter batch failed: {error:?}"),
                        )),
                    }
                }
            }
        }
        "set-rack-slot-instrument-param" => {
            if let Value::Map(ref map) = payload {
                let track = map_usize(map, "track");
                let slot_idx = map_usize(map, "slot");
                let param_idx = map_usize(map, "param-idx");
                let value = map_number(map, "value").map(|value| value as f32);
                if let (Some(track), Some(slot_idx), Some(param_idx), Some(user_val)) =
                    (track, slot_idx, param_idx, value)
                {
                    if let Some(slot) =
                        rack_slot_snapshot_for_host(&state, track, slot_idx)
                    {
                        if let Some(desc) = app
                            .rack_slot_instrument_descriptor(&slot)
                            .and_then(|desc| desc.params.get(param_idx).cloned())
                        {
                            let stored =
                                desc.clamp(desc.user_input_to_stored(user_val));
                            if try_latch_param_print(
                                ctx.shared,
                                &mut editor,
                                &app,
                                track,
                                &[(PrintTarget::RackSlotInstrument {
                                    slot_idx,
                                    param_idx,
                                }, stored)],
                            ) {
                                return;
                            }
                            app::apply_command(
                                &mut app,
                                app::AppCommand::SetRackSlotInstrumentParam {
                                    track,
                                    slot_idx,
                                    param_idx,
                                    value: stored,
                                },
                            );
                            ctx.gesture.rack_control_snapshot_dirty = true;
                            if param_change_needs_fx_rebuild(&desc) {
                                refresh_instrument_panel_reactive(
                                    &mut editor,
                                    &app,
                                    track,
                                    &selected_steps,
                                    &ui_epoch,
                                );
                            } else {
                                refresh_rack_direct_param_reactive(
                                    &mut editor,
                                    &app,
                                    &state,
                                    track,
                                    RackDirectDisplayTarget::InstrumentParam {
                                        slot_idx,
                                        param_idx,
                                    },
                                    &selected_steps,
                                    false,
                                    &ui_epoch,
                                );
                            }
                        }
                    }
                }
            }
        }
        "set-rack-slot-instrument-plock" => {
            if let Value::Map(ref map) = payload {
                let track = map_usize(map, "track");
                let slot_idx = map_usize(map, "slot");
                let param_idx = map_usize(map, "param-idx");
                let value = map_number(map, "value").map(|value| value as f32);
                if let (Some(track), Some(slot_idx), Some(param_idx), Some(user_val)) =
                    (track, slot_idx, param_idx, value)
                {
                    if let Some(slot) =
                        rack_slot_snapshot_for_host(&state, track, slot_idx)
                    {
                        if let Some(desc) = app
                            .rack_slot_instrument_descriptor(&slot)
                            .and_then(|desc| desc.params.get(param_idx).cloned())
                        {
                            let stored =
                                desc.clamp(desc.user_input_to_stored(user_val));
                            let steps: Vec<usize> = selected_steps
                                .lock()
                                .unwrap()
                                .iter()
                                .copied()
                                .collect();
                            app::apply_command(
                                &mut app,
                                app::AppCommand::SetRackSlotInstrumentPlockMulti {
                                    track,
                                    slot_idx,
                                    steps,
                                    param_idx,
                                    value: stored,
                                },
                            );
                            ctx.gesture.rack_control_snapshot_dirty = true;
                            if param_change_needs_fx_rebuild(&desc) {
                                sync_rack_slot_instrument_authoring_display(
                                    &mut editor,
                                    &app,
                                    &state,
                                    track,
                                    &selected_steps,
                                );
                                ui_epoch.fetch_add(1, Ordering::Relaxed);
                            } else {
                                refresh_rack_direct_param_reactive(
                                    &mut editor,
                                    &app,
                                    &state,
                                    track,
                                    RackDirectDisplayTarget::InstrumentParam {
                                        slot_idx,
                                        param_idx,
                                    },
                                    &selected_steps,
                                    true,
                                    &ui_epoch,
                                );
                            }
                            fx_epoch.fetch_add(1, Ordering::Relaxed);
                        }
                    }
                }
            }
        }
        "toggle-rack-slot-instrument-param" => {
            if let Value::Map(ref map) = payload {
                let track = map_usize(map, "track");
                let slot_idx = map_usize(map, "slot");
                let param_idx = map_usize(map, "param-idx");
                if let (Some(track), Some(slot_idx), Some(param_idx)) =
                    (track, slot_idx, param_idx)
                {
                    if let Some(slot) =
                        rack_slot_snapshot_for_host(&state, track, slot_idx)
                    {
                        if let Some(desc) = app
                            .rack_slot_instrument_descriptor(&slot)
                            .and_then(|desc| desc.params.get(param_idx).cloned())
                        {
                            let current = slot
                                .instrument_slot
                                .defaults
                                .get(param_idx)
                                .copied()
                                .unwrap_or(desc.default);
                            let next =
                                desc.clamp(if current > 0.5 { 0.0 } else { 1.0 });
                            if try_latch_param_print(
                                ctx.shared,
                                &mut editor,
                                &app,
                                track,
                                &[(PrintTarget::RackSlotInstrument {
                                    slot_idx,
                                    param_idx,
                                }, next)],
                            ) {
                                return;
                            }
                            app::apply_command(
                                &mut app,
                                app::AppCommand::SetRackSlotInstrumentParam {
                                    track,
                                    slot_idx,
                                    param_idx,
                                    value: next,
                                },
                            );
                            refresh_instrument_panel_reactive(
                                &mut editor,
                                &app,
                                track,
                                &selected_steps,
                                &ui_epoch,
                            );
                        }
                    }
                }
            }
        }
        "toggle-rack-slot-instrument-plock" => {
            if let Value::Map(ref map) = payload {
                let track = map_usize(map, "track");
                let slot_idx = map_usize(map, "slot");
                let param_idx = map_usize(map, "param-idx");
                if let (Some(track), Some(slot_idx), Some(param_idx)) =
                    (track, slot_idx, param_idx)
                {
                    if let Some(slot) =
                        rack_slot_snapshot_for_host(&state, track, slot_idx)
                    {
                        if let Some(desc) = app
                            .rack_slot_instrument_descriptor(&slot)
                            .and_then(|desc| desc.params.get(param_idx).cloned())
                        {
                            let selected: Vec<usize> = selected_steps
                                .lock()
                                .unwrap()
                                .iter()
                                .copied()
                                .collect();
                            let default = slot
                                .instrument_slot
                                .defaults
                                .get(param_idx)
                                .copied()
                                .unwrap_or(desc.default);
                            let current = selected
                                .iter()
                                .copied()
                                .min()
                                .and_then(|step| {
                                    slot.instrument_slot
                                        .plocks
                                        .get(step)
                                        .and_then(|step_plocks| {
                                            step_plocks.get(param_idx)
                                        })
                                        .copied()
                                        .flatten()
                                })
                                .unwrap_or(default);
                            let next =
                                desc.clamp(if current > 0.5 { 0.0 } else { 1.0 });
                            app::apply_command(
                                &mut app,
                                app::AppCommand::SetRackSlotInstrumentPlockMulti {
                                    track,
                                    slot_idx,
                                    steps: selected,
                                    param_idx,
                                    value: next,
                                },
                            );
                            sync_rack_slot_instrument_authoring_display(
                                &mut editor,
                                &app,
                                &state,
                                track,
                                &selected_steps,
                            );
                            fx_epoch.fetch_add(1, Ordering::Relaxed);
                            ui_epoch.fetch_add(1, Ordering::Relaxed);
                        }
                    }
                }
            }
        }
        "set-rack-slot-instrument-param-option" => {
            if let Value::Map(ref map) = payload {
                let track = map_usize(map, "track");
                let slot_idx = map_usize(map, "slot");
                let param_idx = map_usize(map, "param-idx");
                let label = map_string(map, "label");
                if let (Some(track), Some(slot_idx), Some(param_idx), Some(label)) =
                    (track, slot_idx, param_idx, label)
                {
                    if let Some(slot) =
                        rack_slot_snapshot_for_host(&state, track, slot_idx)
                    {
                        if let Some(sequencer::effects::ParamKind::Enum { labels }) =
                            app.rack_slot_instrument_descriptor(&slot).and_then(
                                |desc| {
                                    desc.params
                                        .get(param_idx)
                                        .map(|param| param.kind.clone())
                                },
                            )
                        {
                            if let Some(selected_idx) =
                                labels.iter().position(|item| item == &label)
                            {
                                let value = selected_idx as f32;
                                if try_latch_param_print(
                                    ctx.shared,
                                    &mut editor,
                                    &app,
                                    track,
                                    &[(PrintTarget::RackSlotInstrument {
                                        slot_idx,
                                        param_idx,
                                    }, value)],
                                ) {
                                    return;
                                }
                                app::apply_command(
                                    &mut app,
                                    app::AppCommand::SetRackSlotInstrumentParam {
                                        track,
                                        slot_idx,
                                        param_idx,
                                        value,
                                    },
                                );
                                refresh_instrument_panel_reactive(
                                    &mut editor,
                                    &app,
                                    track,
                                    &selected_steps,
                                    &ui_epoch,
                                );
                            }
                        }
                    }
                }
            }
        }
        "set-rack-slot-instrument-plock-option" => {
            if let Value::Map(ref map) = payload {
                let track = map_usize(map, "track");
                let slot_idx = map_usize(map, "slot");
                let param_idx = map_usize(map, "param-idx");
                let label = map_string(map, "label");
                if let (Some(track), Some(slot_idx), Some(param_idx), Some(label)) =
                    (track, slot_idx, param_idx, label)
                {
                    if let Some(slot) =
                        rack_slot_snapshot_for_host(&state, track, slot_idx)
                    {
                        if let Some(sequencer::effects::ParamKind::Enum { labels }) =
                            app.rack_slot_instrument_descriptor(&slot).and_then(
                                |desc| {
                                    desc.params
                                        .get(param_idx)
                                        .map(|param| param.kind.clone())
                                },
                            )
                        {
                            if let Some(selected_idx) =
                                labels.iter().position(|item| item == &label)
                            {
                                let steps: Vec<usize> = selected_steps
                                    .lock()
                                    .unwrap()
                                    .iter()
                                    .copied()
                                    .collect();
                                app::apply_command(
                                    &mut app,
                                    app::AppCommand::SetRackSlotInstrumentPlockMulti {
                                        track,
                                        slot_idx,
                                        steps,
                                        param_idx,
                                        value: selected_idx as f32,
                                    },
                                );
                                sync_rack_slot_instrument_authoring_display(
                                    &mut editor,
                                    &app,
                                    &state,
                                    track,
                                    &selected_steps,
                                );
                                fx_epoch.fetch_add(1, Ordering::Relaxed);
                                ui_epoch.fetch_add(1, Ordering::Relaxed);
                            }
                        }
                    }
                }
            }
        }
        "set-instrument-param-batch"
        | "set-instrument-plock-batch"
        | "set-instrument-key-lock-batch" => {
            if let Value::Map(ref map) = payload {
                if let Some(updates) = map_param_updates(map) {
                    let track = map_usize(map, "track")
                        .unwrap_or_else(|| current_track.load(Ordering::Relaxed));
                    let mut commands = Vec::with_capacity(updates.len());
                    let steps = map_usize_list(map, "steps").unwrap_or_else(|| {
                        selected_steps
                            .lock()
                            .unwrap()
                            .iter()
                            .copied()
                            .collect::<Vec<_>>()
                    });
                    let notes = map_u8_list(map, "notes").unwrap_or_default();
                    for (param_idx, user_value) in updates {
                        if let Some(desc) = app
                            .graph
                            .instrument_descriptors
                            .get(track)
                            .and_then(|descriptor| descriptor.params.get(param_idx))
                        {
                            let value = desc.clamp(desc.user_input_to_stored(user_value));
                            commands.push(if name == "set-instrument-plock-batch" {
                                app::AppCommand::SetInstrumentPlockMulti {
                                    track,
                                    steps: steps.clone(),
                                    param_idx,
                                    value,
                                }
                            } else if name == "set-instrument-key-lock-batch" {
                                app::AppCommand::SetInstrumentKeyLockMulti {
                                    track,
                                    notes: notes.clone(),
                                    param_idx,
                                    value,
                                }
                            } else {
                                app::AppCommand::SetInstrumentParam {
                                    track,
                                    param_idx,
                                    value,
                                }
                            });
                        }
                    }
                    let result = if name == "set-instrument-plock-batch" {
                        let gesture = map_string(map, "gesture")
                            .unwrap_or_else(|| "instrument-envelope".to_string());
                        let label = map_string(map, "label")
                            .unwrap_or_else(|| "Set instrument envelope".to_string());
                        app::edit::apply_coalesced_device_plock_batch(
                            &mut app,
                            &commands,
                            &gesture,
                            &label,
                        )
                    } else {
                        let gesture = map_string(map, "gesture")
                            .unwrap_or_else(|| "instrument-envelope".to_string());
                        let label = map_string(map, "label")
                            .unwrap_or_else(|| "Set instrument envelope".to_string());
                        app::edit::apply_coalesced_device_value_batch(
                            &mut app,
                            &commands,
                            &gesture,
                            &label,
                        )
                    };
                    if result.is_ok() {
                        let plocks_changed = name == "set-instrument-plock-batch";
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
                                app::AppCommand::SetInstrumentParam { param_idx, .. }
                                | app::AppCommand::SetInstrumentPlockMulti {
                                    param_idx, ..
                                }
                                | app::AppCommand::SetInstrumentKeyLockMulti {
                                    param_idx, ..
                                } => Some(*param_idx),
                                _ => None,
                            })
                            .collect::<Vec<_>>();
                        let neural_selection =
                            selected_neural_neurons.lock().unwrap().clone();
                        sync_instrument_param_batch_display(
                            &mut editor,
                            &app,
                            &state,
                            &selected_steps,
                            &neural_selection,
                            track,
                            current_track.load(Ordering::Relaxed),
                            &param_indices,
                            display_step,
                            plocks_changed,
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
                            format!("instrument parameter batch failed: {error:?}"),
                        )),
                    }
                }
            }
        }
        _ => {}
    }
}

fn try_latch_rack_slot_param_print(
    shared: &SharedHandles,
    editor: &mut Editor,
    app: &app::App,
    track: usize,
    slot_idx: usize,
    param: RackSlotParam,
    value: f32,
) -> bool {
    try_latch_param_print(
        shared,
        editor,
        app,
        track,
        &[(PrintTarget::RackSlotParam { slot_idx, param }, param.clamp(value))],
    )
}

fn rack_slot_effect_print_value(
    app: &app::App,
    track: usize,
    rack_slot_idx: usize,
    effect_slot_idx: usize,
    param_idx: usize,
    value: f32,
) -> Option<f32> {
    app.rack_slot_effect_snapshot(track, rack_slot_idx)
        .ok()
        .and_then(|slot| slot.effect_descriptors.get(effect_slot_idx).cloned())
        .and_then(|descriptor| descriptor.params.get(param_idx).cloned())
        .filter(|param| {
            !matches!(
                param.host_control,
                Some(sequencer::effects::HostControl::FxSidechain { .. })
            ) && !sequencer::instruments::voice_modulator::is_envelope_source_param_value(
                param.node_param_idx,
                value,
            )
        })
        .map(|param| param.clamp(value))
}
