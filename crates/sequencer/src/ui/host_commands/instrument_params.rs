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
    let expanded_step_projection = ctx.shared.expanded_step_projection.clone();
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
                                expanded_step_projection: &expanded_step_projection,
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
                                    expanded_step_projection: &expanded_step_projection,
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
                                    expanded_step_projection: &expanded_step_projection,
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
                                    expanded_step_projection: &expanded_step_projection,
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
                        // Whether the *step* panel already lists a row for this
                        // p-lock. If it does, the row's LOCK readout is bound
                        // to the per-param SEQV field (see plocks.rs) and the
                        // targeted value sync below repaints it — so the row
                        // list itself does not need republishing, which would
                        // rerun the whole plock panel on every drag event.
                        let plock_row_existed = displayed_plock_step(
                            &state,
                            track,
                            selected_plock_step(&selected_steps),
                        )
                        .and_then(|step| {
                            state
                                .pattern
                                .instrument_slots
                                .get(track)
                                .and_then(|slot| slot.plocks.get(step, param_idx))
                        })
                        .is_some()
                            && matches!(
                                desc.kind,
                                sequencer::effects::ParamKind::Continuous { .. }
                            );
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
                                expanded_step_projection: &expanded_step_projection,
                                track,
                                param_idx,
                                display_step,
                                // Publish the row list only when the row set can
                                // have changed (first write of this lock, or a
                                // neural override). Later drag events repaint
                                // through the bound value field.
                                sync_plock_list: wrote_neural_plock || !plock_row_existed,
                                sync_plock_presence: !wrote_neural_plock,
                                sync_sampler_times: true,
                            },
                        );
                        // Same policy as "set-instrument-param": a continuous
                        // p-lock drag is fully covered by the targeted display
                        // syncs above. Bumping the epochs per drag event forced
                        // `SEQ.instrument-panel` to be rebuilt, which reruns the
                        // whole *fx* widget source (~30ms) to move one number.
                        // Only structural params (bool/enum) still need it.
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
                                    expanded_step_projection: &expanded_step_projection,
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

    fn reactive_number(editor: &Editor, field: &str) -> f64 {
        match editor.runtime().reactive_field_value("SEQ", field) {
            Some(Value::Number(n)) => *n,
            other => panic!("SEQ.{field} should be a number, got {other:?}"),
        }
    }

    /// The compact step-sequencer grid binds its p-lock tick to
    /// `seq-track-step-plock-kind-{track}-{step}` and its tint to the per-step
    /// `seq-track-step-variant-{r,g,b}-*` fields. A knob drag with a step
    /// selected no longer bumps `ui_epoch`, so the p-lock authoring path must
    /// publish those fields itself. Drives the real
    /// `dispatch_custom_host_command` -> `instrument_params::handle` seam
    /// rather than any sync helper, because both previous fixes for this bug
    /// were validated against helpers/mirrors and missed the real path.
    #[test]
    fn set_instrument_plock_publishes_the_compact_step_plock_render_fields() {
        const TRACK: usize = 0;
        const STEP: usize = 3;
        const PARAM: usize = 0;

        let state = Arc::new(sequencer::sequencer::SequencerState::new(
            1,
            vec![sequencer::sequencer::default_empty_effect_chain()],
        ));
        let descriptor = sequencer::effects::EffectDescriptor::builtin_filter();
        state.pattern.instrument_slots[TRACK].apply_descriptor(&descriptor, 1);
        let (keyboard_tx, _keyboard_rx) = std::sync::mpsc::channel();
        let mut app = app::App::new(
            state.clone(),
            sequencer::audiograph::LiveGraphPtr(std::ptr::null_mut()),
            44_100,
            app::AudioBuses {
                bus_l_id: 0,
                bus_r_id: 0,
                default_bus_nodes: Vec::new(),
                bus_gate_runtime: Arc::new(Mutex::new(Arc::new(Vec::new()))),
                bus_gate_playheads: Arc::new(Mutex::new(Vec::new())),
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

        let mut runtime = Runtime::new();
        runtime.register_reactive("SEQ", Vec::new(), true);
        let mut editor = Editor::new(runtime, eseqlisp::EditorConfig::default());

        let selected_steps = Arc::new(Mutex::new(HashSet::from([STEP])));
        let current_track = Arc::new(AtomicUsize::new(TRACK));
        let ui_epoch = Arc::new(AtomicUsize::new(0));
        let fx_epoch = Arc::new(AtomicUsize::new(0));
        let sample_db = Rc::new(
            sequencer::sample_db::SampleDb::open_in_memory().expect("open in-memory sample db"),
        );
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
            fx_value_epoch: Arc::new(AtomicUsize::new(0)),
            ui_invalidations: Arc::new(UiInvalidationQueue::new()),
            expanded_step_projection: Arc::new(ExpandedStepProjectionRegistry::new()),
            active_delete_target: Arc::new(Mutex::new(None)),
            active_delete_target_version: Arc::new(AtomicUsize::new(0)),
            auto_follow_override_until: Arc::new(Mutex::new(None)),
            track_pan_ids: Arc::new(Mutex::new(Vec::new())),
            track_collapsed: Arc::new(Mutex::new(app.track_collapsed.clone())),
            bus_state: Arc::new(Mutex::new(app.buses.clone())),
            bus_node_ids: Arc::new(Mutex::new(app.graph.bus_node_ids.clone())),
            track_groups: Arc::new(Mutex::new(app.groups.clone())),
            record_armed: Arc::new(Mutex::new(vec![false])),
            recording: Arc::new(AtomicBool::new(false)),
            master_recording: Arc::new(AtomicBool::new(false)),
            held_notes: Arc::new(Mutex::new(Vec::new())),
            roll_record: Arc::new(Mutex::new(RollRecordBuffer::default())),
            keyboard_octave: Arc::new(AtomicI32::new(0)),
            sample_browser: Rc::new(RefCell::new(DebouncedSampleBrowser::new(
                sample_db,
                Duration::from_millis(100),
            ))),
            keyboard_tx,
            accumulator_names: Arc::new(Mutex::new(Vec::new())),
            piano_roll_clipboard: super::super::super::new_piano_roll_clipboard(),
            arrangement_clipboard: app::song_region::new_arrangement_clipboard(),
            selected_drum_lane_steps: Arc::new(Mutex::new(HashSet::new())),
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
            cached_cpu_load_bits: 0.0f32.to_bits(),
            last_meter_poll_at: Instant::now(),
            last_cpu_ui_poll_at: Instant::now(),
            last_neural_visualization_poll_at: Instant::now(),
            visualization_liveness: VisualizationLiveness::default(),
            last_voice_count_log_at: Instant::now(),
        };
        let mut track_names = vec!["Track 1".to_string()];

        // Seed the per-step render bindings the way a full `ui_epoch` sync
        // would for a step with no p-locks: kind 0, black tint.
        {
            let rt = editor.runtime_mut();
            rt.set_reactive(
                "SEQ",
                &track_step_plock_kind_field(TRACK, STEP),
                Value::Number(0.0),
            );
            for channel in ['r', 'g', 'b'] {
                rt.set_reactive(
                    "SEQ",
                    &track_step_variant_color_field(TRACK, STEP, channel),
                    Value::Number(0.0),
                );
            }
        }
        assert_eq!(
            reactive_number(&editor, &track_step_plock_kind_field(TRACK, STEP)),
            0.0,
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
        assert_ne!(
            reactive_number(&editor, &track_step_plock_kind_field(TRACK, STEP)),
            0.0,
            "the compact grid's p-lock tick field must be published by the \
             p-lock authoring path (it no longer bumps ui_epoch)"
        );
        let tint: Vec<f64> = ['r', 'g', 'b']
            .into_iter()
            .map(|channel| {
                reactive_number(&editor, &track_step_variant_color_field(TRACK, STEP, channel))
            })
            .collect();
        assert!(
            tint.iter().any(|channel| *channel != 0.0),
            "the compact grid's per-step variant tint must be published too, got {tint:?}"
        );
        assert!(
            matches!(
                editor.runtime().reactive_field_value(
                    "SEQ",
                    &track_step_plocked_field(TRACK, STEP)
                ),
                Some(Value::Bool(true))
            ),
            "the per-step p-lock presence bool must stay in sync"
        );
    }
}
