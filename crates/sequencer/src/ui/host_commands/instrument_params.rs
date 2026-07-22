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
                        if !wrote_neural_plock {
                            app::apply_command(
                                &mut app,
                                app::AppCommand::SetInstrumentParam {
                                    track,
                                    param_idx,
                                    value: stored,
                                },
                            );
                        }
                        sync_instrument_param_authoring_display(
                            &mut editor,
                            InstrumentParamDisplaySync {
                                app: &app,
                                state: &state,
                                selected_steps: &selected_steps,
                                selection: &neural_selection,
                                track,
                                param_idx,
                                display_step: None,
                                sync_plock_list: wrote_neural_plock,
                                sync_plock_presence: false,
                                sync_sampler_times: true,
                            },
                        );
                        if param_change_needs_fx_rebuild(&desc) {
                            fx_epoch.fetch_add(1, Ordering::Relaxed);
                            ui_epoch.fetch_add(1, Ordering::Relaxed);
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
                            .send(KeyboardTrigger {
                                track,
                                transpose,
                                velocity: 1.0,
                                note_off: false,
                            })
                            .is_ok()
                        {
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
                        app::apply_command(
                            &mut app,
                            app::AppCommand::ClearInstrumentKeyLockVariantsForNotes {
                                track,
                                notes,
                            },
                        );
                        true
                    } else {
                        state
                            .key_lock_variant_registry_snapshot(track)
                            .assignment_for_label(&label)
                            .is_some_and(|assignment| {
                                app::apply_command(
                                    &mut app,
                                    app::AppCommand::StampInstrumentKeyLockVariant {
                                        track,
                                        notes,
                                        key: assignment.key,
                                    },
                                );
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
                            sync_instrument_param_authoring_display(
                                &mut editor,
                                InstrumentParamDisplaySync {
                                    app: &app,
                                    state: &state,
                                    selected_steps: &selected_steps,
                                    selection: &neural_selection,
                                    track,
                                    param_idx,
                                    display_step: None,
                                    sync_plock_list: true,
                                    sync_plock_presence: false,
                                    sync_sampler_times: false,
                                },
                            );
                        } else if selected.is_empty() {
                            app::apply_command(
                                &mut app,
                                app::AppCommand::SetInstrumentParam {
                                    track,
                                    param_idx,
                                    value: next,
                                },
                            );
                            if sync_instrument_param_value_field(
                                editor.runtime_mut(),
                                &app,
                                track,
                                param_idx,
                                None,
                            ) {
                                editor.mark_needs_redraw();
                            }
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
                            let display_step = displayed_plock_step(
                                &state,
                                track,
                                selected_plock_step(&selected_steps),
                            );
                            sync_instrument_param_authoring_display(
                                &mut editor,
                                InstrumentParamDisplaySync {
                                    app: &app,
                                    state: &state,
                                    selected_steps: &selected_steps,
                                    selection: &neural_selection,
                                    track,
                                    param_idx,
                                    display_step,
                                    sync_plock_list: false,
                                    sync_plock_presence: true,
                                    sync_sampler_times: false,
                                },
                            );
                        }
                        fx_epoch.fetch_add(1, Ordering::Relaxed);
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
                            let (neural_selection, wrote_neural_plock, neural_history_before) =
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
                            sync_instrument_param_authoring_display(
                                &mut editor,
                                InstrumentParamDisplaySync {
                                    app: &app,
                                    state: &state,
                                    selected_steps: &selected_steps,
                                    selection: &neural_selection,
                                    track,
                                    param_idx,
                                    display_step: None,
                                    sync_plock_list: wrote_neural_plock,
                                    sync_plock_presence: false,
                                    sync_sampler_times: false,
                                },
                            );
                            fx_epoch.fetch_add(1, Ordering::Relaxed);
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
                        let display_step = displayed_plock_step(
                            &state,
                            track,
                            selected_plock_step(&selected_steps),
                        );
                        sync_instrument_param_authoring_display(
                            &mut editor,
                            InstrumentParamDisplaySync {
                                app: &app,
                                state: &state,
                                selected_steps: &selected_steps,
                                selection: &neural_selection,
                                track,
                                param_idx,
                                display_step,
                                sync_plock_list: wrote_neural_plock,
                                sync_plock_presence: !wrote_neural_plock,
                                sync_sampler_times: true,
                            },
                        );
                        fx_epoch.fetch_add(1, Ordering::Relaxed);
                        ui_epoch.fetch_add(1, Ordering::Relaxed);
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
                        let display_step = displayed_plock_step(
                            &state,
                            track,
                            selected_plock_step(&selected_steps),
                        );
                        if sync_instrument_tensor_value_field(
                            editor.runtime_mut(),
                            &app,
                            track,
                            tensor_idx,
                            display_step,
                        ) {
                            editor.refresh_runtime_side_effects();
                            editor.mark_needs_redraw();
                        }
                        fx_epoch.fetch_add(1, Ordering::Relaxed);
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
                            let (neural_selection, wrote_neural_plock, neural_history_before) =
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
                            let display_step = displayed_plock_step(
                                &state,
                                track,
                                selected_plock_step(&selected_steps),
                            );
                            sync_instrument_param_authoring_display(
                                &mut editor,
                                InstrumentParamDisplaySync {
                                    app: &app,
                                    state: &state,
                                    selected_steps: &selected_steps,
                                    selection: &neural_selection,
                                    track,
                                    param_idx,
                                    display_step,
                                    sync_plock_list: wrote_neural_plock,
                                    sync_plock_presence: !wrote_neural_plock,
                                    sync_sampler_times: false,
                                },
                            );
                            fx_epoch.fetch_add(1, Ordering::Relaxed);
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
                    sync_instrument_base_note_value_field(
                        editor.runtime_mut(),
                        &app,
                        track,
                    );
                }
            }
        }
        "copy-instrument-values-to-all-scenes" => {
            let track = extract_usize_from_payload(&payload, "track");
            let rack_slot = extract_usize_from_payload(&payload, "rack-slot");
            let updated = match (track, rack_slot) {
                (Some(track), Some(rack_slot)) => state
                    .copy_current_rack_slot_instrument_values_to_all_track_patterns(
                        track, rack_slot,
                    ),
                (Some(track), None) => {
                    state.copy_current_instrument_values_to_all_track_patterns(track)
                }
                _ => 0,
            };
            if updated > 0 {
                editor.handle_host_event(HostEvent::Status(format!(
                    "Copied instrument values to {updated} patterns/scenes"
                )));
            } else {
                editor.handle_host_event(HostEvent::Status(
                    "Could not copy instrument values: invalid instrument target"
                        .to_string(),
                ));
            }
        }
        _ => {}
    }
}
