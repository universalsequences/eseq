use crate::*;

pub(super) const COMMANDS: &[&str] = &[
    "midi-fx-history-action",
    "process-history-action",
    "piano-roll-gesture-update",
    "piano-roll-gesture-finish",
    "piano-roll-history-action",
    "drum-lane-history-action",
    "delete-selected-steps",
    "paste-steps",
    "set-step-param-history",
    "move-step-history",
    "slice2-history-action",
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
    let selected_drum_lane_steps = ctx.shared.selected_drum_lane_steps.clone();
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
        "drum-lane-history-action" => {
            match apply_drum_lane_history_host_command(&mut app, &payload) {
                Ok((app::edit::EditOutcome::Applied(result), action)) => {
                    let track = action.track();
                    let bindings = editor.runtime().reactive_binding_store();
                    match action {
                        DrumLaneHistoryAction::Toggle { .. } => {
                            if !selected_steps.lock().unwrap().is_empty() {
                                selected_steps.lock().unwrap().clear();
                                fx_epoch.fetch_add(1, Ordering::Relaxed);
                            }
                            clear_drum_lane_selection(
                                &bindings,
                                &mut selected_drum_lane_steps.lock().unwrap(),
                            );
                        }
                        DrumLaneHistoryAction::Move {
                            pad_note,
                            steps,
                            delta,
                            move_selection: true,
                            ..
                        } => {
                            let mut selected = selected_drum_lane_steps.lock().unwrap();
                            for step in steps {
                                let old = DrumLaneStepSelection {
                                    track,
                                    pad_note,
                                    step,
                                };
                                selected.remove(&old);
                                write_drum_lane_selection(&bindings, old, false);
                                let new = DrumLaneStepSelection {
                                    track,
                                    pad_note,
                                    step: (step as isize + delta) as usize,
                                };
                                selected.insert(new);
                                write_drum_lane_selection(&bindings, new, true);
                            }
                        }
                        DrumLaneHistoryAction::Clear { .. } => {
                            clear_drum_lane_selection(
                                &bindings,
                                &mut selected_drum_lane_steps.lock().unwrap(),
                            );
                        }
                        DrumLaneHistoryAction::Duration { .. }
                        | DrumLaneHistoryAction::Move {
                            move_selection: false,
                            ..
                        } => {}
                    }
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
                        "Drum-lane edit was applied without history".to_string(),
                    ));
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
                    ui_invalidations.push(UiInvalidation::Pattern(
                        PatternInvalidation::WholeTrack { track },
                    ));
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
                    ui_epoch.fetch_add(1, Ordering::Relaxed);
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
