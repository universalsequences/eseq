use crate::*;

pub(super) const COMMANDS: &[&str] = &[
    "set-instrument-param",
    "audition-instrument-key",
    "set-instrument-key-lock",
    "set-instrument-key-lock-multi",
    "set-instrument-key-lock-option",
    "set-instrument-key-lock-option-multi",
    "clear-instrument-key-lock",
    "clear-instrument-key-locks-for-note",
    "stamp-key-lock-variant",
    "toggle-instrument-param",
    "set-instrument-param-option",
    "set-instrument-plock",
    "set-instrument-tensor-cell",
    "set-instrument-plock-option",
    "set-instrument-base-note",
    "copy-instrument-values-to-all-scenes",
    "apply-polyphony-to-all-scenes",
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
    let keyboard_tx = ctx.shared.keyboard_tx.clone();
    match name {
        "set-instrument-param" => {
            if let Value::Map(ref map) = payload {
                let param_idx =
                    map.get("param-idx").and_then(|cell| match &*cell.borrow() {
                        Value::Number(n) => Some(*n as usize),
                        _ => None,
                    });
                let value = map.get("value").and_then(|cell| match &*cell.borrow() {
                    Value::Number(n) => Some(*n as f32),
                    _ => None,
                });
                if let (Some(param_idx), Some(user_val)) = (param_idx, value) {
                    let track = current_track.load(Ordering::Relaxed);
                    if let Some(desc) = app
                        .graph
                        .instrument_descriptors
                        .get(track)
                        .and_then(|d| d.params.get(param_idx))
                        .cloned()
                    {
                        let stored = desc.clamp(desc.user_input_to_stored(user_val));
                        let (neural_selection, wrote_neural_plock, neural_history_before) =
                            record_selected_neural_instrument_plock(
                                &mut editor,
                                &state,
                                &selected_neural_neurons,
                                track,
                                param_idx,
                                stored,
                            );
                        if let Some(before) = neural_history_before {
                            app.commit_applied_scene_structure_mutation(
                                before,
                                "Edit neural override",
                            );
                        }
                        let print_gesture = !wrote_neural_plock
                            && neural_selection.is_empty()
                            && selected_steps.lock().unwrap().is_empty()
                            && state.is_playing()
                            && ctx.shared.recording.load(Ordering::Relaxed);
                        if print_gesture {
                            // While record+play is active the knob is a
                            // recorder: latch the already-clamped value and
                            // leave the base value untouched. The reactive
                            // tick writes p-locks onto passing triggers.
                            let mut print = ctx.shared.step_print.lock().unwrap();
                            print.latch(
                                track,
                                PrintTarget::Instrument { param_idx },
                                stored,
                            );
                            // A cross-track touch may have replaced a Step
                            // target, so clear or republish its engine-only
                            // override at the same latch transition.
                            print.publish_engine_override(&state);
                            // The base write is skipped: the knob shows the
                            // latch through `param.value` (visual only — the
                            // sound stays step-quantized).
                            drop(print);
                        } else {
                            if wrote_neural_plock {
                                super::rebuild_panel_if_needed(ctx.shared, &desc);
                            } else {
                                super::apply_device_param_base(
                                    app,
                                    ctx.shared,
                                    (track, DeviceSlot::Instrument, param_idx),
                                    Some(&desc),
                                    stored,
                                    |app, command| {
                                        app::apply_command(app, command);
                                        true
                                    },
                                );
                            }
                        }
                    }
                }
            }
        }
        "audition-instrument-key" => {
            if let Value::Map(ref map) = payload {
                let note =
                    map_usize(map, "note").and_then(|note| u8::try_from(note).ok());
                if let Some(note) = note {
                    let track = current_track.load(Ordering::Relaxed);
                    if track < app.tracks.len() {
                        let base_note_offset = f32::from_bits(
                            app.state.pattern.instrument_base_note_offsets[track]
                                .load(Ordering::Relaxed),
                        );
                        let transpose = note as f32 - 60.0 - base_note_offset;
                        release_matching_key_lock_auditions(
                            &mut ctx.sessions.pending_key_lock_auditions,
                            &keyboard_tx,
                            track,
                            transpose,
                        );
                        if keyboard_tx
                            .send(sequencer::sequencer::LiveInputEvent::Note(KeyboardTrigger {
                                generation: 0,
                source: None,
                                track,
                                transpose,
                                velocity: 1.0,
                                note_off: false,
                            }))
                            .is_ok()
                        {
                            if let Some(id) = app.track_registry.id_at(track) {
                                app.retrospective.trig(id, transpose, Instant::now(), KEY_LOCK_AUDITION_DURATION);
                            }
                            ctx.sessions.pending_key_lock_auditions.push(PendingKeyLockAudition {
                                track,
                                transpose,
                                release_at: Instant::now() + KEY_LOCK_AUDITION_DURATION,
                            });
                        }
                    }
                }
            }
        }
        "set-instrument-key-lock" => {
            if let Value::Map(ref map) = payload {
                let param_idx = map_usize(map, "param-idx");
                let note =
                    map_usize(map, "note").and_then(|note| u8::try_from(note).ok());
                let value = map_number(map, "value").map(|value| value as f32);
                if let (Some(param_idx), Some(note), Some(user_val)) =
                    (param_idx, note, value)
                {
                    let track = current_track.load(Ordering::Relaxed);
                    if let Some(desc) = app
                        .graph
                        .instrument_descriptors
                        .get(track)
                        .and_then(|d| d.params.get(param_idx))
                        .cloned()
                    {
                        let stored = desc.clamp(desc.user_input_to_stored(user_val));
                        app::apply_command(
                            &mut app,
                            app::AppCommand::SetInstrumentKeyLock {
                                track,
                                note,
                                param_idx,
                                value: stored,
                            },
                        );
                        fx_epoch.fetch_add(1, Ordering::Relaxed);
                        ui_epoch.fetch_add(1, Ordering::Relaxed);
                    }
                }
            }
        }
        "set-instrument-key-lock-multi" => {
            if let Value::Map(ref map) = payload {
                let param_idx = map_usize(map, "param-idx");
                let notes = map_u8_list(map, "notes").filter(|notes| !notes.is_empty());
                let value = map_number(map, "value").map(|value| value as f32);
                if let (Some(param_idx), Some(notes), Some(user_val)) =
                    (param_idx, notes, value)
                {
                    let track = current_track.load(Ordering::Relaxed);
                    if let Some(desc) = app
                        .graph
                        .instrument_descriptors
                        .get(track)
                        .and_then(|d| d.params.get(param_idx))
                        .cloned()
                    {
                        let stored = desc.clamp(desc.user_input_to_stored(user_val));
                        app::apply_command(
                            &mut app,
                            app::AppCommand::SetInstrumentKeyLockMulti {
                                track,
                                notes,
                                param_idx,
                                value: stored,
                            },
                        );
                        fx_epoch.fetch_add(1, Ordering::Relaxed);
                        ui_epoch.fetch_add(1, Ordering::Relaxed);
                    }
                }
            }
        }
        "set-instrument-key-lock-option" => {
            if let Value::Map(ref map) = payload {
                let param_idx = map_usize(map, "param-idx");
                let note =
                    map_usize(map, "note").and_then(|note| u8::try_from(note).ok());
                let label = map_string(map, "label");
                if let (Some(param_idx), Some(note), Some(label)) =
                    (param_idx, note, label)
                {
                    let track = current_track.load(Ordering::Relaxed);
                    if let Some(sequencer::effects::ParamKind::Enum { labels }) = app
                        .graph
                        .instrument_descriptors
                        .get(track)
                        .and_then(|d| d.params.get(param_idx))
                        .map(|d| &d.kind)
                    {
                        if let Some(selected_idx) =
                            labels.iter().position(|item| item == &label)
                        {
                            app::apply_command(
                                &mut app,
                                app::AppCommand::SetInstrumentKeyLock {
                                    track,
                                    note,
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
        "set-instrument-key-lock-option-multi" => {
            if let Value::Map(ref map) = payload {
                let param_idx = map_usize(map, "param-idx");
                let notes = map_u8_list(map, "notes").filter(|notes| !notes.is_empty());
                let label = map_string(map, "label");
                if let (Some(param_idx), Some(notes), Some(label)) =
                    (param_idx, notes, label)
                {
                    let track = current_track.load(Ordering::Relaxed);
                    if let Some(sequencer::effects::ParamKind::Enum { labels }) = app
                        .graph
                        .instrument_descriptors
                        .get(track)
                        .and_then(|d| d.params.get(param_idx))
                        .map(|d| &d.kind)
                    {
                        if let Some(selected_idx) =
                            labels.iter().position(|item| item == &label)
                        {
                            app::apply_command(
                                &mut app,
                                app::AppCommand::SetInstrumentKeyLockMulti {
                                    track,
                                    notes,
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
        "clear-instrument-key-lock" => {
            if let Value::Map(ref map) = payload {
                let param_idx = map_usize(map, "param-idx");
                let note =
                    map_usize(map, "note").and_then(|note| u8::try_from(note).ok());
                if let (Some(param_idx), Some(note)) = (param_idx, note) {
                    let track = current_track.load(Ordering::Relaxed);
                    app::apply_command(
                        &mut app,
                        app::AppCommand::ClearInstrumentKeyLock {
                            track,
                            note,
                            param_idx,
                        },
                    );
                    fx_epoch.fetch_add(1, Ordering::Relaxed);
                    ui_epoch.fetch_add(1, Ordering::Relaxed);
                }
            }
        }
        "clear-instrument-key-locks-for-note" => {
            if let Value::Map(ref map) = payload {
                let note =
                    map_usize(map, "note").and_then(|note| u8::try_from(note).ok());
                if let Some(note) = note {
                    let track = current_track.load(Ordering::Relaxed);
                    app::apply_command(
                        &mut app,
                        app::AppCommand::ClearInstrumentKeyLocksForNote { track, note },
                    );
                    fx_epoch.fetch_add(1, Ordering::Relaxed);
                    ui_epoch.fetch_add(1, Ordering::Relaxed);
                }
            }
        }
        "stamp-key-lock-variant" => {
            if let Value::Map(ref map) = payload {
                let notes = map_u8_list(map, "notes").filter(|notes| !notes.is_empty());
                let label = map_string(map, "label");
                if let (Some(notes), Some(label)) = (notes, label) {
                    let track = current_track.load(Ordering::Relaxed);
                    let applied = if label == "def" {
                        app::apply_command(&mut app, key_variant_command(track, notes, None));
                        true
                    } else {
                        state
                            .key_lock_variant_registry_snapshot(track)
                            .assignment_for_label(&label)
                            .is_some_and(|assignment| {
                                let command =
                                    key_variant_command(track, notes, Some(assignment.key));
                                app::apply_command(&mut app, command);
                                true
                            })
                    };
                    if applied {
                        fx_epoch.fetch_add(1, Ordering::Relaxed);
                        ui_epoch.fetch_add(1, Ordering::Relaxed);
                    }
                }
            }
        }
        "toggle-instrument-param" => {
            if let Value::Map(ref map) = payload {
                let param_idx =
                    map.get("param-idx").and_then(|cell| match &*cell.borrow() {
                        Value::Number(n) => Some(*n as usize),
                        _ => None,
                    });
                if let Some(param_idx) = param_idx {
                    let track = current_track.load(Ordering::Relaxed);
                    if let Some(desc) = app
                        .graph
                        .instrument_descriptors
                        .get(track)
                        .and_then(|d| d.params.get(param_idx))
                        .cloned()
                    {
                        let slot = &app.state.pattern.instrument_slots[track];
                        let selected: Vec<usize> =
                            selected_steps.lock().unwrap().iter().copied().collect();
                        let neural_selection =
                            selected_neural_neurons.lock().unwrap().clone();
                        let default = if param_idx
                            < slot.num_params.load(Ordering::Relaxed) as usize
                        {
                            slot.defaults.get(param_idx)
                        } else {
                            desc.default
                        };
                        let current = sequencer::lisp_host::selected_neural_instrument_plock_value(
                            &state,
                            &neural_selection,
                            track,
                            param_idx,
                        )
                        .or_else(|| {
                            selected
                                .iter()
                                .copied()
                                .min()
                                .and_then(|step| slot.plocks.get(step, param_idx))
                        })
                        .unwrap_or(default);
                        let next = desc.clamp(if current > 0.5 { 0.0 } else { 1.0 });
                        let neural_history_before = (!neural_selection.is_empty())
                            .then(|| state.capture_project_scenes());
                        let wrote_neural_plock = write_selected_neural_instrument_plock(
                            &mut editor,
                            &state,
                            &neural_selection,
                            track,
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
                            // The override shows through the param's value.
                        } else if selected.is_empty() {
                            app::apply_command(
                                &mut app,
                                app::AppCommand::SetInstrumentParam {
                                    track,
                                    param_idx,
                                    value: next,
                                },
                            );
                        } else {
                            app::apply_command(
                                &mut app,
                                app::AppCommand::SetInstrumentPlockMulti {
                                    track,
                                    steps: selected,
                                    param_idx,
                                    value: next,
                                },
                            );
                            sync_instrument_plock_presence_display(&mut editor, &app, &state, track);
                        }
                        ui_epoch.fetch_add(1, Ordering::Relaxed);
                    }
                }
            }
        }
        "set-instrument-param-option" => {
            if let Value::Map(ref map) = payload {
                let param_idx =
                    map.get("param-idx").and_then(|cell| match &*cell.borrow() {
                        Value::Number(n) => Some(*n as usize),
                        _ => None,
                    });
                let label = map.get("label").and_then(|cell| match &*cell.borrow() {
                    Value::String(s) => Some(s.clone()),
                    _ => None,
                });
                if let (Some(param_idx), Some(label)) = (param_idx, label) {
                    let track = current_track.load(Ordering::Relaxed);
                    if let Some(sequencer::effects::ParamKind::Enum { labels }) = app
                        .graph
                        .instrument_descriptors
                        .get(track)
                        .and_then(|d| d.params.get(param_idx))
                        .map(|d| &d.kind)
                    {
                        if let Some(selected_idx) =
                            labels.iter().position(|item| item == &label)
                        {
                            let value = selected_idx as f32;
                            let (_, wrote_neural_plock, neural_history_before) =
                                record_selected_neural_instrument_plock(
                                    &mut editor,
                                    &state,
                                    &selected_neural_neurons,
                                    track,
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
                                    app::AppCommand::SetInstrumentParam {
                                        track,
                                        param_idx,
                                        value,
                                    },
                                );
                            }
                            ui_epoch.fetch_add(1, Ordering::Relaxed);
                        }
                    }
                }
            }
        }
        "set-instrument-plock" => {
            if let Value::Map(ref map) = payload {
                let param_idx =
                    map.get("param-idx").and_then(|cell| match &*cell.borrow() {
                        Value::Number(n) => Some(*n as usize),
                        _ => None,
                    });
                let value = map.get("value").and_then(|cell| match &*cell.borrow() {
                    Value::Number(n) => Some(*n as f32),
                    _ => None,
                });
                if let (Some(param_idx), Some(user_val)) = (param_idx, value) {
                    let track = current_track.load(Ordering::Relaxed);
                    if let Some(desc) = app
                        .graph
                        .instrument_descriptors
                        .get(track)
                        .and_then(|d| d.params.get(param_idx))
                        .cloned()
                    {
                        let stored = desc.clamp(desc.user_input_to_stored(user_val));
                        let (_, wrote_neural_plock, neural_history_before) =
                            record_selected_neural_instrument_plock(
                                &mut editor,
                                &state,
                                &selected_neural_neurons,
                                track,
                                param_idx,
                            stored,
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
                                app::AppCommand::SetInstrumentPlockMulti {
                                    track,
                                    steps,
                                    param_idx,
                                    value: stored,
                                },
                            );
                        }
                        if !wrote_neural_plock {
                            sync_instrument_plock_presence_display(&mut editor, &app, &state, track);
                        }
                        // Same policy as "set-instrument-param": a p-lock drag
                        // repaints through the param it binds (the host kinds
                        // push its value). Bumping the epochs per drag event
                        // reran the whole *fx* widget source (~30ms) to move
                        // one number; only a param whose value redefines model
                        // data (`param_change_needs_fx_rebuild`) bumps them.
                        if param_change_needs_fx_rebuild(&desc) {
                            fx_epoch.fetch_add(1, Ordering::Relaxed);
                            ui_epoch.fetch_add(1, Ordering::Relaxed);
                        }
                    }
                }
            }
        }
        "set-instrument-tensor-cell" => {
            if let Value::Map(ref map) = payload {
                let tensor_idx = map_usize(map, "tensor-idx");
                let value = map_number(map, "value").map(|value| value as f32);
                if let (Some(tensor_idx), Some(user_val)) = (tensor_idx, value) {
                    let track = current_track.load(Ordering::Relaxed);
                    if let Some(tensor_desc) = app
                        .graph
                        .instrument_descriptors
                        .get(track)
                        .and_then(|desc| desc.tensor_params.get(tensor_idx))
                        .cloned()
                    {
                        let cell_idx = map_usize(map, "cell-idx").or_else(|| {
                            let row = map_usize(map, "row")?;
                            let col = map_usize(map, "col")?;
                            (col < tensor_desc.cols())
                                .then_some(row * tensor_desc.cols() + col)
                        });
                        let Some(cell_idx) = cell_idx else {
                            return;
                        };
                        if cell_idx >= tensor_desc.default.len() {
                            return;
                        }
                        let value = user_val.clamp(tensor_desc.min, tensor_desc.max);
                        let steps: Vec<usize> =
                            selected_steps.lock().unwrap().iter().copied().collect();
                        if steps.is_empty() {
                            app::apply_command(
                                &mut app,
                                app::AppCommand::SetInstrumentTensorCell {
                                    track,
                                    tensor_idx,
                                    cell_idx,
                                    value,
                                },
                            );
                        } else {
                            app::apply_command(
                                &mut app,
                                app::AppCommand::SetInstrumentTensorPlockCellMulti {
                                    track,
                                    steps,
                                    tensor_idx,
                                    cell_idx,
                                    value,
                                },
                            );
                        }
                        ui_epoch.fetch_add(1, Ordering::Relaxed);
                    }
                }
            }
        }
        "set-instrument-plock-option" => {
            if let Value::Map(ref map) = payload {
                let param_idx =
                    map.get("param-idx").and_then(|cell| match &*cell.borrow() {
                        Value::Number(n) => Some(*n as usize),
                        _ => None,
                    });
                let label = map.get("label").and_then(|cell| match &*cell.borrow() {
                    Value::String(s) => Some(s.clone()),
                    _ => None,
                });
                if let (Some(param_idx), Some(label)) = (param_idx, label) {
                    let track = current_track.load(Ordering::Relaxed);
                    if let Some(sequencer::effects::ParamKind::Enum { labels }) = app
                        .graph
                        .instrument_descriptors
                        .get(track)
                        .and_then(|d| d.params.get(param_idx))
                        .map(|d| &d.kind)
                    {
                        if let Some(selected_idx) =
                            labels.iter().position(|item| item == &label)
                        {
                            let value = selected_idx as f32;
                            let (_, wrote_neural_plock, neural_history_before) =
                                record_selected_neural_instrument_plock(
                                    &mut editor,
                                    &state,
                                    &selected_neural_neurons,
                                    track,
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
                                    app::AppCommand::SetInstrumentPlockMulti {
                                        track,
                                        steps,
                                        param_idx,
                                        value,
                                    },
                                );
                            }
                            if !wrote_neural_plock {
                                sync_instrument_plock_presence_display(&mut editor, &app, &state, track);
                            }
                            ui_epoch.fetch_add(1, Ordering::Relaxed);
                        }
                    }
                }
            }
        }
        "set-instrument-base-note" => {
            if let Value::Map(ref map) = payload {
                let value = map.get("value").and_then(|cell| match &*cell.borrow() {
                    Value::Number(n) => Some(*n as f32),
                    _ => None,
                });
                if let Some(value) = value {
                    let track = current_track.load(Ordering::Relaxed);
                    let clamped = value.clamp(-48.0, 48.0);
                    app::apply_command(
                        &mut app,
                        app::AppCommand::SetInstrumentBaseNoteOffset {
                            track,
                            value: clamped,
                        },
                    );
                }
            }
        }
        "copy-instrument-values-to-all-scenes" => {
            let track = extract_usize_from_payload(&payload, "track");
            let rack_slot = extract_usize_from_payload(&payload, "rack-slot");
            // `:rack true` = the whole rack (every slot + macros), sent from
            // the rack header menu; `:rack-slot n` = one slot's instrument,
            // sent from that slot's instrument header menu.
            let whole_rack = extract_bool_from_payload(&payload, "rack");
            let updated = match (track, rack_slot, whole_rack) {
                (Some(track), _, true) => {
                    state.copy_current_rack_values_to_all_track_patterns(track)
                }
                (Some(track), Some(rack_slot), false) => state
                    .copy_current_rack_slot_instrument_values_to_all_track_patterns(
                        track, rack_slot,
                    ),
                (Some(track), None, false) => {
                    state.copy_current_instrument_values_to_all_track_patterns(track)
                }
                _ => 0,
            };
            if updated > 0 {
                let what = if whole_rack { "rack" } else { "instrument" };
                editor.handle_host_event(HostEvent::Status(format!(
                    "Copied {what} values to {updated} patterns/scenes"
                )));
            } else {
                editor.handle_host_event(HostEvent::Status(
                    "Could not copy instrument values: invalid instrument target"
                        .to_string(),
                ));
            }
        }
        // Right-click "Apply to all scenes" on the header's mono/poly button:
        // only the polyphony choice, not the rest of the instrument values.
        "apply-polyphony-to-all-scenes" => {
            let track = extract_usize_from_payload(&payload, "track");
            let rack_slot = extract_usize_from_payload(&payload, "rack-slot");
            let updated = track
                .map(|track| state.copy_current_polyphony_to_all_track_patterns(track, rack_slot))
                .unwrap_or(0);
            let status = if updated > 0 {
                format!("Applied mono/poly to {updated} patterns/scenes")
            } else {
                "Could not apply mono/poly: invalid instrument target".to_string()
            };
            editor.handle_host_event(HostEvent::Status(status));
        }
        _ => {}
    }
}

/// The command stamping key-lock variant `key` onto `notes` of `track`'s
/// instrument, or clearing those keys' variant locks (`None`). Shared by
/// `stamp-key-lock-variant` and the host kinds' `stamp-key-variant`.
pub(super) fn key_variant_command(
    track: usize,
    notes: Vec<u8>,
    key: Option<sequencer::plock_variants::PlockVariantKey>,
) -> app::AppCommand {
    match key {
        None => app::AppCommand::ClearInstrumentKeyLockVariantsForNotes { track, notes },
        Some(key) => app::AppCommand::StampInstrumentKeyLockVariant { track, notes, key },
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

    fn number_payload(entries: &[(&str, f64)]) -> Value {
        Value::Map(
            entries
                .iter()
                .map(|(key, value)| {
                    (
                        (*key).to_string(),
                        Rc::new(RefCell::new(Value::Number(*value))),
                    )
                })
                .collect(),
        )
    }

    fn effect_option_payload(slot_idx: usize, node_id: u32, param_idx: usize, label: &str) -> Value {
        Value::Map(
            [
                ("slot-idx".to_string(), Value::Number(slot_idx as f64)),
                ("target-node-id".to_string(), Value::Number(node_id as f64)),
                ("param-idx".to_string(), Value::Number(param_idx as f64)),
                ("label".to_string(), Value::String(label.to_string())),
            ]
            .into_iter()
            .map(|(key, value)| (key, Rc::new(RefCell::new(value))))
            .collect(),
        )
    }

    fn effect_batch_payload(
        slot_idx: usize,
        node_id: u32,
        updates: &[(usize, f32)],
    ) -> Value {
        let updates = updates
            .iter()
            .map(|(param_idx, value)| {
                Rc::new(RefCell::new(Value::Map(
                    [
                        ("param-idx".to_string(), Value::Number(*param_idx as f64)),
                        ("value".to_string(), Value::Number(*value as f64)),
                    ]
                    .into_iter()
                    .map(|(key, value)| (key, Rc::new(RefCell::new(value))))
                    .collect(),
                )))
            })
            .collect();
        Value::Map(
            [
                ("slot-idx".to_string(), Value::Number(slot_idx as f64)),
                ("target-node-id".to_string(), Value::Number(node_id as f64)),
                ("updates".to_string(), Value::List(updates)),
            ]
            .into_iter()
            .map(|(key, value)| (key, Rc::new(RefCell::new(value))))
            .collect(),
        )
    }

    /// The step grids draw a step's p-lock tick and tint from the kinds'
    /// `step.lock-kind` / `variant-color`, which read the p-lock render
    /// (`plock_variant_step_render_values`). A knob drag with a step
    /// selected no longer bumps `ui_epoch`, so the p-lock authoring path must
    /// leave that render current itself. Drives the real
    /// `dispatch_custom_host_command` -> `instrument_params::handle` seam
    /// rather than any sync helper, because both previous fixes for this bug
    /// were validated against helpers/mirrors and missed the real path.
    #[test]
    fn device_param_commands_print_while_recording() {
        const TRACK: usize = 0;
        const STEP: usize = 3;
        const PARAM: usize = 0;

        let state = Arc::new(sequencer::sequencer::SequencerState::new(
            1,
            vec![sequencer::sequencer::default_empty_effect_chain()],
        ));
        let descriptor = sequencer::effects::EffectDescriptor::builtin_filter();
        state.pattern.instrument_slots[TRACK].apply_descriptor(&descriptor, 1);
        const EFFECT_NODE_ID: u32 = 42;
        state.pattern.effect_chains[TRACK][0]
            .apply_descriptor(&descriptor, EFFECT_NODE_ID);
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
        app.graph.instrument_descriptors = vec![descriptor.clone()];
        app.graph.effect_descriptors = vec![vec![descriptor.clone()]];

        let midi_descriptor = sequencer::lisp_host::load_midi_fx_descriptor("arp")
            .expect("builtin arp descriptor");
        state.pattern.track_params[TRACK]
            .set_midi_fx_chain(vec![midi_descriptor.name.clone()]);
        state.pattern.midi_fx_slots[TRACK][0].apply_descriptor(&midi_descriptor, 43);

        app.buses[0].effect_descriptors = vec![descriptor.clone()];
        app.buses[0].effect_slots = vec![
            sequencer::effects::EffectSlotSnapshot::new_default(&descriptor, 44),
        ];

        let sampler_descriptor = sequencer::effects::EffectDescriptor::builtin_sampler();
        let mut rack_effect_slots =
            sequencer::sequencer::RackSlotSnapshot::empty_effect_slots();
        rack_effect_slots[0] =
            sequencer::effects::EffectSlotSnapshot::new_default_with_modulator(
                &descriptor,
                45,
                0,
            );
        let mut rack_effect_descriptors =
            sequencer::effects::EffectDescriptor::default_full_chain();
        rack_effect_descriptors[0] = descriptor.clone();
        state.set_rack_track_for_all_pattern_snapshots(
            TRACK,
            sequencer::sequencer::RackTrackSnapshot::new(
                vec![sequencer::sequencer::RackSlotSnapshot {
                    instrument_type: sequencer::sequencer::InstrumentType::Sampler,
                    instrument_run_mode:
                        sequencer::sequencer::CustomInstrumentRunMode::Instrument,
                    instrument_base_note_offset: 0.0,
                    choke_group: None,
                    gain: 1.0,
                    pan: 0.0,
                    mute: false,
                    solo: false,
                    enabled: true,
                    max_polyphony: 8,
                    param_plocks: sequencer::sequencer::RackSlotParamPlocks::new(),
                    instrument_slot:
                        sequencer::effects::EffectSlotSnapshot::new_default_with_modulator(
                            &sampler_descriptor,
                            46,
                            0,
                        ),
                    effect_slots: rack_effect_slots,
                    effect_descriptors: rack_effect_descriptors,
                    custom_effect_names:
                        sequencer::sequencer::RackSlotSnapshot::empty_effect_names(),
                    track_sound_state: sequencer::sequencer::TrackSoundState::default(),
                    sample_id: Some((1, "test.wav".to_string(), 44_100)),
                }],
                sequencer::sequencer::default_rack_macros(),
            ),
        );

        let mut runtime = Runtime::new();
        runtime.register_reactive("SEQ", Vec::new(), true);
        let mut editor = Editor::new(runtime, eseqlisp::EditorConfig::default());

        let selected_steps = Arc::new(Mutex::new(HashSet::from([STEP])));
        let current_track = Arc::new(AtomicUsize::new(TRACK));
        let ui_epoch = Arc::new(AtomicUsize::new(0));
        let fx_epoch = Arc::new(AtomicUsize::new(0));
        let sample_db = sequencer::sample_db::SampleDb::open_in_memory().expect("open in-memory sample db");
        let shared = SharedHandles {
            state: state.clone(),
            lg_raw: std::ptr::null_mut(),
            current_track: current_track.clone(),
            selected_tracks: Arc::new(Mutex::new(HashSet::new())),
            selected_steps: selected_steps.clone(),
            selected_neural_neurons: Arc::new(Mutex::new(BTreeSet::new())),
            piano_roll_selection: Arc::new(Mutex::new(HashSet::new())),
            piano_roll_move_state: Arc::new(Mutex::new(None)),
            piano_roll_focus: super::super::super::new_shared_piano_roll_focus(),
            step_clipboard: Arc::new(Mutex::new(None)),
            ui_epoch: ui_epoch.clone(),
            fx_epoch: fx_epoch.clone(),
            ui_invalidations: Arc::new(UiInvalidationQueue::new()),
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
            cached_mod_port_levels: Default::default(),
            cached_mod_display_values: Default::default(),
            watched_display_modulators: std::collections::HashSet::new(),
            mod_display_poll_fx_epoch: usize::MAX,
                mod_display_poll_track: None,
            cached_cpu_load_bits: 0.0f32.to_bits(),
            last_meter_poll_at: Instant::now(),
            last_cpu_ui_poll_at: Instant::now(),
            last_voice_count_log_at: Instant::now(),
        };
        let mut track_names = vec!["Track 1".to_string()];

        let render =
            |state: &Arc<SequencerState>| plock_variant_step_render_values(state, TRACK)[STEP];
        assert_eq!(
            render(&state).kind,
            0,
            "precondition: the selected step starts with no p-lock tick"
        );

        let mut ctx = LoopCtx {
            sessions: &mut sessions,
            meters: &mut meters,
            frame: &mut frame,
            gesture: &mut gesture,
            track_names: &mut track_names,
            shared: &shared,
        };
        dispatch_custom_host_command(
            "set-instrument-plock",
            number_payload(&[("param-idx", PARAM as f64), ("value", 0.42)]),
            &mut app,
            &mut editor,
            &mut ctx,
        );

        assert!(
            state.pattern.instrument_slots[TRACK]
                .plocks
                .get(STEP, PARAM)
                .is_some(),
            "the handler must have written the instrument p-lock"
        );
        // The step grids' p-lock tick and tint (`step.lock-kind`,
        // `variant-color`, `plocked`) read this render.
        assert_ne!(render(&state).kind, 0, "the step now carries a p-lock tick");
        assert!(
            render(&state).color.iter().any(|channel| *channel != 0.0),
            "the step's variant tint is set too"
        );

        // With no selection, record+play diverts the normal base-param host
        // command into the print latch. The base stays untouched and the tick
        // writes only the triggered step under the playhead.
        const PRINT_STEP: usize = 5;
        selected_steps.lock().unwrap().clear();
        state.pattern.patterns[TRACK].toggle_step(PRINT_STEP);
        state.transport.track_playheads[TRACK].store(PRINT_STEP as u32, Ordering::Relaxed);
        state.transport.playing.store(true, Ordering::Relaxed);
        shared.recording.store(true, Ordering::Relaxed);
        let default_before = state.pattern.instrument_slots[TRACK].defaults.get(PARAM);
        let print_epochs_before = (
            fx_epoch.load(Ordering::Relaxed),
            ui_epoch.load(Ordering::Relaxed),
        );
        // Replay consecutive real handler updates from one drag before the
        // frame tick. Only the latest held value should print, and the tick
        // must enqueue one presence batch rather than rebuilding *fx*.
        dispatch_custom_host_command(
            "set-instrument-param",
            number_payload(&[("param-idx", PARAM as f64), ("value", 0.73)]),
            &mut app,
            &mut editor,
            &mut ctx,
        );
        dispatch_custom_host_command(
            "set-instrument-param",
            number_payload(&[("param-idx", PARAM as f64), ("value", 0.74)]),
            &mut app,
            &mut editor,
            &mut ctx,
        );
        assert_eq!(
            state.pattern.instrument_slots[TRACK].defaults.get(PARAM),
            default_before,
            "printing must never write the instrument base value"
        );
        // The print branch skips the base write: the latch holds the latest
        // value right now, before any playhead crossing, and the knob shows
        // it through `param.value` (host_kinds::tests::panel::
        // a_param_held_by_the_print_latch_shows_the_latched_value). Without
        // it the knob only moves once per step (very visible at slow BPM).
        assert_eq!(
            shared
                .step_print
                .lock()
                .unwrap()
                .latched(TRACK, PrintTarget::Instrument { param_idx: PARAM }),
            Some(0.74),
            "the print latch must hold the latest value before any step crossing"
        );
        assert_eq!(
            (
                fx_epoch.load(Ordering::Relaxed),
                ui_epoch.load(Ordering::Relaxed),
            ),
            print_epochs_before,
            "the display-only latch mirror must not bump any epoch"
        );
        let tick = tick_step_print(&mut app, &shared);
        assert!(tick.printed);
        assert_eq!(
            state.pattern.instrument_slots[TRACK].plocks.get(PRINT_STEP, PARAM),
            Some(0.74)
        );
        // The scheduler resolves instrument p-locks from its published
        // snapshot (a deep copy), not live SlotPLockData, so the tick must
        // have republished the track — otherwise the printed gesture stays
        // inaudible until the next transport restart.
        assert_eq!(
            state.latest_scheduler_snapshot().tracks[TRACK]
                .instrument_slot
                .plocks[PRINT_STEP][PARAM],
            Some(0.74),
            "printed instrument p-lock reaches the published scheduler snapshot",
        );
        assert_eq!(
            (
                fx_epoch.load(Ordering::Relaxed),
                ui_epoch.load(Ordering::Relaxed),
            ),
            print_epochs_before,
            "a printed drag must not rebuild the fx or whole UI trees"
        );
        assert_eq!(
            shared.ui_invalidations.drain(),
            vec![UiInvalidation::StepInvalidationBatch {
                track: TRACK,
                steps: vec![PRINT_STEP],
                change: StepInvalidation::PlockPresence,
            }],
            "all printed params for one tick must share one targeted presence invalidation"
        );
        shared
            .step_print
            .lock()
            .unwrap()
            .release_device_param_gesture(&state);
        assert!(!shared.step_print.lock().unwrap().armed());

        // Track-effect scalar, batch, and enum-option commands all use the
        // same print gate. The stale reported index proves each latch keys on
        // the slot resolved from the stable node id, not the payload index.
        const EFFECT_SLOT: usize = 0;
        const STALE_REPORTED_SLOT: usize = 7;
        let effect_slot = &state.pattern.effect_chains[TRACK][EFFECT_SLOT];
        let effect_default_before = effect_slot.defaults.get(2);
        let epochs_before = (
            fx_epoch.load(Ordering::Relaxed),
            ui_epoch.load(Ordering::Relaxed),
        );
        dispatch_custom_host_command(
            "set-effect-param",
            number_payload(&[
                ("slot-idx", STALE_REPORTED_SLOT as f64),
                ("target-node-id", EFFECT_NODE_ID as f64),
                ("param-idx", 2.0),
                ("value", 1_800.0),
            ]),
            &mut app,
            &mut editor,
            &mut ctx,
        );
        assert_eq!(effect_slot.defaults.get(2), effect_default_before);
        // Same contract for a track effect knob: the latch holds the value
        // at once (its `param.value` shows it), with no epoch bump.
        let target = PrintTarget::Effect {
            slot_idx: EFFECT_SLOT,
            param_idx: 2,
        };
        assert_eq!(
            shared.step_print.lock().unwrap().latched(TRACK, target),
            Some(1_800.0),
            "the effect knob's latch must hold the value before any step crossing"
        );
        assert_eq!(
            (
                fx_epoch.load(Ordering::Relaxed),
                ui_epoch.load(Ordering::Relaxed),
            ),
            epochs_before,
            "the effect display-only latch mirror must not bump any epoch"
        );
        assert!(tick_step_print(&mut app, &shared).printed);
        assert_eq!(effect_slot.plocks.get(PRINT_STEP, 2), Some(1_800.0));
        assert_eq!(
            state.latest_scheduler_snapshot().tracks[TRACK].effect_slots
                [EFFECT_SLOT]
                .plocks[PRINT_STEP][2],
            Some(1_800.0),
            "printed effect p-lock reaches the published scheduler snapshot",
        );
        shared
            .step_print
            .lock()
            .unwrap()
            .release_device_param_gesture(&state);

        dispatch_custom_host_command(
            "set-effect-param-batch",
            effect_batch_payload(
                STALE_REPORTED_SLOT,
                EFFECT_NODE_ID,
                &[(2, 2_200.0), (3, 0.8)],
            ),
            &mut app,
            &mut editor,
            &mut ctx,
        );
        assert!(tick_step_print(&mut app, &shared).printed);
        assert_eq!(effect_slot.plocks.get(PRINT_STEP, 2), Some(2_200.0));
        assert_eq!(effect_slot.plocks.get(PRINT_STEP, 3), Some(0.8));
        shared
            .step_print
            .lock()
            .unwrap()
            .release_device_param_gesture(&state);

        dispatch_custom_host_command(
            "set-effect-param-option",
            effect_option_payload(
                STALE_REPORTED_SLOT,
                EFFECT_NODE_ID,
                1,
                "highpass",
            ),
            &mut app,
            &mut editor,
            &mut ctx,
        );
        assert!(tick_step_print(&mut app, &shared).printed);
        assert_eq!(effect_slot.plocks.get(PRINT_STEP, 1), Some(1.0));
        assert_eq!(effect_slot.defaults.get(1), descriptor.params[1].default);
        assert_eq!(
            (
                fx_epoch.load(Ordering::Relaxed),
                ui_epoch.load(Ordering::Relaxed),
            ),
            epochs_before,
            "effect printing must not rebuild the fx or whole UI trees"
        );
        shared
            .step_print
            .lock()
            .unwrap()
            .release_device_param_gesture(&state);

        // Extended scalar targets share the same gate and one tick. Their
        // defaults remain untouched while each family receives a p-lock.
        let midi_default_before =
            state.pattern.midi_fx_slots[TRACK][0].defaults.get(0);
        let bus_default_before = app.buses[0].effect_slots[0].defaults[2];
        let rack_before = state.pattern.rack_tracks.lock().unwrap()[TRACK]
            .as_ref()
            .unwrap()
            .clone();
        dispatch_custom_host_command(
            "set-midi-fx-param",
            number_payload(&[("slot-idx", 0.0), ("param-idx", 0.0), ("value", 6.0)]),
            &mut app,
            &mut editor,
            &mut ctx,
        );
        // Tick the MIDI-FX print alone: the scheduler applies MIDI-FX p-locks
        // from its published snapshot, not live slot state, so this tick must
        // republish the track by itself (no rack target may piggyback the
        // publish).
        assert!(tick_step_print(&mut app, &shared).printed);
        assert_eq!(
            state.pattern.midi_fx_slots[TRACK][0].defaults.get(0),
            midi_default_before,
        );
        assert_eq!(
            state.pattern.midi_fx_slots[TRACK][0].plocks.get(PRINT_STEP, 0),
            Some(6.0),
        );
        assert_eq!(
            state.latest_scheduler_snapshot().tracks[TRACK].midi_fx_slots[0]
                .plocks[PRINT_STEP][0],
            Some(6.0),
            "printed MIDI-FX p-lock reaches the published scheduler snapshot",
        );
        dispatch_custom_host_command(
            "set-bus-effect-param",
            number_payload(&[
                ("bus", 0.0),
                ("slot-idx", 0.0),
                ("param-idx", 2.0),
                ("value", 1_600.0),
            ]),
            &mut app,
            &mut editor,
            &mut ctx,
        );
        dispatch_custom_host_command(
            "set-rack-slot-gain",
            number_payload(&[("track", 0.0), ("slot", 0.0), ("value", 1.5)]),
            &mut app,
            &mut editor,
            &mut ctx,
        );
        dispatch_custom_host_command(
            "set-rack-slot-instrument-param",
            number_payload(&[
                ("track", 0.0),
                ("slot", 0.0),
                ("param-idx", 8.0),
                ("value", 22_050.0),
            ]),
            &mut app,
            &mut editor,
            &mut ctx,
        );
        dispatch_custom_host_command(
            "set-rack-slot-effect-param",
            number_payload(&[
                ("track", 0.0),
                ("rack-slot", 0.0),
                ("effect-slot", 0.0),
                ("param", 2.0),
                ("value", 1_200.0),
            ]),
            &mut app,
            &mut editor,
            &mut ctx,
        );
        dispatch_custom_host_command(
            "set-rack-macro-value",
            number_payload(&[("track", 0.0), ("id", 0.0), ("value", 0.65)]),
            &mut app,
            &mut editor,
            &mut ctx,
        );
        assert!(tick_step_print(&mut app, &shared).printed);
        assert_eq!(app.buses[0].effect_slots[0].defaults[2], bus_default_before);
        assert_eq!(app.buses[0].effect_slots[0].plocks[PRINT_STEP][2], Some(1_600.0));
        let racks = state.pattern.rack_tracks.lock().unwrap();
        let rack = racks[TRACK].as_ref().unwrap();
        assert_eq!(rack.slots[0].gain, rack_before.slots[0].gain);
        assert_eq!(
            rack.slots[0]
                .param_plocks
                .get(PRINT_STEP, sequencer::sequencer::RackSlotParam::Gain),
            Some(1.5),
        );
        assert_eq!(
            rack.slots[0].instrument_slot.defaults,
            rack_before.slots[0].instrument_slot.defaults,
        );
        assert_eq!(
            rack.slots[0].instrument_slot.plocks[PRINT_STEP][8],
            Some(22_050.0),
        );
        assert_eq!(
            rack.slots[0].effect_slots[0].defaults,
            rack_before.slots[0].effect_slots[0].defaults,
        );
        assert_eq!(rack.slots[0].effect_slots[0].plocks[PRINT_STEP][2], Some(1_200.0));
        assert_eq!(rack.macros[0].value, rack_before.macros[0].value);
        assert_eq!(rack.macros[0].plocks[PRINT_STEP], Some(0.65));
        drop(racks);
        shared
            .step_print
            .lock()
            .unwrap()
            .release_device_param_gesture(&state);

        // If the record gate races off, the same payload falls through to the
        // normal base edit rather than being dropped.
        shared.recording.store(false, Ordering::Relaxed);
        dispatch_custom_host_command(
            "set-instrument-param",
            number_payload(&[("param-idx", PARAM as f64), ("value", 0.31)]),
            &mut app,
            &mut editor,
            &mut ctx,
        );
        assert_eq!(
            state.pattern.instrument_slots[TRACK].defaults.get(PARAM),
            0.31
        );
    }
}
