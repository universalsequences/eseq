use crate::*;

pub(super) const COMMANDS: &[&str] = &[
    "midi-fx-history-action",
    "process-history-action",
    "piano-roll-gesture-update",
    "piano-roll-gesture-finish",
    "piano-roll-history-action",
    "delete-selected-steps",
    "paste-steps",
    "set-step-param-history",
    "print-step-param",
    "print-step-param-release",
    "move-step-history",
    "slice2-history-action",
    "resize-drum-rack-patterns",
    "slice3-history-action",
    "bus-mixer-history-action",
    "toggle-step",
    "set-track-plock-entry",
    "set-track-plock-entry-option",
    "clear-track-plock-entry",
    "preview-plock-variant",
    "stamp-plock-variant",
    "clear-step-variant-locks",
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
    let piano_roll_move_state = ctx.shared.piano_roll_move_state.clone();
    let step_clipboard = ctx.shared.step_clipboard.clone();
    let ui_epoch = ctx.shared.ui_epoch.clone();
    let fx_epoch = ctx.shared.fx_epoch.clone();
    let ui_invalidations = ctx.shared.ui_invalidations.clone();
    let auto_follow_override_until = ctx.shared.auto_follow_override_until.clone();
    let bus_state = ctx.shared.bus_state.clone();
    let piano_roll_clipboard = ctx.shared.piano_roll_clipboard.clone();
    match name {
        "midi-fx-history-action" => {
            let Value::Map(ref map) = payload else {
                editor.handle_host_event(HostEvent::Error(
                    "MIDI FX edit failed: invalid payload".to_string(),
                ));
                return;
            };
            let field = |name: &str| map.get(name).map(|cell| cell.borrow().clone());
            let op = field("op").and_then(|value| match value {
                Value::Keyword(value) | Value::String(value) | Value::Symbol(value) => Some(value),
                _ => None,
            });
            let track = field("track").and_then(|value| match value {
                Value::Number(value) if value >= 0.0 => Some(value as usize),
                _ => None,
            });
            let (Some(op), Some(track)) = (op, track) else {
                editor.handle_host_event(HostEvent::Error(
                    "MIDI FX edit failed: missing target".to_string(),
                ));
                return;
            };
            enum MidiFxHistoryMutation {
                Chain(Vec<String>),
                Position(sequencer::sequencer::MidiFxPosition),
            }
            let mutation = match op.as_str() {
                "set-chain" => match field("value") {
                    Some(Value::List(values)) => {
                        let chain = values.into_iter().map(|value| {
                            match &*value.borrow() {
                                Value::String(name) => Ok(name.clone()),
                                _ => Err("MIDI FX chain contains a non-string name".to_string()),
                            }
                        }).collect::<Result<Vec<_>, _>>();
                        match chain {
                            Ok(chain) => MidiFxHistoryMutation::Chain(chain),
                            Err(error) => {
                                editor.handle_host_event(HostEvent::Error(error));
                                return;
                            }
                        }
                    }
                    _ => {
                        editor.handle_host_event(HostEvent::Error(
                            "MIDI FX chain is missing".to_string(),
                        ));
                        return;
                    }
                },
                "set-position" => match field("value") {
                    Some(Value::Keyword(value)) | Some(Value::String(value))
                        if value == "post-accumulator" =>
                    {
                        MidiFxHistoryMutation::Position(
                            sequencer::sequencer::MidiFxPosition::PostAccumulator,
                        )
                    }
                    Some(Value::Keyword(value)) | Some(Value::String(value))
                        if value == "pre-accumulator" =>
                    {
                        MidiFxHistoryMutation::Position(
                            sequencer::sequencer::MidiFxPosition::PreAccumulator,
                        )
                    }
                    _ => {
                        editor.handle_host_event(HostEvent::Error(
                            "MIDI FX position is invalid".to_string(),
                        ));
                        return;
                    }
                },
                _ => {
                    editor.handle_host_event(HostEvent::Error(format!(
                        "Unknown MIDI FX edit {op}"
                    )));
                    return;
                }
            };
            let Some(params) = app.state.pattern.track_params.get(track) else {
                editor.handle_host_event(HostEvent::Error(
                    "MIDI FX track no longer exists".to_string(),
                ));
                return;
            };
            let unchanged = match &mutation {
                MidiFxHistoryMutation::Chain(chain) => params.midi_fx_chain() == *chain,
                MidiFxHistoryMutation::Position(position) => {
                    params.get_midi_fx_position() == *position
                }
            };
            if unchanged {
                return;
            }
            let result = app.apply_recorded_scene_structure_mutation(
                "Edit MIDI FX routing",
                |app| {
                    let params = app.state.pattern.track_params.get(track)
                        .ok_or_else(|| "MIDI FX track no longer exists".to_string())?;
                    match mutation {
                        MidiFxHistoryMutation::Chain(chain) => {
                            params.set_midi_fx_chain(chain)
                        }
                        MidiFxHistoryMutation::Position(position) => {
                            params.set_midi_fx_position(position)
                        }
                    }
                    Ok(())
                },
            );
            match result {
                Ok(()) => {
                    state.publish_scheduler_snapshot();
                    ui_epoch.fetch_add(1, Ordering::Relaxed);
                }
                Err(error) => editor.handle_host_event(HostEvent::Error(format!(
                    "MIDI FX edit failed: {error}"
                ))),
            }
        }
        "process-history-action" => {
            let Value::Map(ref map) = payload else {
                editor.handle_host_event(HostEvent::Status(
                    "Process edit failed: invalid payload".to_string(),
                ));
                return;
            };
            let field = |name: &str| map.get(name).map(|cell| cell.borrow().clone());
            let op = field("op").and_then(|value| match value {
                Value::Keyword(value) | Value::String(value) | Value::Symbol(value) => Some(value),
                _ => None,
            });
            let track = field("track").and_then(|value| match value {
                Value::Number(value) if value >= 0.0 => Some(value as usize),
                _ => None,
            });
            let instance_id = field("instance-id").and_then(|value| match value {
                Value::Number(value) if value >= 0.0 => {
                    Some(sequencer::process::ProcessInstanceId(value as u64))
                }
                _ => None,
            });
            let (Some(op), Some(track), Some(instance_id)) =
                (op, track, instance_id)
            else {
                editor.handle_host_event(HostEvent::Status(
                    "Process edit failed: missing operation target".to_string(),
                ));
                return;
            };
            let result = app.apply_recorded_scene_structure_mutation(
                "Edit process chain",
                |app| {
                    let changed = match op.as_str() {
                        "set-lane-step" => {
                            let inlet = field("inlet").and_then(|value| match value {
                                Value::String(value) => Some(value),
                                _ => None,
                            }).ok_or_else(|| "Process lane inlet is missing".to_string())?;
                            let step = field("step").and_then(|value| match value {
                                Value::Number(value) if value >= 0.0 => Some(value as usize),
                                _ => None,
                            }).ok_or_else(|| "Process lane step is missing".to_string())?;
                            let value = field("value").and_then(|value| match value {
                                Value::Number(value) => Some(value as f32),
                                _ => None,
                            }).ok_or_else(|| "Process lane value is missing".to_string())?;
                            app.state.set_process_lane_value(
                                track, instance_id, inlet, step, value,
                            )
                        }
                        "clear-project-lane-override" => {
                            let inlet = field("inlet").and_then(|value| match value {
                                Value::String(value) => Some(value),
                                _ => None,
                            }).ok_or_else(|| "Process lane inlet is missing".to_string())?;
                            app.state.clear_project_process_lane_override(
                                track, instance_id, &inlet,
                            )
                        }
                        "set-inlet" => {
                            let inlet = field("inlet").and_then(|value| match value {
                                Value::String(value) => Some(value),
                                _ => None,
                            }).ok_or_else(|| "Process inlet is missing".to_string())?;
                            let literal = match field("value")
                                .ok_or_else(|| "Process inlet value is missing".to_string())?
                            {
                                Value::Number(value) => sequencer::process::ProcessLiteral::Number(value),
                                Value::Bool(value) => sequencer::process::ProcessLiteral::Bool(value),
                                Value::String(value) => sequencer::process::ProcessLiteral::String(value),
                                Value::Keyword(value) => sequencer::process::ProcessLiteral::Keyword(value),
                                Value::Symbol(value) => sequencer::process::ProcessLiteral::Symbol(value),
                                Value::Nil => sequencer::process::ProcessLiteral::Nil,
                                _ => return Err("Unsupported process inlet literal".to_string()),
                            };
                            app.state.set_track_process_inlet_value(
                                track, instance_id, &inlet, literal,
                            )
                        }
                        "set-enabled" => {
                            let enabled = field("enabled").and_then(|value| match value {
                                Value::Bool(value) => Some(value),
                                _ => None,
                            }).ok_or_else(|| "Process enabled state is missing".to_string())?;
                            app.state.set_track_process_slot_enabled(
                                track, instance_id, enabled,
                            )
                        }
                        "move-slot" => {
                            let before = match field("before-instance-id") {
                                Some(Value::Number(value)) if value >= 0.0 => {
                                    Some(sequencer::process::ProcessInstanceId(value as u64))
                                }
                                Some(Value::Nil) | None => None,
                                _ => return Err("Process move target is invalid".to_string()),
                            };
                            app.state.move_track_process_slot_before(
                                track, instance_id, before,
                            )
                        }
                        "remove-slot" => app.state.remove_track_process_slot(
                            track, instance_id,
                        ),
                        "bind-port" => {
                            let port = field("port").and_then(|value| match value {
                                Value::String(value) => Some(value),
                                _ => None,
                            }).ok_or_else(|| "Process port is missing".to_string())?;
                            let target = field("target")
                                .ok_or_else(|| "Process binding target is missing".to_string())?;
                            let target = natives::param_target_from_value(
                                &app.state, track, &target,
                            )?;
                            app.state.set_process_port_binding(
                                track, instance_id, &port, target,
                            )
                        }
                        "clear-port-binding" => {
                            let port = field("port").and_then(|value| match value {
                                Value::String(value) => Some(value),
                                _ => None,
                            }).ok_or_else(|| "Process port is missing".to_string())?;
                            app.state.clear_process_port_binding(
                                track, instance_id, &port,
                            )
                        }
                        _ => return Err(format!("Unknown process history operation {op}")),
                    };
                    changed.then_some(()).ok_or_else(|| {
                        "Process edit target was missing or unchanged".to_string()
                    })
                },
            );
            match result {
                Ok(()) => ui_invalidations.push(UiInvalidation::ProcessChain { track }),
                Err(error) => editor.handle_host_event(HostEvent::Status(format!(
                    "Process edit failed: {error}"
                ))),
            }
        }
        "piano-roll-gesture-update" => {
            match apply_piano_roll_gesture_update(
                &mut app,
                &piano_roll_selection,
                &piano_roll_move_state,
                &mut ctx.gesture.piano_roll_history_gesture,
                &payload,
            ) {
                Ok((status, track)) => {
                    *auto_follow_override_until.lock().unwrap() =
                        Some(Instant::now() + AUTO_FOLLOW_COOLDOWN);
                    ui_invalidations.push(UiInvalidation::PianoRoll {
                        track,
                        change: PianoRollInvalidation::Items,
                    });
                    ui_epoch.fetch_add(1, Ordering::Relaxed);
                    editor.show_transient_message(status);
                }
                Err(error) => editor.handle_host_event(HostEvent::Error(error)),
            }
        }
        "piano-roll-gesture-finish" => {
            match finish_piano_roll_gesture(
                &mut app,
                &piano_roll_move_state,
                &mut ctx.gesture.piano_roll_history_gesture,
                &payload,
            ) {
                Ok((app::edit::EditOutcome::Applied(result), track)) => {
                    ui_invalidations.push(UiInvalidation::PianoRoll {
                        track,
                        change: PianoRollInvalidation::Items,
                    });
                    fx_epoch.fetch_add(1, Ordering::Relaxed);
                    ui_epoch.fetch_add(1, Ordering::Relaxed);
                    editor.show_transient_message(result.label);
                }
                Ok((app::edit::EditOutcome::NoOp, _)) => {}
                Ok((app::edit::EditOutcome::AppliedUnrecorded, _)) => {
                    editor.handle_host_event(HostEvent::Error(
                        "Piano-roll gesture was applied without history".to_string(),
                    ));
                }
                Err(error) => editor.handle_host_event(HostEvent::Error(error)),
            }
        }
        "piano-roll-history-action" => {
            match apply_piano_roll_history_host_command(
                &mut app,
                &piano_roll_selection,
                &piano_roll_move_state,
                &piano_roll_clipboard,
                &payload,
            ) {
                Ok((outcome, status, track)) => {
                    if matches!(outcome, app::edit::EditOutcome::Applied(_)) {
                        *auto_follow_override_until.lock().unwrap() =
                            Some(Instant::now() + AUTO_FOLLOW_COOLDOWN);
                        ui_invalidations.push(UiInvalidation::PianoRoll {
                            track,
                            change: PianoRollInvalidation::Items,
                        });
                        fx_epoch.fetch_add(1, Ordering::Relaxed);
                        ui_epoch.fetch_add(1, Ordering::Relaxed);
                    }
                    editor.show_transient_message(status);
                }
                Err(error) => editor.handle_host_event(HostEvent::Error(error)),
            }
        }
        "delete-selected-steps" => {
            let track = match &payload {
                Value::Map(map) => map_usize(map, "track"),
                _ => None,
            };
            let Some(track) = track else {
                editor.handle_host_event(HostEvent::Error(
                    "Selected-step delete target was invalid".to_string(),
                ));
                return;
            };
            match apply_selected_steps_delete(
                &mut app,
                track,
                &selected_steps,
            ) {
                Ok((app::edit::EditOutcome::Applied(_), steps)) => {
                    *auto_follow_override_until.lock().unwrap() =
                        Some(Instant::now() + AUTO_FOLLOW_COOLDOWN);
                    // Only the deleted steps changed, so invalidate them as a
                    // batch (the same contract the step-move path relies on)
                    // instead of resyncing every visible track.
                    ui_invalidations.push(UiInvalidation::StepBatch {
                        track,
                        steps: steps.clone(),
                    });
                    ui_invalidations.push(UiInvalidation::StepSelection {
                        track,
                        changed_steps: steps,
                    });
                    fx_epoch.fetch_add(1, Ordering::Relaxed);
                    ui_epoch.fetch_add(1, Ordering::Relaxed);
                }
                Ok(_) => {}
                Err(error) => editor.handle_host_event(HostEvent::Error(error)),
            }
        }
        "paste-steps" => {
            match apply_step_paste_host_command(&mut app, &step_clipboard, &payload) {
                Ok((app::edit::EditOutcome::Applied(result), track)) => {
                    *auto_follow_override_until.lock().unwrap() =
                        Some(Instant::now() + AUTO_FOLLOW_COOLDOWN);
                    ui_invalidations.push(UiInvalidation::Pattern(
                        PatternInvalidation::WholeTrack { track },
                    ));
                    ui_epoch.fetch_add(1, Ordering::Relaxed);
                    editor.show_transient_message(result.label);
                }
                Ok((app::edit::EditOutcome::NoOp, _)) => {}
                Ok((app::edit::EditOutcome::AppliedUnrecorded, _)) => {
                    editor.handle_host_event(HostEvent::Error(
                        "Step paste was applied without history".to_string(),
                    ));
                }
                Err(error) => editor.handle_host_event(HostEvent::Error(error)),
            }
        }
        "print-step-param" => {
            // Live step-param printing (bead eseq-jc9): while playing with
            // record on, a *step*-buffer param touch latches into print mode;
            // the reactive tick writes the latched value onto passing trigger
            // steps. No cool-off / auto-follow override here — the performer
            // is watching the playhead, so follow must stay alive.
            if !state.is_playing() || !ctx.shared.recording.load(Ordering::Relaxed) {
                // The lisp-side gate raced off between the touch and this
                // dispatch: the payload is shaped exactly like a normal
                // cursor-step edit, so apply it as one.
                handle("set-step-param-history", payload, app, editor, ctx);
                return;
            }
            let Value::Map(ref map) = payload else {
                editor.handle_host_event(HostEvent::Error(
                    "Step print failed: invalid payload".to_string(),
                ));
                return;
            };
            let field = |name: &str| map.get(name).map(|cell| cell.borrow().clone());
            let track = field("track").and_then(|value| match value {
                Value::Number(track) => Some(track as usize),
                _ => None,
            });
            let param = field("param").and_then(|value| match value {
                Value::Keyword(name) => match name.as_str() {
                    "velocity" | "vel" => Some(StepParam::Velocity),
                    "duration" | "dur" => Some(StepParam::Duration),
                    "transpose" => Some(StepParam::Transpose),
                    _ => None,
                },
                _ => None,
            });
            let value = field("value").and_then(|value| match value {
                Value::Number(value) => Some(value as f32),
                _ => None,
            });
            let (Some(track), Some(param), Some(value)) = (track, param, value) else {
                editor.handle_host_event(HostEvent::Error(
                    "Step print failed: invalid payload".to_string(),
                ));
                return;
            };
            let mut print = ctx.shared.step_print.lock().unwrap();
            print.latch(track, param, value.clamp(param.min(), param.max()));
            // Engine-side substitution: the scheduler plays the latch on this
            // track from the next scheduled step, so the touch is audible
            // immediately — the pattern write only lands behind the playhead.
            print.publish_engine_override(&state);
        }
        "print-step-param-release" => {
            // Hold-to-print: the picker's mouse-up ends that param's print.
            // Runs unconditionally (not gated on play/record) so a gesture
            // that straddles a transport stop still cleans up its latch.
            let Value::Map(ref map) = payload else {
                return;
            };
            let param = map
                .get("param")
                .map(|cell| cell.borrow().clone())
                .and_then(|value| match value {
                    Value::Keyword(name) => match name.as_str() {
                        "velocity" | "vel" => Some(StepParam::Velocity),
                        "duration" | "dur" => Some(StepParam::Duration),
                        "transpose" => Some(StepParam::Transpose),
                        _ => None,
                    },
                    _ => None,
                });
            let Some(param) = param else {
                return;
            };
            let mut print = ctx.shared.step_print.lock().unwrap();
            let was_latched = print.armed();
            let ended = print.unlatch(param);
            print.publish_engine_override(&state);
            drop(print);
            if was_latched && ended {
                // The whole latch ended here (not in the tick's gate check),
                // so restore all picker readouts to the cursor step now.
                if crate::step_print::restore_cursor_display_fields(
                    editor.runtime_mut(),
                    &state,
                    current_track.load(Ordering::Relaxed),
                    &selected_steps,
                ) {
                    editor.refresh_visible_layouts_for_buffer_named("*step*");
                }
            } else if was_latched {
                // Other params are still held: hand only THIS param's picker
                // readout back to the cursor step. (When the whole latch
                // ends, the tick's disarm branch restores all three.)
                if let Some(field) = fx_step_param_value_field(param) {
                    let track = current_track.load(Ordering::Relaxed);
                    if track < state.pattern.step_data.len() {
                        let cursor = fx_step_cursor_from_runtime(editor.runtime());
                        let num_steps = state.pattern.track_params[track]
                            .get_num_steps()
                            .clamp(1, MAX_STEPS);
                        let value = state.pattern.step_data[track]
                            .get(cursor.min(num_steps.saturating_sub(1)), param);
                        editor
                            .runtime_mut()
                            .set_reactive("SEQ", field, Value::Number(value as f64));
                        editor.refresh_visible_layouts_for_buffer_named("*step*");
                    }
                }
            }
        }
        "set-step-param-history" => {
            match apply_step_param_history_host_command(&mut app, &payload) {
                Ok((app::edit::EditOutcome::Applied(result), track, steps, param)) => {
                    *auto_follow_override_until.lock().unwrap() =
                        Some(Instant::now() + AUTO_FOLLOW_COOLDOWN);
                    for step in steps {
                        ui_invalidations.push(UiInvalidation::Step {
                            track,
                            step,
                            change: StepInvalidation::Param(param.into()),
                        });
                        if param == StepParam::Duration {
                            ui_invalidations.push(UiInvalidation::Step {
                                track,
                                step,
                                change: StepInvalidation::DurationSpan,
                            });
                        }
                    }
                    // No ui_epoch bump: the targeted Step invalidations above
                    // are the complete UI path for a step-param edit (per-step
                    // slider/haptic fields, the flat + per-track param lists,
                    // the duration bar, the expanded-lane viewports, the
                    // *step* panel readouts and the piano roll — see
                    // `apply_ui_invalidations`). Bumping the epoch instead ran
                    // `sync_all_track_sequencer_state` + a whole-list
                    // `sync_step_param_lists` on EVERY drag update, which cost
                    // ~7ms of epoch sync plus (for velocity) ~4.4ms of
                    // reactive cycle per event.
                    editor.show_transient_message(result.label);
                }
                Ok((app::edit::EditOutcome::NoOp, ..)) => {}
                Ok((app::edit::EditOutcome::AppliedUnrecorded, ..)) => {
                    editor.handle_host_event(HostEvent::Error(
                        "Step parameter edit was applied without history".to_string(),
                    ));
                }
                Err(error) => editor.handle_host_event(HostEvent::Error(error)),
            }
        }
        "move-step-history" => {
            match apply_move_step_history_host_command(&mut app, &payload) {
                Ok((app::edit::EditOutcome::Applied(result), track, steps, affected_steps, delta, move_selection)) => {
                    let moved_steps = steps
                        .iter()
                        .map(|step| (*step as isize + delta) as usize)
                        .collect::<Vec<_>>();
                    let mut changed_selection = Vec::new();
                    if move_selection {
                        let mut selected = selected_steps.lock().unwrap();
                        let previous = selected.clone();
                        selected.clear();
                        selected.extend(moved_steps.iter().copied());
                        changed_selection = previous
                            .symmetric_difference(&selected)
                            .copied()
                            .collect();
                        changed_selection.sort_unstable();
                    }
                    *auto_follow_override_until.lock().unwrap() =
                        Some(Instant::now() + AUTO_FOLLOW_COOLDOWN);
                    ui_invalidations.push(UiInvalidation::StepBatch {
                        track,
                        steps: affected_steps,
                    });
                    if move_selection {
                        ui_invalidations.push(UiInvalidation::StepSelection {
                            track,
                            changed_steps: changed_selection,
                        });
                    }
                    editor.show_transient_message(result.label);
                }
                Ok((app::edit::EditOutcome::NoOp, ..)) => {}
                Ok((app::edit::EditOutcome::AppliedUnrecorded, ..)) => {
                    editor.handle_host_event(HostEvent::Error(
                        "Step move was applied without history".to_string(),
                    ));
                }
                Err(error) => editor.handle_host_event(HostEvent::Error(error)),
            }
        }
        "slice2-history-action" => {
            match apply_slice2_history_host_command(&mut app, &payload) {
                Ok((app::edit::EditOutcome::Applied(result), track)) => {
                    *auto_follow_override_until.lock().unwrap() =
                        Some(Instant::now() + AUTO_FOLLOW_COOLDOWN);
                    ui_invalidations.push(UiInvalidation::Pattern(
                        PatternInvalidation::WholeTrack { track },
                    ));
                    ui_epoch.fetch_add(1, Ordering::Relaxed);
                    editor.show_transient_message(result.label);
                }
                Ok((app::edit::EditOutcome::NoOp, _)) => {}
                Ok((app::edit::EditOutcome::AppliedUnrecorded, _)) => {
                    editor.handle_host_event(HostEvent::Error(
                        "Slice 2 edit was applied without history".to_string(),
                    ));
                }
                Err(error) => editor.handle_host_event(HostEvent::Error(error)),
            }
        }
        "resize-drum-rack-patterns" => {
            let Value::Map(map) = &payload else {
                editor.handle_host_event(HostEvent::Error(
                    "Drum rack pattern resize failed: invalid payload".to_string(),
                ));
                return;
            };
            let Some(bus_index) = map_usize(map, "bus") else {
                editor.handle_host_event(HostEvent::Error(
                    "Drum rack pattern resize failed: invalid bus".to_string(),
                ));
                return;
            };
            let Some(bus_id) = app.buses.get(bus_index).map(|bus| bus.id.0) else {
                editor.handle_host_event(HostEvent::Error(format!(
                    "Drum rack pattern resize failed: bus {bus_index} does not exist"
                )));
                return;
            };
            let Some(group) = app
                .groups
                .iter()
                .find(|group| group.bus_id == bus_id && group.rack.is_some())
            else {
                editor.handle_host_event(HostEvent::Error(
                    "Drum rack pattern resize failed: selected bus is not a drum rack"
                        .to_string(),
                ));
                return;
            };
            let group_id = group.id;
            let members = group.members.clone();
            let change = match map_string(map, "op").as_deref() {
                Some("double") => app::edit::PatternLengthChange::Double,
                Some("halve") => app::edit::PatternLengthChange::Halve,
                _ => {
                    editor.handle_host_event(HostEvent::Error(
                        "Drum rack pattern resize failed: invalid operation".to_string(),
                    ));
                    return;
                }
            };
            match app.resize_drum_rack_patterns_recorded(group_id, change) {
                Ok(app::edit::EditOutcome::Applied(result)) => {
                    *auto_follow_override_until.lock().unwrap() =
                        Some(Instant::now() + AUTO_FOLLOW_COOLDOWN);
                    for track in members {
                        ui_invalidations.push(UiInvalidation::Pattern(
                            PatternInvalidation::WholeTrack { track },
                        ));
                    }
                    ui_epoch.fetch_add(1, Ordering::Relaxed);
                    editor.show_transient_message(result.label);
                }
                Ok(app::edit::EditOutcome::NoOp) => {}
                Ok(app::edit::EditOutcome::AppliedUnrecorded) => {
                    editor.handle_host_event(HostEvent::Error(
                        "Drum rack resize was applied without history".to_string(),
                    ));
                }
                Err(error) => editor.handle_host_event(HostEvent::Error(format!(
                    "Could not resize drum rack patterns: {error:?}"
                ))),
            }
        }
        "slice3-history-action" => {
            match apply_slice3_history_host_command(&mut app, &payload) {
                Ok((app::edit::EditOutcome::Applied(result), track)) => {
                    *auto_follow_override_until.lock().unwrap() =
                        Some(Instant::now() + AUTO_FOLLOW_COOLDOWN);
                    match (track, slice3_track_mixer_invalidation(&payload)) {
                        (Some(track), Some(change)) => {
                            ui_invalidations
                                .push(UiInvalidation::TrackMixer { track, change });
                        }
                        (track, None) => {
                            if let Some(track) = track {
                                ui_invalidations.push(UiInvalidation::Pattern(
                                    PatternInvalidation::WholeTrack { track },
                                ));
                            }
                            ui_epoch.fetch_add(1, Ordering::Relaxed);
                        }
                        (None, Some(_)) => {
                            ui_epoch.fetch_add(1, Ordering::Relaxed);
                        }
                    }
                    editor.show_transient_message(result.label);
                }
                Ok((app::edit::EditOutcome::NoOp, _)) => {}
                Ok((app::edit::EditOutcome::AppliedUnrecorded, _)) => {
                    editor.handle_host_event(HostEvent::Error(
                        "Slice 3 edit was applied without history".to_string(),
                    ));
                }
                Err(error) => editor.handle_host_event(HostEvent::Error(error)),
            }
        }
        "bus-mixer-history-action" => {
            match apply_bus_mixer_history_host_command(&mut app, &payload) {
                Ok((app::edit::EditOutcome::Applied(result), bus)) => {
                    *bus_state.lock().unwrap() = app.buses.clone();
                    match bus_mixer_targeted_invalidation(&payload) {
                        Some(change) => {
                            ui_invalidations
                                .push(UiInvalidation::BusMixer { bus, change });
                        }
                        None => {
                            ui_invalidations.push(UiInvalidation::BusMixer {
                                bus,
                                change: BusMixerInvalidation::Volume,
                            });
                            ui_epoch.fetch_add(1, Ordering::Relaxed);
                        }
                    }
                    editor.show_transient_message(result.label);
                }
                Ok((app::edit::EditOutcome::NoOp, _)) => {}
                Ok((app::edit::EditOutcome::AppliedUnrecorded, _)) => {
                    editor.handle_host_event(HostEvent::Error(
                        "Bus mixer edit was applied without history".to_string(),
                    ));
                }
                Err(error) => editor.handle_host_event(HostEvent::Error(error)),
            }
        }
        "toggle-step" => {
            match apply_toggle_step_host_command(&mut app, &payload) {
                Ok((app::edit::EditOutcome::Applied(_), track, step)) => {
                    let mut selection = selected_steps.lock().unwrap();
                    if !selection.is_empty() {
                        selection.clear();
                        fx_epoch.fetch_add(1, Ordering::Relaxed);
                    }
                    drop(selection);
                    *auto_follow_override_until.lock().unwrap() =
                        Some(Instant::now() + AUTO_FOLLOW_COOLDOWN);
                    // The targeted Step invalidations were the
                    // complete pre-undo UI path for toggles; no
                    // ui_epoch bump so fast toggle-drags skip the
                    // full resync per step.
                    ui_invalidations.push(UiInvalidation::StepBatch {
                        track,
                        steps: vec![step],
                    });
                }
                Ok(_) => {}
                Err(error) => editor.handle_host_event(HostEvent::Error(error)),
            }
        }
        "set-track-plock-entry" => {
            if let Value::Map(ref map) = payload {
                let target = map.get("target").and_then(|cell| match &*cell.borrow() {
                    Value::String(s) => Some(s.clone()),
                    _ => None,
                });
                let step = map.get("step-idx").and_then(|cell| match &*cell.borrow() {
                    Value::Number(n) => Some(*n as usize),
                    _ => None,
                });
                let slot_idx =
                    map.get("slot-idx").and_then(|cell| match &*cell.borrow() {
                        Value::Number(n) => Some(*n as usize),
                        _ => None,
                    });
                let rack_slot =
                    map.get("rack-slot").and_then(|cell| match &*cell.borrow() {
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
                if let (Some(target), Some(step), Some(value)) = (target, step, value) {
                    let track = current_track.load(Ordering::Relaxed);
                    match target.as_str() {
                        "timebase" => {
                            let idx = (value.round() as usize)
                                .min(sequencer::sequencer::Timebase::ALL.len() - 1);
                            app::apply_command(
                                &mut app,
                                app::AppCommand::SetTimebasePlock {
                                    track,
                                    step,
                                    timebase: Some(
                                        sequencer::sequencer::Timebase::ALL[idx],
                                    ),
                                },
                            );
                        }
                        "swing" => {
                            app::apply_command(
                                &mut app,
                                app::AppCommand::SetTrackSwingPlock {
                                    track,
                                    step,
                                    value: Some(value),
                                },
                            );
                        }
                        "swing-resolution" => {
                            let idx = (value.round() as usize).min(
                                sequencer::sequencer::SwingResolution::ALL.len() - 1,
                            );
                            app::apply_command(
                                &mut app,
                                app::AppCommand::SetTrackSwingResolutionPlock {
                                    track,
                                    step,
                                    resolution: Some(
                                        sequencer::sequencer::SwingResolution::ALL[idx],
                                    ),
                                },
                            );
                        }
                        "bus-send" => {
                            if let Some(bus_idx) = param_idx {
                                if let Some(bus_id) = app.buses.get(bus_idx).map(|bus| bus.id) {
                                    app::apply_command(
                                        &mut app,
                                        app::AppCommand::SetTrackBusSendPlock {
                                            track,
                                            step,
                                            destination: bus_id,
                                            value: Some(value),
                                        },
                                    );
                                }
                            }
                        }
                        "step-param" => {
                            if let Some(param_idx) = param_idx {
                                if let Some(param) =
                                    sequencer::sequencer::StepParam::ALL.get(param_idx)
                                {
                                    app::apply_command(
                                        &mut app,
                                        app::AppCommand::SetStepParam {
                                            track,
                                            step,
                                            param: *param,
                                            value,
                                        },
                                    );
                                }
                            }
                        }
                        "instrument" => {
                            if let Some(param_idx) = param_idx {
                                if let Some(desc) = app
                                    .graph
                                    .instrument_descriptors
                                    .get(track)
                                    .and_then(|d| d.params.get(param_idx))
                                    .cloned()
                                {
                                    let stored =
                                        desc.clamp(desc.user_input_to_stored(value));
                                    app::apply_command(
                                        &mut app,
                                        app::AppCommand::SetInstrumentPlock {
                                            track,
                                            step,
                                            param_idx,
                                            value: stored,
                                        },
                                    );
                                }
                            }
                        }
                        "effect" => {
                            if let (Some(slot_idx), Some(param_idx)) =
                                (slot_idx, param_idx)
                            {
                                if let Some(_slot) = state
                                    .pattern
                                    .effect_chains
                                    .get(track)
                                    .and_then(|chain| chain.get(slot_idx))
                                {
                                    let clamped = app
                                        .graph
                                        .effect_descriptors
                                        .get(track)
                                        .and_then(|d| d.get(slot_idx))
                                        .and_then(|d| d.params.get(param_idx))
                                        .map(|p| value.clamp(p.min, p.max))
                                        .unwrap_or(value);
                                    app::apply_command(
                                        &mut app,
                                        app::AppCommand::SetEffectPlock {
                                            track,
                                            step,
                                            slot_idx,
                                            param_idx,
                                            value: clamped,
                                        },
                                    );
                                }
                            }
                        }
                        "rack-macro" => {
                            if let Some(param_idx) = param_idx {
                                if let Some(id) =
                                    sequencer::sequencer::RackMacroId::from_index(
                                        param_idx,
                                    )
                                {
                                    app::apply_command(
                                        &mut app,
                                        app::AppCommand::SetRackMacroPlockMulti {
                                            track,
                                            steps: vec![step],
                                            macro_idx: id.index(),
                                            value,
                                        },
                                    );
                                }
                            }
                        }
                        "rack-effect" => {
                            if let (
                                Some(rack_slot),
                                Some(effect_slot),
                                Some(param_idx),
                            ) = (rack_slot, slot_idx, param_idx)
                            {
                                app::apply_command(
                                    &mut app,
                                    app::AppCommand::SetRackSlotEffectPlockMulti {
                                        track,
                                        steps: vec![step],
                                        rack_slot_idx: rack_slot,
                                        effect_slot_idx: effect_slot,
                                        param_idx,
                                        value,
                                    },
                                );
                            }
                        }
                        "midi-fx" => {
                            if let (Some(slot_idx), Some(param_idx)) =
                                (slot_idx, param_idx)
                            {
                                if let Some(_slot) = state
                                    .pattern
                                    .midi_fx_slots
                                    .get(track)
                                    .and_then(|slots| slots.get(slot_idx))
                                {
                                    let chain = state.pattern.track_params[track]
                                        .midi_fx_chain();
                                    let clamped = chain
                                        .get(slot_idx)
                                        .and_then(|name| {
                                            sequencer::lisp_host::load_midi_fx_descriptor(name)
                                        })
                                        .and_then(|desc| {
                                            desc.params.get(param_idx).cloned()
                                        })
                                        .map(|p| value.clamp(p.min, p.max))
                                        .unwrap_or(value);
                                    app::apply_command(
                                        &mut app,
                                        app::AppCommand::SetMidiFxPlockMulti {
                                            track,
                                            steps: vec![step],
                                            slot_idx,
                                            param_idx,
                                            value: clamped,
                                        },
                                    );
                                }
                            }
                        }
                        _ => {}
                    }
                    fx_epoch.fetch_add(1, Ordering::Relaxed);
                    ui_epoch.fetch_add(1, Ordering::Relaxed);
                }
            }
        }
        "set-track-plock-entry-option" => {
            if let Value::Map(ref map) = payload {
                let target = map.get("target").and_then(|cell| match &*cell.borrow() {
                    Value::String(s) => Some(s.clone()),
                    _ => None,
                });
                let step = map.get("step-idx").and_then(|cell| match &*cell.borrow() {
                    Value::Number(n) => Some(*n as usize),
                    _ => None,
                });
                let slot_idx =
                    map.get("slot-idx").and_then(|cell| match &*cell.borrow() {
                        Value::Number(n) => Some(*n as usize),
                        _ => None,
                    });
                let rack_slot =
                    map.get("rack-slot").and_then(|cell| match &*cell.borrow() {
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
                if let (Some(target), Some(step), Some(label)) = (target, step, label) {
                    let track = current_track.load(Ordering::Relaxed);
                    match target.as_str() {
                        "timebase" => {
                            if let Some(idx) = sequencer::sequencer::Timebase::LABELS
                                .iter()
                                .position(|item| *item == label)
                            {
                                app::apply_command(
                                    &mut app,
                                    app::AppCommand::SetTimebasePlock {
                                        track,
                                        step,
                                        timebase: Some(
                                            sequencer::sequencer::Timebase::ALL[idx],
                                        ),
                                    },
                                );
                            }
                        }
                        "swing-resolution" => {
                            if let Some(idx) =
                                sequencer::sequencer::SwingResolution::LABELS
                                    .iter()
                                    .position(|item| *item == label)
                            {
                                app::apply_command(
                                    &mut app,
                                    app::AppCommand::SetTrackSwingResolutionPlock {
                                        track,
                                        step,
                                        resolution: Some(
                                            sequencer::sequencer::SwingResolution::ALL[idx],
                                        ),
                                    },
                                );
                            }
                        }
                        "step-param" => {}
                        "instrument" => {
                            if let Some(param_idx) = param_idx {
                                if let Some(selected_idx) = app
                                    .graph
                                    .instrument_descriptors
                                    .get(track)
                                    .and_then(|d| d.params.get(param_idx))
                                    .and_then(|p| match &p.kind {
                                        sequencer::effects::ParamKind::Enum {
                                            labels,
                                        } => labels
                                            .iter()
                                            .position(|item| item == &label),
                                        sequencer::effects::ParamKind::Boolean => {
                                            match label.as_str() {
                                                "on" | "ON" => Some(1),
                                                "off" | "OFF" => Some(0),
                                                _ => None,
                                            }
                                        }
                                        _ => None,
                                    })
                                {
                                    app::apply_command(
                                        &mut app,
                                        app::AppCommand::SetInstrumentPlock {
                                            track,
                                            step,
                                            param_idx,
                                            value: selected_idx as f32,
                                        },
                                    );
                                }
                            }
                        }
                        "effect" => {
                            if let (Some(slot_idx), Some(param_idx)) =
                                (slot_idx, param_idx)
                            {
                                if let Some(selected_idx) = app
                                    .graph
                                    .effect_descriptors
                                    .get(track)
                                    .and_then(|d| d.get(slot_idx))
                                    .and_then(|d| d.params.get(param_idx))
                                    .and_then(|p| match &p.kind {
                                        sequencer::effects::ParamKind::Enum {
                                            labels,
                                        } => labels
                                            .iter()
                                            .position(|item| item == &label),
                                        sequencer::effects::ParamKind::Boolean => {
                                            match label.as_str() {
                                                "on" | "ON" => Some(1),
                                                "off" | "OFF" => Some(0),
                                                _ => None,
                                            }
                                        }
                                        _ => None,
                                    })
                                {
                                    if let Some(_slot) = state
                                        .pattern
                                        .effect_chains
                                        .get(track)
                                        .and_then(|chain| chain.get(slot_idx))
                                    {
                                        app::apply_command(
                                            &mut app,
                                            app::AppCommand::SetEffectPlock {
                                                track,
                                                step,
                                                slot_idx,
                                                param_idx,
                                                value: selected_idx as f32,
                                            },
                                        );
                                    }
                                }
                            }
                        }
                        "rack-effect" => {
                            if let (
                                Some(rack_slot),
                                Some(effect_slot),
                                Some(param_idx),
                            ) = (rack_slot, slot_idx, param_idx)
                            {
                                match app.rack_slot_effect_option_value(
                                    track,
                                    rack_slot,
                                    effect_slot,
                                    param_idx,
                                    &label,
                                ) {
                                    Ok(value) => {
                                        app::apply_command(
                                            &mut app,
                                            app::AppCommand::SetRackSlotEffectPlockMulti {
                                                track,
                                                steps: vec![step],
                                                rack_slot_idx: rack_slot,
                                                effect_slot_idx: effect_slot,
                                                param_idx,
                                                value,
                                            },
                                        );
                                    }
                                    Err(error) => editor.handle_host_event(
                                        HostEvent::Status(format!(
                                            "Error editing rack-slot effect lock: {error}"
                                        )),
                                    ),
                                }
                            }
                        }
                        "midi-fx" => {
                            if let (Some(slot_idx), Some(param_idx)) =
                                (slot_idx, param_idx)
                            {
                                let chain =
                                    state.pattern.track_params[track].midi_fx_chain();
                                if let Some(selected_idx) = chain
                                    .get(slot_idx)
                                    .and_then(|name| {
                                        sequencer::lisp_host::load_midi_fx_descriptor(
                                            name,
                                        )
                                    })
                                    .and_then(|desc| {
                                        desc.params.get(param_idx).and_then(|p| {
                                            match &p.kind {
                                                sequencer::effects::ParamKind::Enum {
                                                    labels,
                                                } => labels
                                                    .iter()
                                                    .position(|item| item == &label),
                                                sequencer::effects::ParamKind::Boolean => {
                                                    match label.as_str() {
                                                        "on" | "ON" => Some(1),
                                                        "off" | "OFF" => Some(0),
                                                        _ => None,
                                                    }
                                                }
                                                _ => None,
                                            }
                                        })
                                    })
                                {
                                    if let Some(_slot) = state
                                        .pattern
                                        .midi_fx_slots
                                        .get(track)
                                        .and_then(|slots| slots.get(slot_idx))
                                    {
                                        app::apply_command(
                                            &mut app,
                                            app::AppCommand::SetMidiFxPlockMulti {
                                                track,
                                                steps: vec![step],
                                                slot_idx,
                                                param_idx,
                                                value: selected_idx as f32,
                                            },
                                        );
                                    }
                                }
                            }
                        }
                        _ => {}
                    }
                    fx_epoch.fetch_add(1, Ordering::Relaxed);
                    ui_epoch.fetch_add(1, Ordering::Relaxed);
                }
            }
        }
        "clear-track-plock-entry" => {
            if let Value::Map(ref map) = payload {
                let target = map.get("target").and_then(|cell| match &*cell.borrow() {
                    Value::String(s) => Some(s.clone()),
                    _ => None,
                });
                let step = map.get("step-idx").and_then(|cell| match &*cell.borrow() {
                    Value::Number(n) => Some(*n as usize),
                    _ => None,
                });
                let slot_idx =
                    map.get("slot-idx").and_then(|cell| match &*cell.borrow() {
                        Value::Number(n) => Some(*n as usize),
                        _ => None,
                    });
                let rack_slot =
                    map.get("rack-slot").and_then(|cell| match &*cell.borrow() {
                        Value::Number(n) => Some(*n as usize),
                        _ => None,
                    });
                let param_idx =
                    map.get("param-idx").and_then(|cell| match &*cell.borrow() {
                        Value::Number(n) => Some(*n as usize),
                        _ => None,
                    });
                let target_track =
                    map.get("target-track")
                        .and_then(|cell| match &*cell.borrow() {
                            Value::Number(n) => Some(*n as usize),
                            _ => None,
                        });
                let network_id =
                    map.get("network-id")
                        .and_then(|cell| match &*cell.borrow() {
                            Value::Number(n) => Some(*n as u64),
                            _ => None,
                        });
                let neuron_idx =
                    map.get("neuron-idx")
                        .and_then(|cell| match &*cell.borrow() {
                            Value::Number(n) => Some(*n as usize),
                            _ => None,
                        });
                if let Some(target) = target {
                    let track = current_track.load(Ordering::Relaxed);
                    let mut changed = false;
                    match target.as_str() {
                        "timebase" => {
                            if let Some(step) = step {
                                changed = app::try_apply_command(
                                    &mut app,
                                    app::AppCommand::SetTimebasePlock {
                                        track,
                                        step,
                                        timebase: None,
                                    },
                                )
                                .is_ok_and(|outcome| outcome != app::edit::EditOutcome::NoOp);
                            }
                        }
                        "swing" => {
                            if let Some(step) = step {
                                changed = app::try_apply_command(
                                    &mut app,
                                    app::AppCommand::SetTrackSwingPlock {
                                        track,
                                        step,
                                        value: None,
                                    },
                                )
                                .is_ok_and(|outcome| outcome != app::edit::EditOutcome::NoOp);
                            }
                        }
                        "swing-resolution" => {
                            if let Some(step) = step {
                                changed = app::try_apply_command(
                                    &mut app,
                                    app::AppCommand::SetTrackSwingResolutionPlock {
                                        track,
                                        step,
                                        resolution: None,
                                    },
                                )
                                .is_ok_and(|outcome| outcome != app::edit::EditOutcome::NoOp);
                            }
                        }
                        "bus-send" => {
                            if let (Some(step), Some(bus_idx)) = (step, param_idx) {
                                if let Some(bus_id) = app.buses.get(bus_idx).map(|bus| bus.id) {
                                    changed = app::try_apply_command(
                                        &mut app,
                                        app::AppCommand::SetTrackBusSendPlock {
                                            track,
                                            step,
                                            destination: bus_id,
                                            value: None,
                                        },
                                    )
                                    .is_ok_and(|outcome| outcome != app::edit::EditOutcome::NoOp);
                                }
                            }
                        }
                        "step-param" => {
                            if let (Some(step), Some(param_idx)) = (step, param_idx) {
                                if let Some(param) =
                                    sequencer::sequencer::StepParam::ALL.get(param_idx)
                                {
                                    changed = app::try_apply_command(
                                        &mut app,
                                        app::AppCommand::SetStepParam {
                                            track,
                                            step,
                                            param: *param,
                                            value: param.default_value(),
                                        },
                                    )
                                    .is_ok_and(|outcome| outcome != app::edit::EditOutcome::NoOp);
                                }
                            }
                        }
                        "instrument" => {
                            if let (Some(step), Some(param_idx)) = (step, param_idx) {
                                changed = app::try_apply_command(
                                    &mut app,
                                    app::AppCommand::ClearInstrumentPlockMulti {
                                        track,
                                        steps: vec![step],
                                        param_idx,
                                    },
                                )
                                .is_ok_and(|outcome| outcome != app::edit::EditOutcome::NoOp);
                            }
                        }
                        "effect" => {
                            if let (Some(step), Some(slot_idx), Some(param_idx)) =
                                (step, slot_idx, param_idx)
                            {
                                changed = app::try_apply_command(
                                    &mut app,
                                    app::AppCommand::ClearEffectPlockMulti {
                                        track,
                                        steps: vec![step],
                                        slot_idx,
                                        param_idx,
                                    },
                                )
                                .is_ok_and(|outcome| outcome != app::edit::EditOutcome::NoOp);
                            }
                        }
                        "rack-macro" => {
                            if let (Some(step), Some(param_idx)) = (step, param_idx) {
                                if let Some(id) =
                                    sequencer::sequencer::RackMacroId::from_index(
                                        param_idx,
                                    )
                                {
                                    changed = app::try_apply_command(
                                        &mut app,
                                        app::AppCommand::ClearRackMacroPlockMulti {
                                            track,
                                            steps: vec![step],
                                            macro_idx: id.index(),
                                        },
                                    )
                                    .is_ok_and(|outcome| outcome != app::edit::EditOutcome::NoOp);
                                }
                            }
                        }
                        "rack-effect" => {
                            if let (
                                Some(step),
                                Some(rack_slot),
                                Some(effect_slot),
                                Some(param_idx),
                            ) = (step, rack_slot, slot_idx, param_idx)
                            {
                                changed = app::try_apply_command(
                                    &mut app,
                                    app::AppCommand::ClearRackSlotEffectPlockMulti {
                                        track,
                                        steps: vec![step],
                                        rack_slot_idx: rack_slot,
                                        effect_slot_idx: effect_slot,
                                        param_idx,
                                    },
                                )
                                .is_ok_and(|outcome| outcome != app::edit::EditOutcome::NoOp);
                            }
                        }
                        "midi-fx" => {
                            if let (Some(step), Some(slot_idx), Some(param_idx)) =
                                (step, slot_idx, param_idx)
                            {
                                changed = app::try_apply_command(
                                    &mut app,
                                    app::AppCommand::ClearMidiFxPlockMulti {
                                        track,
                                        steps: vec![step],
                                        slot_idx,
                                        param_idx,
                                    },
                                )
                                .is_ok_and(|outcome| outcome != app::edit::EditOutcome::NoOp);
                            }
                        }
                        "neural-instrument" => {
                            if let (
                                Some(network_id),
                                Some(neuron_idx),
                                Some(target_track),
                                Some(param_idx),
                            ) = (network_id, neuron_idx, target_track, param_idx)
                            {
                                let history_before = state.capture_project_scenes();
                                match sequencer::lisp_host::clear_neural_instrument_plock_by_network_id(
                                    &state,
                                    network_id,
                                    neuron_idx,
                                    target_track,
                                    param_idx,
                                ) {
                                    Ok(removed) => {
                                        changed |= removed;
                                        if removed {
                                            app.commit_applied_scene_structure_mutation(
                                                history_before,
                                                "Clear neural override",
                                            );
                                        }
                                    }
                                    Err(error) => editor.handle_host_event(
                                        HostEvent::Status(format!(
                                            "Error clearing neuron instrument p-lock: {error}"
                                        )),
                                    ),
                                }
                            }
                        }
                        "neural-effect" => {
                            if let (
                                Some(network_id),
                                Some(neuron_idx),
                                Some(target_track),
                                Some(slot_idx),
                                Some(param_idx),
                            ) = (
                                network_id,
                                neuron_idx,
                                target_track,
                                slot_idx,
                                param_idx,
                            ) {
                                let history_before = state.capture_project_scenes();
                                match sequencer::lisp_host::clear_neural_effect_plock_by_network_id(
                                    &state,
                                    network_id,
                                    neuron_idx,
                                    target_track,
                                    slot_idx,
                                    param_idx,
                                ) {
                                    Ok(removed) => {
                                        changed |= removed;
                                        if removed {
                                            app.commit_applied_scene_structure_mutation(
                                                history_before,
                                                "Clear neural override",
                                            );
                                        }
                                    }
                                    Err(error) => editor.handle_host_event(
                                        HostEvent::Status(format!(
                                            "Error clearing neuron effect p-lock: {error}"
                                        )),
                                    ),
                                }
                            }
                        }
                        _ => {}
                    }
                    if changed {
                        let selection = selected_neural_neurons.lock().unwrap().clone();
                        sync_track_plocks_for_neural_selection(
                            editor.runtime_mut(),
                            &app,
                            &state,
                            track,
                            &selected_steps,
                            &selection,
                        );
                        editor.runtime_mut().run_reactive_cycle();
                        editor.refresh_runtime_side_effects();
                        editor.mark_needs_redraw();
                        fx_epoch.fetch_add(1, Ordering::Relaxed);
                        ui_epoch.fetch_add(1, Ordering::Relaxed);
                    } else if step.is_some() {
                        state.publish_scheduler_snapshot();
                        fx_epoch.fetch_add(1, Ordering::Relaxed);
                        ui_epoch.fetch_add(1, Ordering::Relaxed);
                    }
                }
            }
        }
        "preview-plock-variant" => {
            if let Value::Map(ref map) = payload {
                if let Some(label) = map_string(map, "label") {
                    let track = current_track.load(Ordering::Relaxed);
                    if !selected_steps.lock().unwrap().is_empty() {
                        ctx.gesture.preview_plock_variant = None;
                        return;
                    }
                    ctx.gesture.preview_plock_variant = Some((track, label));
                    {
                        let rt = editor.runtime_mut();
                        sync_track_plock_variant_preview(
                            rt,
                            &app,
                            &state,
                            track,
                            &selected_steps,
                            ctx.gesture.preview_plock_variant.as_ref(),
                        );
                        rt.run_reactive_cycle();
                    }
                    editor.refresh_runtime_side_effects();
                    editor.refresh_visible_layouts_for_buffer_named("*step*");
                    editor.mark_needs_redraw();
                }
            }
        }
        "stamp-plock-variant" | "clear-step-variant-locks" => {
            if let Value::Map(ref map) = payload {
                ctx.gesture.preview_plock_variant = None;
                let label = map.get("label").and_then(|cell| match &*cell.borrow() {
                    Value::String(s) => Some(s.clone()),
                    _ => None,
                });
                let fallback_step =
                    map.get("step").and_then(|cell| match &*cell.borrow() {
                        Value::Number(n) => Some(*n as usize),
                        _ => None,
                    });
                let mut steps: Vec<usize> =
                    selected_steps.lock().unwrap().iter().copied().collect();
                steps.sort_unstable();
                if steps.is_empty() {
                    if let Some(step) = fallback_step {
                        steps.push(step.min(MAX_STEPS - 1));
                    }
                }
                if steps.is_empty() {
                    return;
                }
                let track = current_track.load(Ordering::Relaxed);
                let is_clear = name == "clear-step-variant-locks"
                    || label.as_deref() == Some("def");
                let assignment = label.as_ref().and_then(|label| {
                    state
                        .plock_variant_registry_snapshot(track)
                        .assignment_for_label(label)
                        .map(|assignment| assignment.key.clone())
                });
                let outcome = app::edit::apply_recorded_step_mutation(
                    &mut app,
                    track,
                    &steps,
                    if is_clear {
                        "Clear step variant locks"
                    } else {
                        "Stamp step variant"
                    },
                    |app| {
                        if is_clear {
                            app.state.clear_variant_locks_for_steps_no_publish(
                                track,
                                &steps,
                            );
                        } else if let Some(key) = &assignment {
                            app.state.stamp_variant_key_to_steps_no_publish(
                                track,
                                key,
                                &steps,
                            );
                        }
                        Ok(())
                    },
                );
                let changed = match outcome {
                    Ok(app::edit::EditOutcome::Applied(_)) => true,
                    Ok(app::edit::EditOutcome::NoOp) => false,
                    Ok(app::edit::EditOutcome::AppliedUnrecorded) => {
                        editor.handle_host_event(HostEvent::Error(
                            "Variant edit was applied without history".to_string(),
                        ));
                        false
                    }
                    Err(error) => {
                        editor.handle_host_event(HostEvent::Error(format!(
                            "Could not apply variant edit: {error:?}"
                        )));
                        false
                    }
                };
                if changed {
                    fx_epoch.fetch_add(1, Ordering::Relaxed);
                    ui_epoch.fetch_add(1, Ordering::Relaxed);
                }
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

    const TRACK: usize = 0;
    const STEP: usize = 5;

    struct Harness {
        app: app::App,
        editor: Editor,
        state: Arc<SequencerState>,
        shared: SharedHandles,
        sessions: EditSessionState,
        frame: FrameDiffState,
        gesture: GestureState,
        meters: MeterCache,
        track_names: Vec<String>,
        selected_steps: Arc<Mutex<HashSet<usize>>>,
        piano_roll_selection: Arc<Mutex<HashSet<u64>>>,
        track_collapsed: Arc<Mutex<Vec<bool>>>,
        bus_state: Arc<Mutex<Vec<app::BusChannelState>>>,
        accumulator_names: Arc<Mutex<Vec<String>>>,
        record_armed: Arc<Mutex<Vec<bool>>>,
        active_delete_target: Arc<Mutex<Option<ActiveDeleteTarget>>>,
        active_delete_target_version: Arc<AtomicUsize>,
        expanded_step_projection: Arc<ExpandedStepProjectionRegistry>,
        ui_epoch: Arc<AtomicUsize>,
        ui_invalidations: Arc<UiInvalidationQueue>,
    }

    impl Harness {
        fn new() -> Self {
            let state = Arc::new(sequencer::sequencer::SequencerState::new(
                1,
                vec![sequencer::sequencer::default_empty_effect_chain()],
            ));
            state.pattern.track_params[TRACK].set_num_steps(16);
            // An ACTIVE step: the duration bar and the piano roll only render
            // steps that are on.
            state.pattern.patterns[TRACK].set_step_active(STEP, true);
            let (keyboard_tx, _keyboard_rx) = std::sync::mpsc::channel();
            let mut app = app::App::new(
                state.clone(),
                sequencer::audiograph::LiveGraphPtr(std::ptr::null_mut()),
                44_100,
                app::AudioBuses {
                    bus_l_id: 0,
                    bus_r_id: 0,
                    default_bus_nodes: Vec::new(),
                    bus_effect_runtime: Arc::new(Mutex::new(Arc::new(Vec::new()))),
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
            // `sync_single_step_param_binding` resolves the *step* panel's
            // "parameter step" from the lisp `cursor-step` global, exactly as
            // ui/main.lisp defines it. Park the cursor on the edited step,
            // which is what dragging that panel's picker means.
            runtime
                .eval_str(&format!("(def cursor-step {STEP})"))
                .expect("seed the lisp cursor-step global");
            let editor = Editor::new(runtime, eseqlisp::EditorConfig::default());

            let selected_steps = Arc::new(Mutex::new(HashSet::new()));
            let piano_roll_selection = Arc::new(Mutex::new(HashSet::new()));
            let track_collapsed = Arc::new(Mutex::new(app.track_collapsed.clone()));
            let bus_state = Arc::new(Mutex::new(app.buses.clone()));
            let accumulator_names = Arc::new(Mutex::new(Vec::new()));
            let record_armed = Arc::new(Mutex::new(vec![false]));
            let active_delete_target = Arc::new(Mutex::new(None));
            let active_delete_target_version = Arc::new(AtomicUsize::new(0));
            let expanded_step_projection = Arc::new(ExpandedStepProjectionRegistry::new());
            let ui_epoch = Arc::new(AtomicUsize::new(0));
            let ui_invalidations = Arc::new(UiInvalidationQueue::new());
            let sample_db = Rc::new(
                sequencer::sample_db::SampleDb::open_in_memory().expect("in-memory sample db"),
            );
            let shared = SharedHandles {
                state: state.clone(),
                lg_raw: std::ptr::null_mut(),
                current_track: Arc::new(AtomicUsize::new(TRACK)),
                selected_tracks: Arc::new(Mutex::new(HashSet::new())),
                selected_steps: selected_steps.clone(),
                selected_neural_neurons: Arc::new(Mutex::new(BTreeSet::new())),
                piano_roll_selection: piano_roll_selection.clone(),
                piano_roll_move_state: Arc::new(Mutex::new(None)),
                piano_roll_focus: super::super::super::new_shared_piano_roll_focus(),
                step_clipboard: Arc::new(Mutex::new(None)),
                ui_epoch: ui_epoch.clone(),
                fx_epoch: Arc::new(AtomicUsize::new(0)),
                fx_value_epoch: Arc::new(AtomicUsize::new(0)),
                ui_invalidations: ui_invalidations.clone(),
                expanded_step_projection: expanded_step_projection.clone(),
                active_delete_target: active_delete_target.clone(),
                active_delete_target_version: active_delete_target_version.clone(),
                auto_follow_override_until: Arc::new(Mutex::new(None)),
                track_pan_ids: Arc::new(Mutex::new(Vec::new())),
                track_collapsed: track_collapsed.clone(),
                bus_state: bus_state.clone(),
                bus_node_ids: Arc::new(Mutex::new(app.graph.bus_node_ids.clone())),
                track_groups: Arc::new(Mutex::new(app.groups.clone())),
                record_armed: record_armed.clone(),
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
                accumulator_names: accumulator_names.clone(),
                piano_roll_clipboard: super::super::super::new_piano_roll_clipboard(),
                arrangement_clipboard: app::song_region::new_arrangement_clipboard(),
            };
            Self {
                app,
                editor,
                state,
                shared,
                sessions: EditSessionState::default(),
                frame: FrameDiffState::default(),
                gesture: GestureState::default(),
                meters: MeterCache {
                    cached_peak_l_level: 0.0,
                    cached_peak_r_level: 0.0,
                    cached_track_peak_levels: vec![0.0],
                    cached_rack_slot_peak_levels: Vec::new(),
                    cached_bus_peak_levels: Vec::new(),
                    cached_modulator_phases: Vec::new(),
                    cached_modulator_levels: Vec::new(),
                    cached_mod_display_values: Default::default(),
                    watched_display_modulators: std::collections::HashSet::new(),
                    mod_display_poll_fx_epoch: usize::MAX,
                mod_display_poll_track: None,
                    cached_cpu_load_bits: 0.0f32.to_bits(),
                    last_meter_poll_at: Instant::now(),
                    last_cpu_ui_poll_at: Instant::now(),
                    last_neural_visualization_poll_at: Instant::now(),
                    visualization_liveness: VisualizationLiveness::default(),
                    last_voice_count_log_at: Instant::now(),
                },
                track_names: vec!["Track 1".to_string()],
                selected_steps,
                piano_roll_selection,
                track_collapsed,
                bus_state,
                accumulator_names,
                record_armed,
                active_delete_target,
                active_delete_target_version,
                expanded_step_projection,
                ui_epoch,
                ui_invalidations,
            }
        }

        /// The real seam: `dispatch_custom_host_command` -> this module's
        /// `handle`, then the reactive tick's invalidation drain. Nothing here
        /// mirrors handler policy — if the handler stops publishing a surface,
        /// the assertions below stop seeing it.
        fn set_step_param(&mut self, param: &str, value: f64) {
            let payload = Value::Map(
                [
                    (
                        "track".to_string(),
                        Rc::new(RefCell::new(Value::Number(TRACK as f64))),
                    ),
                    (
                        "param".to_string(),
                        Rc::new(RefCell::new(Value::Keyword(param.to_string()))),
                    ),
                    (
                        "value".to_string(),
                        Rc::new(RefCell::new(Value::Number(value))),
                    ),
                    (
                        "steps".to_string(),
                        Rc::new(RefCell::new(Value::List(vec![Rc::new(RefCell::new(
                            Value::Number(STEP as f64),
                        ))]))),
                    ),
                ]
                .into_iter()
                .collect(),
            );
            {
                let mut ctx = LoopCtx {
                    sessions: &mut self.sessions,
                    meters: &mut self.meters,
                    frame: &mut self.frame,
                    gesture: &mut self.gesture,
                    track_names: &mut self.track_names,
                    shared: &self.shared,
                };
                dispatch_custom_host_command(
                    "set-step-param-history",
                    payload,
                    &mut self.app,
                    &mut self.editor,
                    &mut ctx,
                );
            }
            let invalidations = self.ui_invalidations.drain();
            assert!(
                !invalidations.is_empty(),
                "a step-param edit must queue targeted invalidations"
            );
            let neural = BTreeSet::new();
            let peaks = vec![0.0f64];
            let bus_peaks: Vec<f64> = Vec::new();
            apply_ui_invalidations(
                invalidations,
                UiInvalidationApplyCtx {
                    app: &mut self.app,
                    editor: &mut self.editor,
                    state: &self.state,
                    track_collapsed: &self.track_collapsed,
                    bus_state: &self.bus_state,
                    current_track_idx: TRACK,
                    selected_steps: &self.selected_steps,
                    selected_neural_neurons: &neural,
                    piano_roll_selection: &self.piano_roll_selection,
                    accumulator_names: &self.accumulator_names,
                    cached_track_peak_levels: &peaks,
                    cached_bus_peak_levels: &bus_peaks,
                    record_armed: &self.record_armed,
                    active_delete_target: &self.active_delete_target,
                    active_delete_target_version: &self.active_delete_target_version,
                    expanded_step_projection: &self.expanded_step_projection,
                    fx_visible: true,
                    sequencer_visible: true,
                    mixer_visible: true,
                },
            );
        }

        fn number(&self, field: &str) -> f64 {
            match self.editor.runtime().reactive_field_value("SEQ", field) {
                Some(Value::Number(value)) => *value,
                other => panic!("SEQ.{field} should be a number, got {other:?}"),
            }
        }

        fn list_number(&self, field: &str, index: usize) -> f64 {
            match self.editor.runtime().reactive_field_value("SEQ", field) {
                Some(Value::List(items)) => match items.get(index).map(|item| item.borrow().clone())
                {
                    Some(Value::Number(value)) => value,
                    other => panic!("SEQ.{field}[{index}] should be a number, got {other:?}"),
                },
                other => panic!("SEQ.{field} should be a list, got {other:?}"),
            }
        }

        fn nested_list_number(&self, field: &str, outer: usize, inner: usize) -> f64 {
            match self.editor.runtime().reactive_field_value("SEQ", field) {
                Some(Value::List(rows)) => match rows.get(outer).map(|row| row.borrow().clone()) {
                    Some(Value::List(items)) => {
                        match items.get(inner).map(|item| item.borrow().clone()) {
                            Some(Value::Number(value)) => value,
                            other => panic!(
                                "SEQ.{field}[{outer}][{inner}] should be a number, got {other:?}"
                            ),
                        }
                    }
                    other => panic!("SEQ.{field}[{outer}] should be a list, got {other:?}"),
                },
                other => panic!("SEQ.{field} should be a list of lists, got {other:?}"),
            }
        }

        fn piano_roll_lanes(&self) -> Vec<f64> {
            match self
                .editor
                .runtime()
                .reactive_field_value("SEQ", "piano-roll-items")
            {
                Some(Value::List(items)) => items
                    .iter()
                    .filter_map(|item| match &*item.borrow() {
                        Value::Map(map) => map.get("lane").and_then(|cell| match &*cell.borrow() {
                            Value::Number(lane) => Some(*lane),
                            _ => None,
                        }),
                        _ => None,
                    })
                    .collect(),
                other => panic!("SEQ.piano-roll-items should be a list, got {other:?}"),
            }
        }

        fn nested_list_bool(&self, field: &str, outer: usize, inner: usize) -> bool {
            match self.editor.runtime().reactive_field_value("SEQ", field) {
                Some(Value::List(rows)) => match rows.get(outer).map(|row| row.borrow().clone()) {
                    Some(Value::List(items)) => {
                        match items.get(inner).map(|item| item.borrow().clone()) {
                            Some(Value::Bool(value)) => value,
                            other => panic!(
                                "SEQ.{field}[{outer}][{inner}] should be a bool, got {other:?}"
                            ),
                        }
                    }
                    other => panic!("SEQ.{field}[{outer}] should be a list, got {other:?}"),
                },
                other => panic!("SEQ.{field} should be a list of lists, got {other:?}"),
            }
        }

        fn bool_field(&self, field: &str) -> bool {
            match self.editor.runtime().reactive_field_value("SEQ", field) {
                Some(Value::Bool(value)) => *value,
                other => panic!("SEQ.{field} should be a bool, got {other:?}"),
            }
        }
    }

    /// Every surface a Transpose / Velocity edit from the `*step*` panel feeds.
    ///
    /// `set-step-param-history` no longer bumps `ui_epoch` (that bump cost
    /// ~7ms of `sync_all_track_sequencer_state` + a whole-list
    /// `sync_step_param_lists` per drag update), so the targeted invalidations
    /// are now the ONLY writer for all of these:
    ///   - `SEQ.{transposes,velocities}` — read by the `*step*` panel's
    ///     `fx-step-param-value`, `set-cursor-step-value`, and `*metal*`.
    ///   - `SEQ.track-{transposes,velocities}` — read by
    ///     `seqv-track-param-values` for every non-current expanded lane.
    ///   - `seq-track-step-param-{slider,haptic}-{track}-{mode}-{step}`.
    ///   - `fx-step-value-{param}` — the number-picker readout being dragged.
    ///   - `SEQ.piano-roll-items` — note pitch comes from the step transpose.
    #[test]
    fn set_step_param_publishes_every_step_panel_surface_without_a_ui_epoch_bump() {
        let mut harness = Harness::new();
        let epoch_before = harness.ui_epoch.load(Ordering::Relaxed);

        harness.set_step_param("transpose", 7.0);

        assert_eq!(
            harness.state.pattern.step_data[TRACK].get(STEP, StepParam::Transpose),
            7.0,
            "precondition: the handler wrote the model"
        );
        assert_eq!(
            harness.ui_epoch.load(Ordering::Relaxed),
            epoch_before,
            "a step-param edit must stay on the targeted path — a ui_epoch bump \
             resyncs every track on every drag update"
        );
        assert_eq!(
            harness.list_number("transposes", STEP),
            7.0,
            "the current track's flat transpose list must be published"
        );
        assert_eq!(
            harness.nested_list_number("track-transposes", TRACK, STEP),
            7.0,
            "the per-track transpose list-of-lists (eseq.seqv-track-params/seqv-track-param-values) \
             must be published"
        );
        assert_eq!(
            harness.number(&track_step_param_slider_field(TRACK, 3, STEP)),
            7.0,
            "the per-step transpose slider binding must be published"
        );
        assert_eq!(
            harness.number(&track_step_param_haptic_field(TRACK, 3, STEP)),
            7.0,
            "the per-step transpose haptic binding must be published"
        );
        assert_eq!(
            harness.number("fx-step-value-transpose"),
            7.0,
            "the *step* panel's Transpose number-picker readout must be published"
        );
        let lanes_at_7 = harness.piano_roll_lanes();
        assert!(
            !lanes_at_7.is_empty(),
            "the active step must render a piano-roll note"
        );
        harness.set_step_param("transpose", -5.0);
        let lanes_at_minus_5 = harness.piano_roll_lanes();
        assert_eq!(
            lanes_at_minus_5.len(),
            lanes_at_7.len(),
            "the note count must not change"
        );
        for (before, after) in lanes_at_7.iter().zip(lanes_at_minus_5.iter()) {
            // The lane axis runs top-down (lane 0 is the highest pitch), so a
            // 12-semitone drop moves the note 12 lanes DOWN the list.
            assert_eq!(
                after - before,
                12.0,
                "the piano roll must follow the step transpose (its note pitch \
                 comes from StepParam::Transpose), lanes {lanes_at_7:?} -> \
                 {lanes_at_minus_5:?}"
            );
        }
        harness.set_step_param("transpose", 7.0);

        // Velocity shares the funnel but a different mode index / list.
        harness.set_step_param("velocity", 0.25);
        assert_eq!(harness.list_number("velocities", STEP), 0.25);
        assert_eq!(
            harness.nested_list_number("track-velocities", TRACK, STEP),
            0.25
        );
        assert_eq!(
            harness.number(&track_step_param_slider_field(TRACK, 0, STEP)),
            0.25
        );
        assert_eq!(
            harness.number(&track_step_param_haptic_field(TRACK, 0, STEP)),
            0.25
        );
        assert_eq!(
            harness.number("fx-step-value-velocity"),
            0.25,
            "the *step* panel's Velocity number-picker readout must be published"
        );
        assert_eq!(
            harness.ui_epoch.load(Ordering::Relaxed),
            epoch_before,
            "velocity edits must stay off the epoch path too"
        );
    }

    #[test]
    fn step_panel_edits_surviving_chord_backed_note_after_original_is_deleted() {
        let mut harness = Harness::new();
        let lanes = PianoRollLanes::live(&harness.state, TRACK);
        let selection = Arc::new(Mutex::new(HashSet::new()));
        let move_state = Arc::new(Mutex::new(None));

        let add_second_note = map_value([
            ("type", Value::Keyword("finish-create-item".to_string())),
            ("lane", Value::Number(41.0)), // G4, transpose +7
            ("start", Value::Number(STEP as f64)),
            ("end", Value::Number(STEP as f64 + 2.0)),
        ]);
        apply_piano_roll_action(&lanes, &selection, &move_state, &add_second_note)
            .expect("add the second piano-roll note");
        let delete_original = map_value([
            ("type", Value::Keyword("delete-items".to_string())),
            (
                "ids",
                list_value([Value::Number(piano_roll_item_id(STEP, 0) as f64)]),
            ),
        ]);
        apply_piano_roll_action(&lanes, &selection, &move_state, &delete_original)
            .expect("delete the original piano-roll note");

        assert_eq!(harness.state.pattern.chord_data[TRACK].count(STEP), 1);
        assert_eq!(harness.state.pattern.chord_data[TRACK].get(STEP, 0), 7.0);
        assert_eq!(
            harness.state.pattern.step_data[TRACK].get(STEP, StepParam::Transpose),
            7.0,
            "the base transpose is only the chord's step-level anchor"
        );

        harness.set_step_param("transpose", 12.0);
        assert_eq!(harness.state.pattern.chord_data[TRACK].get(STEP, 0), 12.0);
        assert_eq!(harness.piano_roll_lanes(), vec![36.0]);
        assert_eq!(harness.number("fx-step-value-transpose"), 12.0);

        harness.set_step_param("duration", 3.5);
        assert_eq!(
            harness.state.pattern.chord_data[TRACK].get_duration(STEP, 0),
            3.5
        );
        assert_eq!(harness.number("fx-step-value-duration"), 3.5);

        harness.set_step_param("velocity", 0.375);
        assert_eq!(
            harness.state.pattern.step_data[TRACK].get(STEP, StepParam::Velocity),
            0.375
        );
        assert_eq!(harness.number("fx-step-value-velocity"), 0.375);
    }

    /// Duration additionally paints the compact grid's duration bar, which is
    /// a SEPARATE surface with a separate writer: the per-step
    /// `seq-track-step-duration-{track}-{step}` bools cover every cell the
    /// note now reaches, and `SEQ.track-duration-spans` is the list form.
    #[test]
    fn set_step_duration_publishes_the_duration_bar_surfaces_without_a_ui_epoch_bump() {
        let mut harness = Harness::new();
        let epoch_before = harness.ui_epoch.load(Ordering::Relaxed);

        harness.set_step_param("duration", 4.0);

        assert_eq!(
            harness.state.pattern.step_data[TRACK].get(STEP, StepParam::Duration),
            4.0,
            "precondition: the handler wrote the model"
        );
        assert_eq!(
            harness.ui_epoch.load(Ordering::Relaxed),
            epoch_before,
            "duration edits must stay on the targeted path"
        );
        assert_eq!(harness.list_number("durations", STEP), 4.0);
        assert_eq!(
            harness.nested_list_number("track-durations", TRACK, STEP),
            4.0
        );
        assert_eq!(
            harness.number("fx-step-value-duration"),
            4.0,
            "the *step* panel's Duration number-picker readout must be published"
        );
        // The duration bar: the note now reaches STEP..STEP+4.
        for step in STEP..STEP + 4 {
            assert!(
                harness.bool_field(&track_step_duration_field(TRACK, step)),
                "step {step} must be marked as covered by the duration bar"
            );
        }
        assert!(
            !harness.bool_field(&track_step_duration_field(TRACK, STEP + 4)),
            "the cell past the note's reach must not be marked covered"
        );
        assert!(
            harness.nested_list_bool("track-duration-spans", TRACK, STEP + 3),
            "the list form of the duration span must be published too"
        );

        // Shortening it must clear the cells it no longer reaches.
        harness.set_step_param("duration", 1.0);
        for step in STEP + 1..STEP + 4 {
            assert!(
                !harness.bool_field(&track_step_duration_field(TRACK, step)),
                "step {step} must be released when the note is shortened"
            );
        }
    }

    /// The compact step shell's p-lock tick / variant tint is
    /// `plock_variant_step_render_values`, whose `live_track_has_seq_lock` term
    /// is true as soon as ANY `StepParam` departs from its default — so a
    /// transpose edit flips `seq-track-step-plock-kind-{track}-{step}` 0 -> 1
    /// and restoring the default flips it back. Nothing else writes that field
    /// on a step-param edit now that the funnel skips `ui_epoch`.
    #[test]
    fn set_step_param_flips_the_compact_shell_seq_lock_tick() {
        let mut harness = Harness::new();
        let epoch_before = harness.ui_epoch.load(Ordering::Relaxed);

        harness.set_step_param("transpose", 7.0);
        assert_eq!(
            harness.number(&track_step_plock_kind_field(TRACK, STEP)),
            1.0,
            "a step param off its default must light the compact shell's \
             seq-lock tick"
        );

        harness.set_step_param("transpose", 0.0);
        assert_eq!(
            harness.number(&track_step_plock_kind_field(TRACK, STEP)),
            0.0,
            "restoring the default must clear the tick again"
        );
        assert_eq!(
            harness.ui_epoch.load(Ordering::Relaxed),
            epoch_before,
            "the tick must be reached by the targeted path, not an epoch resync"
        );
    }

    /// Bead eseq-jc9: `print-step-param` latches into print mode only while
    /// the transport plays with record on; if that gate raced off before
    /// dispatch, the touch degrades to the normal cursor-step edit.
    #[test]
    fn print_step_param_gates_on_play_and_record_with_cursor_edit_fallback() {
        let mut harness = Harness::new();
        let payload = |value: f64| {
            Value::Map(
                [
                    (
                        "track".to_string(),
                        Rc::new(RefCell::new(Value::Number(TRACK as f64))),
                    ),
                    (
                        "param".to_string(),
                        Rc::new(RefCell::new(Value::Keyword("velocity".to_string()))),
                    ),
                    (
                        "value".to_string(),
                        Rc::new(RefCell::new(Value::Number(value))),
                    ),
                    (
                        "steps".to_string(),
                        Rc::new(RefCell::new(Value::List(vec![Rc::new(RefCell::new(
                            Value::Number(STEP as f64),
                        ))]))),
                    ),
                ]
                .into_iter()
                .collect(),
            )
        };
        let dispatch = |harness: &mut Harness, value: f64| {
            let mut ctx = LoopCtx {
                sessions: &mut harness.sessions,
                meters: &mut harness.meters,
                frame: &mut harness.frame,
                gesture: &mut harness.gesture,
                track_names: &mut harness.track_names,
                shared: &harness.shared,
            };
            dispatch_custom_host_command(
                "print-step-param",
                payload(value),
                &mut harness.app,
                &mut harness.editor,
                &mut ctx,
            );
        };

        // Transport stopped: the touch is a plain cursor-step edit and never
        // arms the latch.
        dispatch(&mut harness, 0.25);
        assert_eq!(
            harness.state.pattern.step_data[TRACK].get(STEP, StepParam::Velocity),
            0.25,
            "stopped-transport fallback must edit the cursor step"
        );
        assert!(
            !harness.shared.step_print.lock().unwrap().armed(),
            "stopped-transport touches must not arm print mode"
        );
        assert!(
            !harness.ui_invalidations.drain().is_empty(),
            "the fallback edit must queue the targeted step invalidations"
        );

        // Playing + recording: the touch latches instead of editing the
        // cursor step.
        harness
            .state
            .transport
            .playing
            .store(true, Ordering::Relaxed);
        harness.shared.recording.store(true, Ordering::Relaxed);
        dispatch(&mut harness, 0.75);
        assert_eq!(
            harness.state.pattern.step_data[TRACK].get(STEP, StepParam::Velocity),
            0.25,
            "an armed touch prints via the tick, not onto the cursor step"
        );
        assert!(
            harness.shared.step_print.lock().unwrap().armed(),
            "playing+recording touches must arm print mode"
        );
        assert!(
            harness.ui_invalidations.drain().is_empty(),
            "arming alone queues no invalidations; the tick's writes do"
        );
    }
}
