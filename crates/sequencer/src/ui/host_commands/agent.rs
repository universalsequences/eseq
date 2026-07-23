use crate::*;

pub(super) const COMMANDS: &[&str] = &[
    "agent-accept",
    "agent-send",
    "agent-ensure-instrument-stub",
    "agent-finalize",
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
    let ui_epoch = ctx.shared.ui_epoch.clone();
    let active_delete_target = ctx.shared.active_delete_target.clone();
    let track_pan_ids = ctx.shared.track_pan_ids.clone();
    let record_armed = ctx.shared.record_armed.clone();
    let accumulator_names = ctx.shared.accumulator_names.clone();
    match name {
        "agent-accept" => {
            let conv_id = match payload {
                Value::Number(id) if id >= 1.0 => id as sequencer::agent::store::ConvId,
                _ => {
                    editor.handle_host_event(HostEvent::Error(
                        "agent-accept: expected conversation id".to_string(),
                    ));
                    return;
                }
            };
            let snapshot = app.agent_store.snapshot(conv_id);
            let apply_as_effect =
                match snapshot.as_ref().map(|snapshot| &snapshot.state) {
                    Some(state) => match state.kind {
                        sequencer::agent::store::AgentKind::Effect => true,
                        sequencer::agent::store::AgentKind::Instrument => false,
                        sequencer::agent::store::AgentKind::General => {
                            state.effect_draft.is_some()
                                || state.accepted_effect_target.is_some()
                        }
                    },
                    None => false,
                };
            if !apply_as_effect {
                match apply_agent_draft_to_owned_instrument(
                    &mut app,
                    &mut editor,
                    &state,
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
                    conv_id,
                ) {
                    Ok(result) => {
                        let verb = if result.created_track {
                            "Accepted agent draft as track"
                        } else {
                            "Updated agent draft on track"
                        };
                        editor.handle_host_event(HostEvent::Status(format!(
                            "{verb} {}",
                            result.track_index + 1
                        )));
                    }
                    Err(error) => {
                        editor.handle_host_event(HostEvent::Error(error));
                    }
                }
            } else {
                match apply_agent_draft_to_effect_slot(
                    &mut app,
                    &mut editor,
                    &state,
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
                    conv_id,
                ) {
                    Ok(result) => {
                        editor.handle_host_event(HostEvent::Status(format!(
                            "Accepted agent effect as track {} slot {}",
                            result.track_index + 1,
                            result.slot_index + 1
                        )));
                    }
                    Err(error) => {
                        editor.handle_host_event(HostEvent::Error(error));
                    }
                }
            }
        }
        "agent-send" => {
            let Value::Map(map) = payload else {
                editor.handle_host_event(HostEvent::Error(
                    "agent-send: expected payload map".to_string(),
                ));
                return;
            };
            let conv_id = match map.get("id").map(|cell| cell.borrow().clone()) {
                Some(Value::Number(id)) if id >= 1.0 => {
                    id as sequencer::agent::store::ConvId
                }
                _ => {
                    editor.handle_host_event(HostEvent::Error(
                        "agent-send: expected conversation id".to_string(),
                    ));
                    return;
                }
            };
            let prompt = match map.get("prompt").map(|cell| cell.borrow().clone()) {
                Some(Value::String(prompt)) => prompt,
                _ => {
                    editor.handle_host_event(HostEvent::Error(
                        "agent-send: expected prompt string".to_string(),
                    ));
                    return;
                }
            };

            let needs_stub = app
                .agent_store
                .snapshot(conv_id)
                .map(|snapshot| {
                    let state = snapshot.state;
                    state.kind == sequencer::agent::store::AgentKind::Instrument
                        && state.draft.is_none()
                        && state.stub_instrument_target.is_none()
                        && state.accepted_instrument_target.is_none()
                        && state.finalized_instrument_name.is_none()
                })
                .unwrap_or(false);
            if needs_stub {
                match ensure_agent_instrument_stub_track(
                    &mut app,
                    &mut editor,
                    &state,
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
                    conv_id,
                ) {
                    Ok(track_index) => {
                        editor.handle_host_event(HostEvent::Status(format!(
                            "Created working instrument track {}",
                            track_index + 1
                        )));
                    }
                    Err(error) => {
                        editor.handle_host_event(HostEvent::Error(error));
                        return;
                    }
                }
            }

            let session_context = metal_agent_session_context(
                &app,
                &current_track,
                &active_delete_target,
            );
            if let Err(error) =
                app.agent_store
                    .send_with_context(conv_id, prompt, session_context)
            {
                editor.handle_host_event(HostEvent::Error(error));
            }
        }
        "agent-ensure-instrument-stub" => {
            let conv_id = match payload {
                Value::Number(id) if id >= 1.0 => id as sequencer::agent::store::ConvId,
                _ => {
                    editor.handle_host_event(HostEvent::Error(
                        "agent-ensure-instrument-stub: expected conversation id"
                            .to_string(),
                    ));
                    return;
                }
            };
            match ensure_agent_instrument_stub_track(
                &mut app,
                &mut editor,
                &state,
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
                conv_id,
            ) {
                Ok(track_index) => {
                    editor.handle_host_event(HostEvent::Status(format!(
                        "Created working instrument track {}",
                        track_index + 1
                    )));
                }
                Err(error) => {
                    editor.handle_host_event(HostEvent::Error(error));
                }
            }
        }
        "agent-finalize" => {
            let Value::Map(map) = payload else {
                editor.handle_host_event(HostEvent::Error(
                    "agent-finalize: expected payload map".to_string(),
                ));
                return;
            };
            let conv_id = match map.get("id").map(|cell| cell.borrow().clone()) {
                Some(Value::Number(id)) if id >= 1.0 => {
                    id as sequencer::agent::store::ConvId
                }
                _ => {
                    editor.handle_host_event(HostEvent::Error(
                        "agent-finalize: expected conversation id".to_string(),
                    ));
                    return;
                }
            };
            let requested_name = match map.get("name").map(|cell| cell.borrow().clone())
            {
                Some(Value::String(name)) if !name.trim().is_empty() => name,
                _ => {
                    editor.handle_host_event(HostEvent::Error(
                        "agent-finalize: expected non-empty artifact name".to_string(),
                    ));
                    return;
                }
            };
            let snapshot = app.agent_store.snapshot(conv_id);
            let finalize_as_effect =
                match snapshot.as_ref().map(|snapshot| &snapshot.state) {
                    Some(state) => match state.kind {
                        sequencer::agent::store::AgentKind::Effect => true,
                        sequencer::agent::store::AgentKind::Instrument => false,
                        sequencer::agent::store::AgentKind::General => {
                            state.effect_draft.is_some()
                                || state.accepted_effect_target.is_some()
                        }
                    },
                    None => false,
                };
            if !finalize_as_effect {
                match finalize_agent_instrument(
                    &mut app,
                    &mut editor,
                    &state,
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
                    conv_id,
                    &requested_name,
                ) {
                    Ok(result) => {
                        editor.handle_host_event(HostEvent::Status(format!(
                            "Saved agent artifact {} as track {}",
                            display_instrument_name(&result.instrument_name),
                            result.track_index + 1
                        )));
                    }
                    Err(error) => {
                        editor.handle_host_event(HostEvent::Error(error));
                    }
                }
            } else {
                match finalize_agent_effect(
                    &mut app,
                    &mut editor,
                    &state,
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
                    conv_id,
                    &requested_name,
                ) {
                    Ok(result) => {
                        let target = match (result.track_index, result.slot_index) {
                            (Some(track), Some(slot)) => {
                                format!(" on track {} slot {}", track + 1, slot + 1)
                            }
                            _ => String::new(),
                        };
                        editor.handle_host_event(HostEvent::Status(format!(
                            "Saved agent effect artifact {}{}",
                            display_instrument_name(&result.effect_name),
                            target
                        )));
                    }
                    Err(error) => {
                        editor.handle_host_event(HostEvent::Error(error));
                    }
                }
            }
        }
        // ── Inline instrument/effect editor commands ──
        _ => {}
    }
}
