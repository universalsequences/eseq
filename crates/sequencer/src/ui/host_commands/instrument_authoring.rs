use crate::*;

pub(super) const COMMANDS: &[&str] = &[
    "enter-new-instrument-editor",
    "set-draft-instrument-run-mode",
    "save-active-editor-macro",
    "save-new-instrument",
    "enter-edit-instrument",
    "update-instrument",
    "preview-instrument-patch",
    "preview-effect-patch",
    "evaluate-editor-source",
    "promote-editor-to-patch",
    "eject-editor-to-code",
    "toggle-instrument-patcher-source",
    "enter-new-effect-editor",
    "save-new-effect",
    "enter-edit-effect",
    "update-effect",
    "cancel-editor",
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
    let piano_roll_selection = ctx.shared.piano_roll_selection.clone();
    let ui_epoch = ctx.shared.ui_epoch.clone();
    let fx_epoch = ctx.shared.fx_epoch.clone();
    let track_pan_ids = ctx.shared.track_pan_ids.clone();
    let track_collapsed = ctx.shared.track_collapsed.clone();
    let bus_state = ctx.shared.bus_state.clone();
    let record_armed = ctx.shared.record_armed.clone();
    let accumulator_names = ctx.shared.accumulator_names.clone();
    match name {
        "enter-new-instrument-editor" => {
            if ctx.sessions.editor_mode.is_some() || ctx.sessions.instrument_edit_session.is_some() {
                editor.handle_host_event(HostEvent::Error(
                    "Close the current editor before creating a new instrument"
                        .to_string(),
                ));
                return;
            }
            let original_track = current_track.load(Ordering::Relaxed);
            let temp_dir = match create_new_instrument_draft_dir() {
                Ok(dir) => dir,
                Err(error) => {
                    editor.handle_host_event(HostEvent::Error(error));
                    return;
                }
            };
            let file_path = temp_dir.join("dsp.lisp");
            if let Err(error) = std::fs::write(&file_path, NEW_INSTRUMENT_STARTER_DSP) {
                let _ = std::fs::remove_dir_all(&temp_dir);
                editor.handle_host_event(HostEvent::Error(format!(
                    "Failed to write starter instrument: {error}"
                )));
                return;
            }

            let draft_track = match app.add_transient_instrument_track_sync(
                NEW_INSTRUMENT_DRAFT_NAME,
                NEW_INSTRUMENT_STARTER_DSP,
                Some(&temp_dir),
            ) {
                Ok(track) => track,
                Err(error) => {
                    let _ = std::fs::remove_dir_all(&temp_dir);
                    editor.handle_host_event(HostEvent::Error(format!(
                        "Failed to create draft instrument track: {error}"
                    )));
                    return;
                }
            };
            let _ = app.force_instrument_enabled(draft_track);
            sync_after_instrument_track_apply(
                &mut app,
                &mut editor,
                &state,
                draft_track,
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

            let Some(engine_id) = app
                .graph
                .track_engine_ids
                .get(draft_track)
                .and_then(|id| *id)
            else {
                let _ = app.graph_controller().delete_track(draft_track);
                let _ = std::fs::remove_dir_all(&temp_dir);
                editor.handle_host_event(HostEvent::Error(
                    "Draft instrument track has no engine binding".to_string(),
                ));
                return;
            };

            let buf_name = "*instrument-patcher:new-instrument*".to_string();
            editor.remove_buffer_by_name(&buf_name);
            editor.create_scratch_buffer(&buf_name, "", BufferMode::ESeqLisp);
            let patcher_source =
                instrument_patcher_buffer_source(&buf_name, &file_path);
            if let Err(error) = editor.runtime_mut().eval_str(&patcher_source) {
                let _ = app.graph_controller().delete_track(draft_track);
                let _ = std::fs::remove_dir_all(&temp_dir);
                editor.handle_host_event(HostEvent::Error(format!(
                    "Failed to build patch editor: {error:?}"
                )));
                editor.remove_buffer_by_name(&buf_name);
                return;
            }
            reset_instrument_patcher_state(&file_path);
            let layout_source = show_instrument_patcher_layout_source(&buf_name);
            if let Err(error) = editor.runtime_mut().eval_str(&layout_source) {
                let _ = app.graph_controller().delete_track(draft_track);
                let _ = std::fs::remove_dir_all(&temp_dir);
                editor.handle_host_event(HostEvent::Error(format!(
                    "Failed to show patch editor: {error:?}"
                )));
                editor.remove_buffer_by_name(&buf_name);
                return;
            }
            ctx.sessions.editor_buffer_name = Some(buf_name.clone());
            ctx.sessions.editor_mode = Some("new-instrument".to_string());
            ctx.sessions.instrument_edit_session = Some(InstrumentEditSession::begin_create_draft(
                NEW_INSTRUMENT_DRAFT_NAME.to_string(),
                file_path,
                buf_name.clone(),
                engine_id,
                NEW_INSTRUMENT_STARTER_DSP.to_string(),
                temp_dir,
                draft_track,
                original_track,
            ));
            let rt = editor.runtime_mut();
            let _ = rt.eval_str("(set! sbrowser-editor-name \"\")");
            rt.set_reactive("SEQ", "editor-active", Value::Bool(true));
            rt.set_reactive(
                "SEQ",
                "editor-mode",
                Value::String("new-instrument".to_string()),
            );
            rt.set_reactive("SEQ", "editor-error", Value::String(String::new()));
            rt.set_reactive(
                "SEQ",
                "editor-buffer-name",
                Value::String(buf_name.clone()),
            );
            rt.set_reactive(
                "SEQ",
                "editor-instrument-run-mode",
                Value::String("instrument".to_string()),
            );
            rt.set_reactive(
                "SEQ",
                "editor-surface",
                Value::String("patch".to_string()),
            );
            rt.run_reactive_cycle();
            editor.refresh_runtime_side_effects();
            editor.handle_host_event(HostEvent::Status(format!(
                "Created draft instrument track {}",
                draft_track + 1
            )));
        }

        "set-draft-instrument-run-mode" => {
            let Some(session) = ctx.sessions.instrument_edit_session.as_mut() else {
                editor.handle_host_event(HostEvent::Status(
                    "No instrument edit session is active".to_string(),
                ));
                return;
            };
            if !matches!(&session.mode, InstrumentEditMode::CreateDraft { .. }) {
                let rt = editor.runtime_mut();
                rt.set_reactive(
                    "SEQ",
                    "editor-error",
                    Value::String(
                        "Run mode can only be changed for draft instruments"
                            .to_string(),
                    ),
                );
                rt.run_reactive_cycle();
                editor.refresh_runtime_side_effects();
                return;
            }
            let requested = extract_string_from_payload(&payload, "run-mode")
                .unwrap_or_else(|| "instrument".to_string());
            let Some(run_mode) = instrument_run_mode_from_label(&requested) else {
                let rt = editor.runtime_mut();
                rt.set_reactive(
                    "SEQ",
                    "editor-error",
                    Value::String(format!("Unknown instrument run mode '{requested}'")),
                );
                rt.run_reactive_cycle();
                editor.refresh_runtime_side_effects();
                return;
            };
            match app
                .graph_controller()
                .set_track_instrument_run_mode(session.track, run_mode)
            {
                Ok(()) => {
                    session.run_mode = run_mode;
                    if let Some(engine_id) = app.graph.track_engine_ids[session.track] {
                        session.engine_id = engine_id;
                    }
                    let rt = editor.runtime_mut();
                    rt.set_reactive(
                        "SEQ",
                        "editor-instrument-run-mode",
                        Value::String(instrument_run_mode_label(run_mode).to_string()),
                    );
                    rt.set_reactive(
                        "SEQ",
                        "editor-error",
                        Value::String(String::new()),
                    );
                    rt.run_reactive_cycle();
                    editor.refresh_runtime_side_effects();
                    editor.handle_host_event(HostEvent::Status(format!(
                        "Draft instrument mode: {}",
                        match run_mode {
                            CustomInstrumentRunMode::Instrument => "Instrument",
                            CustomInstrumentRunMode::FreePatch => "Free Patch",
                        }
                    )));
                }
                Err(error) => {
                    let rt = editor.runtime_mut();
                    rt.set_reactive("SEQ", "editor-error", Value::String(error));
                    rt.run_reactive_cycle();
                    editor.refresh_runtime_side_effects();
                }
            }
        }

        "save-active-editor-macro" => {
            let result = if let Some(session) = ctx.sessions.instrument_edit_session.as_mut() {
                apply_active_instrument_editor_macro_action(session)
            } else if let Some(session) = ctx.sessions.effect_edit_session.as_mut() {
                apply_active_effect_editor_macro_action(session)
            } else {
                Err("No patch editor session is active".to_string())
            };
            match result {
                Ok(Some(result)) => {
                    let action_status = macro_library_action_status(&result);
                    let editor_macro_action = ctx.sessions.instrument_edit_session
                        .as_ref()
                        .and_then(active_instrument_editor_macro_action)
                        .or_else(|| {
                            ctx.sessions.effect_edit_session
                                .as_ref()
                                .and_then(active_effect_editor_macro_action)
                        });
                    let editor_macro_action =
                        editor_macro_action_strings(editor_macro_action.as_ref());
                    let rt = editor.runtime_mut();
                    rt.set_reactive(
                        "SEQ",
                        "editor-active-macro-name",
                        Value::String(editor_macro_action.0.clone()),
                    );
                    rt.set_reactive(
                        "SEQ",
                        "editor-active-macro-action",
                        Value::String(editor_macro_action.1.clone()),
                    );
                    rt.set_reactive(
                        "SEQ",
                        "editor-error",
                        Value::String(String::new()),
                    );
                    ctx.frame.prev_editor_macro_action = editor_macro_action;
                    rt.run_reactive_cycle();
                    editor.refresh_runtime_side_effects();
                    editor.refresh_visible_layouts_for_buffer_named("*samples*");
                    editor.handle_host_event(HostEvent::Status(action_status));
                }
                Ok(None) => {
                    let rt = editor.runtime_mut();
                    rt.set_reactive(
                        "SEQ",
                        "editor-error",
                        Value::String("No active macro is selected".to_string()),
                    );
                    rt.run_reactive_cycle();
                    editor.refresh_runtime_side_effects();
                    editor.refresh_visible_layouts_for_buffer_named("*samples*");
                }
                Err(error) => {
                    let rt = editor.runtime_mut();
                    rt.set_reactive("SEQ", "editor-error", Value::String(error));
                    rt.run_reactive_cycle();
                    editor.refresh_runtime_side_effects();
                    editor.refresh_visible_layouts_for_buffer_named("*samples*");
                }
            }
        }

        "save-new-instrument" => {
            if let Value::Map(ref map) = payload {
                if let Some(cell) = map.get("name") {
                    if let Value::String(inst_name) = &*cell.borrow() {
                        let inst_name = inst_name.trim().to_string();
                        if inst_name.is_empty() {
                            let rt = editor.runtime_mut();
                            rt.set_reactive(
                                "SEQ",
                                "editor-error",
                                Value::String("Name cannot be empty".to_string()),
                            );
                            rt.run_reactive_cycle();
                            editor.refresh_runtime_side_effects();
                            return;
                        }
                        let Some(session) = ctx.sessions.instrument_edit_session.as_ref() else {
                            let rt = editor.runtime_mut();
                            rt.set_reactive(
                                "SEQ",
                                "editor-error",
                                Value::String(
                                    "No draft instrument session is active".to_string(),
                                ),
                            );
                            rt.run_reactive_cycle();
                            editor.refresh_runtime_side_effects();
                            return;
                        };
                        if !matches!(
                            &session.mode,
                            InstrumentEditMode::CreateDraft { .. }
                        ) {
                            let rt = editor.runtime_mut();
                            rt.set_reactive(
                                "SEQ",
                                "editor-error",
                                Value::String(
                                    "Current editor session is not a draft instrument"
                                        .to_string(),
                                ),
                            );
                            rt.run_reactive_cycle();
                            editor.refresh_runtime_side_effects();
                            return;
                        }
                        if !session.visible_revision_valid {
                            let rt = editor.runtime_mut();
                            rt.set_reactive(
                                "SEQ",
                                "editor-error",
                                Value::String(
                                    "Cannot finalize: the current patch has errors"
                                        .to_string(),
                                ),
                            );
                            rt.run_reactive_cycle();
                            editor.refresh_runtime_side_effects();
                            return;
                        }

                        let flushed_macros =
                            match flush_staged_instrument_library_macro_edits(session) {
                                Ok(macros) => macros,
                                Err(error) => {
                                    let rt = editor.runtime_mut();
                                    rt.set_reactive(
                                        "SEQ",
                                        "editor-error",
                                        Value::String(format!(
                                            "Failed to save library macro edits: {error}"
                                        )),
                                    );
                                    rt.run_reactive_cycle();
                                    editor.refresh_runtime_side_effects();
                                    return;
                                }
                            };

                        let final_slug =
                            sequencer::agent::actions::normalize_patch_name(
                                &inst_name,
                                "new-instrument",
                            );
                        let final_name = format!("{final_slug}/");
                        let (final_dir, legacy_file) =
                            finalized_instrument_storage_paths(&final_slug);
                        if final_dir.exists() || legacy_file.exists() {
                            let rt = editor.runtime_mut();
                            rt.set_reactive(
                                "SEQ",
                                "editor-error",
                                Value::String(format!(
                                    "Instrument '{final_slug}' already exists"
                                )),
                            );
                            rt.run_reactive_cycle();
                            editor.refresh_runtime_side_effects();
                            return;
                        }

                        let source = session.last_valid_source.clone();
                        let draft_track = match &session.mode {
                            InstrumentEditMode::CreateDraft { draft_track, .. } => {
                                *draft_track
                            }
                            InstrumentEditMode::EditExisting { .. } => unreachable!(),
                        };
                        if let Err(error) =
                            sequencer::lisp_host::save_instrument(&final_name, &source)
                        {
                            let _ = std::fs::remove_dir_all(&final_dir);
                            let rt = editor.runtime_mut();
                            rt.set_reactive(
                                "SEQ",
                                "editor-error",
                                Value::String(format!(
                                    "Failed to save finalized instrument: {error}"
                                )),
                            );
                            rt.run_reactive_cycle();
                            editor.refresh_runtime_side_effects();
                            return;
                        }
                        if let Err(error) =
                            sequencer::lisp_host::save_instrument_run_mode(
                                &final_name,
                                session.run_mode,
                            )
                        {
                            let _ = std::fs::remove_dir_all(&final_dir);
                            let rt = editor.runtime_mut();
                            rt.set_reactive(
                                "SEQ",
                                "editor-error",
                                Value::String(format!(
                                    "Failed to save finalized instrument mode: {error}"
                                )),
                            );
                            rt.run_reactive_cycle();
                            editor.refresh_runtime_side_effects();
                            return;
                        }
                        let target_dsp = final_dir.join("dsp.lisp");
                        if let Some(layout) = session.last_valid_layout.as_deref() {
                            if let Err(error) =
                                write_patcher_layout_sidecar(&target_dsp, layout)
                            {
                                let _ = std::fs::remove_dir_all(&final_dir);
                                let rt = editor.runtime_mut();
                                rt.set_reactive(
                                    "SEQ",
                                    "editor-error",
                                    Value::String(format!(
                                        "Failed to save finalized instrument layout: {error}"
                                    )),
                                );
                                rt.run_reactive_cycle();
                                editor.refresh_runtime_side_effects();
                                return;
                            }
                        } else if let InstrumentEditMode::CreateDraft {
                            temp_dir, ..
                        } = &session.mode
                        {
                            let source_dsp = temp_dir.join("dsp.lisp");
                            if let Err(error) =
                                copy_patcher_layout_sidecar(&source_dsp, &target_dsp)
                            {
                                let _ = std::fs::remove_dir_all(&final_dir);
                                let rt = editor.runtime_mut();
                                rt.set_reactive(
                                    "SEQ",
                                    "editor-error",
                                    Value::String(format!(
                                        "Failed to save finalized instrument layout: {error}"
                                    )),
                                );
                                rt.run_reactive_cycle();
                                editor.refresh_runtime_side_effects();
                                return;
                            }
                        }
                        if let Err(error) = app.replace_custom_instrument_track_sync(
                            draft_track,
                            &final_name,
                            &source,
                        ) {
                            let _ = std::fs::remove_dir_all(&final_dir);
                            let rt = editor.runtime_mut();
                            rt.set_reactive(
                                "SEQ",
                                "editor-error",
                                Value::String(format!(
                                    "Failed to load finalized instrument: {error}"
                                )),
                            );
                            rt.run_reactive_cycle();
                            editor.refresh_runtime_side_effects();
                            return;
                        }
                        if let Err(error) =
                            app.graph_controller().set_track_instrument_run_mode(
                                draft_track,
                                session.run_mode,
                            )
                        {
                            let rt = editor.runtime_mut();
                            rt.set_reactive(
                                "SEQ",
                                "editor-error",
                                Value::String(format!(
                                    "Failed to apply finalized instrument mode: {error}"
                                )),
                            );
                            rt.run_reactive_cycle();
                            editor.refresh_runtime_side_effects();
                            return;
                        }

                        let session =
                            ctx.sessions.instrument_edit_session.take().expect("session checked");
                        if let InstrumentEditMode::CreateDraft { temp_dir, .. } =
                            session.mode
                        {
                            let _ = std::fs::remove_dir_all(temp_dir);
                        }
                        reset_instrument_patcher_state(&session.path);
                        let buf_name = session.buffer_name;
                        if let Err(error) = editor
                            .runtime_mut()
                            .eval_str(restore_instrument_patcher_layout_source())
                        {
                            editor.handle_host_event(HostEvent::Error(format!(
                                "Failed to restore main editor layout: {error:?}"
                            )));
                        }
                        editor.refresh_runtime_side_effects();
                        editor.remove_buffer_by_name(&buf_name);
                        if let Some(status) =
                            staged_library_macro_flush_status(&flushed_macros)
                        {
                            editor.handle_host_event(HostEvent::Status(status));
                        }
                        ctx.sessions.editor_buffer_name = None;
                        ctx.sessions.editor_mode = None;
                        current_track.store(draft_track, Ordering::Relaxed);
                        app.ui.cursor_track = draft_track;
                        *ctx.track_names = app.tracks.clone();
                        sync_shared_track_collapsed(&track_collapsed, &app);

                        let rt = editor.runtime_mut();
                        rt.set_reactive("SEQ", "editor-active", Value::Bool(false));
                        rt.set_reactive(
                            "SEQ",
                            "editor-mode",
                            Value::String(String::new()),
                        );
                        rt.set_reactive(
                            "SEQ",
                            "editor-error",
                            Value::String(String::new()),
                        );
                        rt.set_reactive(
                            "SEQ",
                            "editor-instrument-run-mode",
                            Value::String("instrument".to_string()),
                        );
                        rt.set_reactive(
                            "SEQ",
                            "editor-buffer-name",
                            Value::String(String::new()),
                        );
                        rt.set_reactive(
                            "SEQ",
                            "track-names",
                            build_track_names(&app.tracks),
                        );
                        rt.set_reactive(
                            "SEQ",
                            "instrument-panel",
                            build_instrument_panel_value(
                                &app,
                                draft_track,
                                &selected_steps,
                            ),
                        );
                        sync_sidebar_browser(rt, &app, draft_track);
                        rt.run_reactive_cycle();
                        editor.refresh_runtime_side_effects();
                        editor.refresh_visible_layouts_for_buffer_named("*fx*");
                        ui_epoch.fetch_add(1, Ordering::Relaxed);
                        editor.handle_host_event(HostEvent::Status(format!(
                            "Finalized instrument '{}' on track {}",
                            display_instrument_name(&final_name),
                            draft_track + 1
                        )));
                    }
                }
            }
        }

        "enter-edit-instrument" => {
            if let Value::Map(ref map) = payload {
                if let Some(cell) = map.get("name") {
                    if let Value::String(inst_name) = &*cell.borrow() {
                        let inst_name = inst_name.clone();
                        let file_path =
                            match sequencer::lisp_host::instrument_source_path(
                                &inst_name,
                            ) {
                                Ok(path) => path,
                                Err(e) => {
                                    editor.handle_host_event(HostEvent::Error(
                                        format!("Instrument file not found: {e}"),
                                    ));
                                    return;
                                }
                            };
                        if !file_path.exists() {
                            editor.handle_host_event(HostEvent::Error(format!(
                                "Instrument file not found: {}",
                                file_path.display()
                            )));
                            return;
                        }
                        let track = current_track.load(Ordering::Relaxed);
                        let Some(engine_id) =
                            app.graph.track_engine_ids.get(track).and_then(|id| *id)
                        else {
                            editor.handle_host_event(HostEvent::Error(
                                "Current instrument track has no engine binding"
                                    .to_string(),
                            ));
                            return;
                        };
                        let persisted_source = match std::fs::read_to_string(&file_path)
                        {
                            Ok(source) => source,
                            Err(error) => {
                                editor.handle_host_event(HostEvent::Error(format!(
                                    "Failed to read '{}': {error}",
                                    file_path.display()
                                )));
                                return;
                            }
                        };
                        let run_mode =
                            match sequencer::lisp_host::load_instrument_run_mode(
                                &inst_name,
                            ) {
                                Ok(run_mode) => run_mode,
                                Err(error) => {
                                    editor.handle_host_event(HostEvent::Error(
                                        format!(
                                            "Failed to load instrument mode: {error}"
                                        ),
                                    ));
                                    return;
                                }
                            };
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
                        let surface = editor_surface_for_existing(
                            &file_path,
                            &persisted_source,
                            eseqlisp::widget_render::patcher::PatcherIntent::Instrument,
                        );
                        let buf_name = match surface {
                            EditorSurface::Patch => {
                                let buf_name =
                                    format!("*instrument-patcher:{inst_name}*");
                                editor.remove_buffer_by_name(&buf_name);
                                editor.create_scratch_buffer(
                                    &buf_name,
                                    "",
                                    BufferMode::ESeqLisp,
                                );
                                let patcher_source = instrument_patcher_buffer_source(
                                    &buf_name, &file_path,
                                );
                                if let Err(error) =
                                    editor.runtime_mut().eval_str(&patcher_source)
                                {
                                    editor.handle_host_event(HostEvent::Error(
                                        format!(
                                            "Failed to build patch editor: {error:?}"
                                        ),
                                    ));
                                    editor.remove_buffer_by_name(&buf_name);
                                    return;
                                }
                                reset_instrument_patcher_state(&file_path);
                                buf_name
                            }
                            EditorSurface::Code => {
                                let buf_name = instrument_code_buffer_name(&inst_name);
                                editor.remove_buffer_by_name(&buf_name);
                                editor.create_scratch_buffer(
                                    &buf_name,
                                    &persisted_source,
                                    BufferMode::DGenLisp,
                                );
                                buf_name
                            }
                        };
                        let layout_source =
                            show_instrument_patcher_layout_source(&buf_name);
                        if let Err(error) =
                            editor.runtime_mut().eval_str(&layout_source)
                        {
                            editor.handle_host_event(HostEvent::Error(format!(
                                "Failed to show patch editor: {error:?}"
                            )));
                            editor.remove_buffer_by_name(&buf_name);
                            return;
                        }
                        editor.refresh_runtime_side_effects();
                        ctx.sessions.editor_buffer_name = Some(buf_name.clone());
                        ctx.sessions.editor_mode = Some("edit-instrument".to_string());
                        ctx.sessions.instrument_edit_session =
                            Some(InstrumentEditSession::begin_edit_existing(
                                inst_name,
                                file_path,
                                buf_name.clone(),
                                engine_id,
                                track,
                                persisted_source,
                                run_mode,
                                surface,
                            ));
                        let rt = editor.runtime_mut();
                        rt.set_reactive("SEQ", "editor-active", Value::Bool(true));
                        rt.set_reactive(
                            "SEQ",
                            "editor-mode",
                            Value::String("edit-instrument".to_string()),
                        );
                        rt.set_reactive(
                            "SEQ",
                            "editor-error",
                            Value::String(String::new()),
                        );
                        rt.set_reactive(
                            "SEQ",
                            "editor-buffer-name",
                            Value::String(buf_name),
                        );
                        rt.set_reactive(
                            "SEQ",
                            "editor-instrument-run-mode",
                            Value::String(
                                instrument_run_mode_label(run_mode).to_string(),
                            ),
                        );
                        rt.set_reactive(
                            "SEQ",
                            "editor-surface",
                            Value::String(
                                editor_surface_label(surface).to_string(),
                            ),
                        );
                        rt.run_reactive_cycle();
                        editor.refresh_runtime_side_effects();
                        refresh_visible_track_topology_layouts(&mut editor);
                    }
                }
            }
        }

        "update-instrument" => {
            if let Value::Map(ref map) = payload {
                if let Some(cell) = map.get("name") {
                    if let Value::String(inst_name) = &*cell.borrow() {
                        let inst_name = inst_name.clone();
                        if let Some(session) = ctx.sessions.instrument_edit_session.as_ref() {
                            let code_surface =
                                session.surface == EditorSurface::Code;
                            if !code_surface && !session.visible_revision_valid {
                                let rt = editor.runtime_mut();
                                rt.set_reactive(
                                    "SEQ",
                                    "editor-error",
                                    Value::String(
                                        "Cannot save: the current patch has errors"
                                            .to_string(),
                                    ),
                                );
                                rt.run_reactive_cycle();
                                editor.refresh_runtime_side_effects();
                                return;
                            }
                            let flushed_macros = if code_surface {
                                Vec::new()
                            } else {
                                match flush_staged_instrument_library_macro_edits(
                                    session,
                                ) {
                                    Ok(macros) => macros,
                                    Err(error) => {
                                        let rt = editor.runtime_mut();
                                        rt.set_reactive(
                                            "SEQ",
                                            "editor-error",
                                            Value::String(format!(
                                                "Failed to save library macro edits: {error}"
                                            )),
                                        );
                                        rt.run_reactive_cycle();
                                        editor.refresh_runtime_side_effects();
                                        return;
                                    }
                                }
                            };
                            // Code sessions save the buffer text verbatim so
                            // comments/formatting survive; patch sessions save
                            // the last compiled emission.
                            let source_to_save = if code_surface {
                                match editor.read_buffer_text(&session.buffer_name)
                                {
                                    Some(text) => text,
                                    None => {
                                        editor.handle_host_event(HostEvent::Error(
                                            format!(
                                                "Code buffer '{}' is missing",
                                                session.buffer_name
                                            ),
                                        ));
                                        return;
                                    }
                                }
                            } else {
                                session.last_valid_source.clone()
                            };
                            let unevaluated_changes = code_surface
                                && source_to_save != session.last_valid_source;
                            if let Err(e) = std::fs::write(
                                &session.path,
                                &source_to_save,
                            ) {
                                let rt = editor.runtime_mut();
                                rt.set_reactive(
                                    "SEQ",
                                    "editor-error",
                                    Value::String(format!("Failed to save: {e}")),
                                );
                                rt.run_reactive_cycle();
                                editor.refresh_runtime_side_effects();
                                return;
                            }
                            if let Some(layout) = session.last_valid_layout.as_deref() {
                                if let Err(e) =
                                    write_patcher_layout_sidecar(&session.path, layout)
                                {
                                    let rt = editor.runtime_mut();
                                    rt.set_reactive(
                                        "SEQ",
                                        "editor-error",
                                        Value::String(format!(
                                            "Failed to save layout: {e}"
                                        )),
                                    );
                                    rt.run_reactive_cycle();
                                    editor.refresh_runtime_side_effects();
                                    return;
                                }
                            }

                            let buf_name = session.buffer_name.clone();
                            reset_instrument_patcher_state(&session.path);
                            if let Err(error) = editor
                                .runtime_mut()
                                .eval_str(restore_instrument_patcher_layout_source())
                            {
                                editor.handle_host_event(HostEvent::Error(format!(
                                    "Failed to restore main editor layout: {error:?}"
                                )));
                                return;
                            }
                            editor.refresh_runtime_side_effects();
                            editor.remove_buffer_by_name(&buf_name);
                            if let Some(status) =
                                staged_library_macro_flush_status(&flushed_macros)
                            {
                                editor.handle_host_event(HostEvent::Status(status));
                            }
                            ctx.sessions.editor_buffer_name = None;
                            ctx.sessions.editor_mode = None;
                            ctx.sessions.instrument_edit_session = None;

                            let ct = current_track.load(Ordering::Relaxed);
                            ctx.track_names[ct] = inst_name.clone();
                            let rt = editor.runtime_mut();
                            rt.set_reactive("SEQ", "editor-active", Value::Bool(false));
                            rt.set_reactive(
                                "SEQ",
                                "editor-mode",
                                Value::String(String::new()),
                            );
                            rt.set_reactive(
                                "SEQ",
                                "editor-error",
                                Value::String(String::new()),
                            );
                            rt.set_reactive(
                                "SEQ",
                                "editor-buffer-name",
                                Value::String(String::new()),
                            );
                            rt.set_reactive(
                                "SEQ",
                                "editor-instrument-run-mode",
                                Value::String("instrument".to_string()),
                            );
                            rt.set_reactive(
                                "SEQ",
                                "track-names",
                                build_track_names(&ctx.track_names),
                            );
                            rt.run_reactive_cycle();
                            editor.refresh_runtime_side_effects();
                            ui_epoch.fetch_add(1, Ordering::Relaxed);
                            editor.handle_host_event(HostEvent::Status(
                                if unevaluated_changes {
                                    format!(
                                        "Saved instrument '{inst_name}' with unevaluated changes (not hot-swapped)"
                                    )
                                } else {
                                    format!("Saved instrument '{inst_name}'")
                                },
                            ));
                            return;
                        }
                        let buf_name = ctx.sessions.editor_buffer_name.clone().unwrap_or_default();
                        let source =
                            editor.read_buffer_text(&buf_name).unwrap_or_default();

                        if let Err(e) =
                            sequencer::lisp_host::save_instrument(&inst_name, &source)
                        {
                            let rt = editor.runtime_mut();
                            rt.set_reactive(
                                "SEQ",
                                "editor-error",
                                Value::String(format!("Failed to save: {e}")),
                            );
                            rt.run_reactive_cycle();
                            editor.refresh_runtime_side_effects();
                            return;
                        }

                        // Try hot-swap FIRST — stay in editor on failure
                        app.ui.cursor_track = current_track.load(Ordering::Relaxed);
                        match app
                            .replace_current_custom_instrument_sync(&inst_name, &source)
                        {
                            Ok(()) => {
                                // Success — close editor
                                editor
                                    .swap_buffer_in_tile_showing(&buf_name, "*sequencer*");
                                editor.remove_buffer_by_name(&buf_name);
                                ctx.sessions.editor_buffer_name = None;
                                ctx.sessions.editor_mode = None;

                                let ct = current_track.load(Ordering::Relaxed);
                                ctx.track_names[ct] = inst_name.clone();
                                let rt = editor.runtime_mut();
                                rt.set_reactive(
                                    "SEQ",
                                    "editor-active",
                                    Value::Bool(false),
                                );
                                rt.set_reactive(
                                    "SEQ",
                                    "editor-mode",
                                    Value::String(String::new()),
                                );
                                rt.set_reactive(
                                    "SEQ",
                                    "editor-error",
                                    Value::String(String::new()),
                                );
                                rt.set_reactive(
                                    "SEQ",
                                    "editor-buffer-name",
                                    Value::String(String::new()),
                                );
                                rt.set_reactive(
                                    "SEQ",
                                    "editor-instrument-run-mode",
                                    Value::String("instrument".to_string()),
                                );
                                rt.set_reactive(
                                    "SEQ",
                                    "track-names",
                                    build_track_names(&ctx.track_names),
                                );
                                rt.set_reactive(
                                    "SEQ",
                                    "instrument-panel",
                                    build_instrument_panel_value(
                                        &app,
                                        ct,
                                        &selected_steps,
                                    ),
                                );
                                rt.set_reactive(
                                    "SEQ",
                                    "effects",
                                    build_effects_value(
                                        &state,
                                        ct,
                                        &app.graph.effect_descriptors,
                                        &selected_steps,
                                    ),
                                );
                                rt.set_reactive(
                                    "SEQ",
                                    "midi-effects",
                                    build_midi_effects_value(
                                        &state,
                                        ct,
                                        &selected_steps,
                                    ),
                                );
                                rt.run_reactive_cycle();
                                editor.refresh_runtime_side_effects();
                                ui_epoch.fetch_add(1, Ordering::Relaxed);
                                editor.handle_host_event(HostEvent::Status(format!(
                                    "Hot-swapped instrument '{inst_name}'"
                                )));
                            }
                            Err(e) => {
                                // Compile failed — stay in editor, show error
                                let rt = editor.runtime_mut();
                                rt.set_reactive(
                                    "SEQ",
                                    "editor-error",
                                    Value::String(format!("{e}")),
                                );
                                rt.run_reactive_cycle();
                                editor.refresh_runtime_side_effects();
                            }
                        }
                    }
                }
            }
        }

        "preview-instrument-patch" => {
            let Some(session) = ctx.sessions.instrument_edit_session.as_mut() else {
                editor.handle_host_event(HostEvent::Status(
                    "No instrument edit session is active".to_string(),
                ));
                return;
            };
            let status = extract_string_from_payload(&payload, "status")
                .unwrap_or_else(|| "invalid".to_string());
            if status == "agentic-submit" {
                let Some(path) = extract_string_from_payload(&payload, "path") else {
                    editor.handle_host_event(HostEvent::Status(
                        "Agentic bubble request missing patch path".to_string(),
                    ));
                    return;
                };
                let Some(bubble_id) =
                    extract_string_from_payload(&payload, "bubble-id")
                else {
                    editor.handle_host_event(HostEvent::Status(
                        "Agentic bubble request missing bubble id".to_string(),
                    ));
                    return;
                };
                let generation = extract_usize_from_payload(&payload, "generation")
                    .unwrap_or(0) as u64;
                let prompt =
                    extract_string_from_payload(&payload, "prompt").unwrap_or_default();
                let macro_name = extract_string_from_payload(&payload, "macro-name")
                    .unwrap_or_else(|| "agentic-macro".to_string());
                let target = extract_string_from_payload(&payload, "target")
                    .unwrap_or_else(|| "create-macro".to_string());
                let intent = match extract_string_from_payload(&payload, "intent")
                    .as_deref()
                {
                    Some("effect") => {
                        eseqlisp::widget_render::patcher::PatcherIntent::Effect
                    }
                    _ => eseqlisp::widget_render::patcher::PatcherIntent::Instrument,
                };
                let task_key = format!("{path}::{bubble_id}");
                eprintln!(
                    "[agentic-bubble] host submit key={} generation={} intent={:?} macro={} prompt={:?}",
                    task_key, generation, intent, macro_name, prompt
                );
                let (tx, rx) = std::sync::mpsc::channel();
                let follow_up = if target == "edit-macro" {
                    let existing_macro_name =
                        extract_string_from_payload(&payload, "existing-macro-name")
                            .unwrap_or_else(|| macro_name.clone());
                    let params =
                        extract_string_from_payload(&payload, "existing-macro-params")
                            .unwrap_or_default()
                            .split_whitespace()
                            .map(str::to_string)
                            .collect::<Vec<_>>();
                    let source =
                        extract_string_from_payload(&payload, "existing-macro-source")
                            .unwrap_or_default();
                    Some(sequencer::agent::agentic_bubble::AgenticBubbleFollowUp {
                        macro_name: existing_macro_name,
                        params,
                        source,
                    })
                } else {
                    None
                };
                let request = sequencer::agent::agentic_bubble::AgenticBubbleRequest {
                    prompt,
                    suggested_macro_name: macro_name.clone(),
                    follow_up,
                };
                std::thread::spawn(move || {
                    let result =
                        sequencer::agent::agentic_bubble::generate_agentic_bubble_macro(
                            request,
                        );
                    let _ = tx.send(result);
                });
                ctx.sessions.pending_agentic_bubbles.insert(
                    task_key,
                    PendingAgenticBubble {
                        path: PathBuf::from(path),
                        intent,
                        bubble_id,
                        generation,
                        receiver: rx,
                    },
                );
                editor.handle_host_event(HostEvent::Status(
                    "Agentic bubble working...".to_string(),
                ));
                return;
            }
            if status == "layout" {
                if let Some(layout) = extract_string_from_payload(&payload, "layout") {
                    session.last_valid_layout = Some(layout);
                    if let Some(pending) = ctx.sessions.pending_instrument_preview.as_mut() {
                        pending.layout = session.last_valid_layout.clone();
                    }
                }
                return;
            }
            if status != "valid" {
                session.preview_generation = session.preview_generation.wrapping_add(1);
                session.visible_revision_valid = false;
                ctx.sessions.pending_instrument_preview = None;
                let diagnostic = extract_string_from_payload(&payload, "diagnostic")
                    .unwrap_or_else(|| "Patch writeback failed".to_string());
                let rt = editor.runtime_mut();
                rt.set_reactive("SEQ", "editor-error", Value::String(diagnostic));
                rt.run_reactive_cycle();
                editor.refresh_runtime_side_effects();
                return;
            }
            let Some(source) = extract_string_from_payload(&payload, "source") else {
                session.preview_generation = session.preview_generation.wrapping_add(1);
                session.visible_revision_valid = false;
                ctx.sessions.pending_instrument_preview = None;
                let rt = editor.runtime_mut();
                rt.set_reactive(
                    "SEQ",
                    "editor-error",
                    Value::String(
                        "Patch preview did not include emitted source".to_string(),
                    ),
                );
                rt.run_reactive_cycle();
                editor.refresh_runtime_side_effects();
                return;
            };

            let compile_source =
                extract_string_from_payload(&payload, "compile-source")
                    .unwrap_or_else(|| source.clone());
            let layout = extract_string_from_payload(&payload, "layout");
            session.preview_generation = session.preview_generation.wrapping_add(1);
            session.visible_revision_valid = false;
            let generation = session.preview_generation;
            let sample_rate = app.graph.sample_rate;
            let asset_base = session.path.parent().map(|parent| parent.to_path_buf());
            let (tx, rx) = std::sync::mpsc::channel();
            std::thread::spawn(move || {
                let result =
                    sequencer::lisp_host::compile_and_load_instrument_with_origin(
                        &compile_source,
                        sample_rate,
                        asset_base.as_deref(),
                        sequencer::lisp_host::DGenSourceOrigin::Draft,
                    );
                let _ = tx.send(result);
            });
            ctx.sessions.pending_instrument_preview = Some(PendingInstrumentPreview {
                generation,
                source,
                layout,
                receiver: rx,
            });
            let rt = editor.runtime_mut();
            rt.set_reactive(
                "SEQ",
                "editor-error",
                Value::String("Preview compiling...".to_string()),
            );
            rt.run_reactive_cycle();
            editor.refresh_runtime_side_effects();
        }

        "preview-effect-patch" => {
            let Some(session) = ctx.sessions.effect_edit_session.as_mut() else {
                editor.handle_host_event(HostEvent::Status(
                    "No effect edit session is active".to_string(),
                ));
                return;
            };
            let status = extract_string_from_payload(&payload, "status")
                .unwrap_or_else(|| "invalid".to_string());
            if status == "agentic-submit" {
                let Some(path) = extract_string_from_payload(&payload, "path") else {
                    editor.handle_host_event(HostEvent::Status(
                        "Agentic bubble request missing patch path".to_string(),
                    ));
                    return;
                };
                let Some(bubble_id) =
                    extract_string_from_payload(&payload, "bubble-id")
                else {
                    editor.handle_host_event(HostEvent::Status(
                        "Agentic bubble request missing bubble id".to_string(),
                    ));
                    return;
                };
                let generation = extract_usize_from_payload(&payload, "generation")
                    .unwrap_or(0) as u64;
                let prompt =
                    extract_string_from_payload(&payload, "prompt").unwrap_or_default();
                let macro_name = extract_string_from_payload(&payload, "macro-name")
                    .unwrap_or_else(|| "agentic-macro".to_string());
                let target = extract_string_from_payload(&payload, "target")
                    .unwrap_or_else(|| "create-macro".to_string());
                let task_key = format!("{path}::{bubble_id}");
                let (tx, rx) = std::sync::mpsc::channel();
                let follow_up = if target == "edit-macro" {
                    let existing_macro_name =
                        extract_string_from_payload(&payload, "existing-macro-name")
                            .unwrap_or_else(|| macro_name.clone());
                    let params =
                        extract_string_from_payload(&payload, "existing-macro-params")
                            .unwrap_or_default()
                            .split_whitespace()
                            .map(str::to_string)
                            .collect::<Vec<_>>();
                    let source =
                        extract_string_from_payload(&payload, "existing-macro-source")
                            .unwrap_or_default();
                    Some(sequencer::agent::agentic_bubble::AgenticBubbleFollowUp {
                        macro_name: existing_macro_name,
                        params,
                        source,
                    })
                } else {
                    None
                };
                let request = sequencer::agent::agentic_bubble::AgenticBubbleRequest {
                    prompt,
                    suggested_macro_name: macro_name,
                    follow_up,
                };
                std::thread::spawn(move || {
                    let result =
                        sequencer::agent::agentic_bubble::generate_agentic_bubble_macro(
                            request,
                        );
                    let _ = tx.send(result);
                });
                ctx.sessions.pending_agentic_bubbles.insert(
                    task_key,
                    PendingAgenticBubble {
                        path: PathBuf::from(path),
                        intent: eseqlisp::widget_render::patcher::PatcherIntent::Effect,
                        bubble_id,
                        generation,
                        receiver: rx,
                    },
                );
                editor.handle_host_event(HostEvent::Status(
                    "Agentic bubble working...".to_string(),
                ));
                return;
            }
            if status == "layout" {
                if let Some(layout) = extract_string_from_payload(&payload, "layout") {
                    session.last_valid_layout = Some(layout);
                    if let Some(pending) = ctx.sessions.pending_effect_preview.as_mut() {
                        pending.layout = session.last_valid_layout.clone();
                    }
                }
                return;
            }
            if status != "valid" {
                session.preview_generation = session.preview_generation.wrapping_add(1);
                session.visible_revision_valid = false;
                ctx.sessions.pending_effect_preview = None;
                let diagnostic = extract_string_from_payload(&payload, "diagnostic")
                    .unwrap_or_else(|| "Patch writeback failed".to_string());
                let rt = editor.runtime_mut();
                rt.set_reactive("SEQ", "editor-error", Value::String(diagnostic));
                rt.run_reactive_cycle();
                editor.refresh_runtime_side_effects();
                return;
            }
            let Some(source) = extract_string_from_payload(&payload, "source") else {
                session.preview_generation = session.preview_generation.wrapping_add(1);
                session.visible_revision_valid = false;
                ctx.sessions.pending_effect_preview = None;
                let rt = editor.runtime_mut();
                rt.set_reactive(
                    "SEQ",
                    "editor-error",
                    Value::String(
                        "Patch preview did not include emitted source".to_string(),
                    ),
                );
                rt.run_reactive_cycle();
                editor.refresh_runtime_side_effects();
                return;
            };

            let compile_source =
                extract_string_from_payload(&payload, "compile-source")
                    .unwrap_or_else(|| source.clone());
            let layout = extract_string_from_payload(&payload, "layout");
            session.preview_generation = session.preview_generation.wrapping_add(1);
            session.visible_revision_valid = false;
            let generation = session.preview_generation;
            let sample_rate = app.graph.sample_rate;
            let asset_base = session.path.parent().map(|parent| parent.to_path_buf());
            let (tx, rx) = std::sync::mpsc::channel();
            std::thread::spawn(move || {
                let result = sequencer::lisp_host::compile_and_load_with_origin(
                    &compile_source,
                    sample_rate,
                    asset_base.as_deref(),
                    sequencer::lisp_host::DGenSourceOrigin::Draft,
                );
                let _ = tx.send(result);
            });
            ctx.sessions.pending_effect_preview = Some(PendingEffectPreview {
                generation,
                source,
                layout,
                receiver: rx,
            });
            let rt = editor.runtime_mut();
            rt.set_reactive(
                "SEQ",
                "editor-error",
                Value::String("Preview compiling...".to_string()),
            );
            rt.run_reactive_cycle();
            editor.refresh_runtime_side_effects();
        }

        "evaluate-editor-source" => {
            // Explicit compile + hot-swap of a code-editor buffer (C-c C-c /
            // Eval button). Reuses the pending-preview pipeline; the compile
            // path materializes defmacro-library imports itself.
            if let Some(session) = ctx.sessions.instrument_edit_session.as_mut() {
                if session.surface != EditorSurface::Code {
                    return;
                }
                let Some(source) = editor.read_buffer_text(&session.buffer_name)
                else {
                    editor.handle_host_event(HostEvent::Error(format!(
                        "Code buffer '{}' is missing",
                        session.buffer_name
                    )));
                    return;
                };
                session.preview_generation =
                    session.preview_generation.wrapping_add(1);
                session.visible_revision_valid = false;
                let generation = session.preview_generation;
                let sample_rate = app.graph.sample_rate;
                let asset_base =
                    session.path.parent().map(|parent| parent.to_path_buf());
                let compile_source = source.clone();
                let (tx, rx) = std::sync::mpsc::channel();
                std::thread::spawn(move || {
                    let result =
                        sequencer::lisp_host::compile_and_load_instrument_with_origin(
                            &compile_source,
                            sample_rate,
                            asset_base.as_deref(),
                            sequencer::lisp_host::DGenSourceOrigin::Draft,
                        );
                    let _ = tx.send(result);
                });
                ctx.sessions.pending_instrument_preview =
                    Some(PendingInstrumentPreview {
                        generation,
                        source,
                        layout: None,
                        receiver: rx,
                    });
                let rt = editor.runtime_mut();
                rt.set_reactive(
                    "SEQ",
                    "editor-error",
                    Value::String("Preview compiling...".to_string()),
                );
                rt.run_reactive_cycle();
                editor.refresh_runtime_side_effects();
            } else if let Some(session) = ctx.sessions.effect_edit_session.as_mut() {
                if session.surface != EditorSurface::Code {
                    return;
                }
                let Some(source) = editor.read_buffer_text(&session.buffer_name)
                else {
                    editor.handle_host_event(HostEvent::Error(format!(
                        "Code buffer '{}' is missing",
                        session.buffer_name
                    )));
                    return;
                };
                session.preview_generation =
                    session.preview_generation.wrapping_add(1);
                session.visible_revision_valid = false;
                let generation = session.preview_generation;
                let sample_rate = app.graph.sample_rate;
                let asset_base =
                    session.path.parent().map(|parent| parent.to_path_buf());
                let compile_source = source.clone();
                let (tx, rx) = std::sync::mpsc::channel();
                std::thread::spawn(move || {
                    let result = sequencer::lisp_host::compile_and_load_with_origin(
                        &compile_source,
                        sample_rate,
                        asset_base.as_deref(),
                        sequencer::lisp_host::DGenSourceOrigin::Draft,
                    );
                    let _ = tx.send(result);
                });
                ctx.sessions.pending_effect_preview = Some(PendingEffectPreview {
                    generation,
                    source,
                    layout: None,
                    receiver: rx,
                });
                let rt = editor.runtime_mut();
                rt.set_reactive(
                    "SEQ",
                    "editor-error",
                    Value::String("Preview compiling...".to_string()),
                );
                rt.run_reactive_cycle();
                editor.refresh_runtime_side_effects();
            }
        }

        "promote-editor-to-patch" => {
            // §3.3 "Open as patch": parse-check the CURRENT buffer text; on a
            // clean projection, persist it, stamp the authored sidecar, and
            // reopen the item in the patch editor.
            if let Some(session) = ctx.sessions.instrument_edit_session.as_mut() {
                if session.surface != EditorSurface::Code {
                    return;
                }
                let Some(source) = editor.read_buffer_text(&session.buffer_name) else {
                    editor.handle_host_event(HostEvent::Error(format!(
                        "Code buffer '{}' is missing",
                        session.buffer_name
                    )));
                    return;
                };
                if let Err(error) = std::fs::write(&session.path, &source) {
                    editor.handle_host_event(HostEvent::Error(format!(
                        "Failed to save before promotion: {error}"
                    )));
                    return;
                }
                if let Err(error) =
                    eseqlisp::widget_render::patcher::promote_source_to_patch(
                        &session.path,
                        &source,
                        eseqlisp::widget_render::patcher::PatcherIntent::Instrument,
                    )
                {
                    let rt = editor.runtime_mut();
                    rt.set_reactive("SEQ", "editor-error", Value::String(error));
                    rt.run_reactive_cycle();
                    editor.refresh_runtime_side_effects();
                    return;
                }
                let old_buf = session.buffer_name.clone();
                let buf_name = format!("*instrument-patcher:{}*", session.name);
                editor.remove_buffer_by_name(&buf_name);
                editor.create_scratch_buffer(&buf_name, "", BufferMode::ESeqLisp);
                let patcher_source =
                    instrument_patcher_buffer_source(&buf_name, &session.path);
                if let Err(error) = editor.runtime_mut().eval_str(&patcher_source) {
                    editor.handle_host_event(HostEvent::Error(format!(
                        "Failed to build patch editor: {error:?}"
                    )));
                    editor.remove_buffer_by_name(&buf_name);
                    return;
                }
                reset_instrument_patcher_state(&session.path);
                let layout_source = show_instrument_patcher_layout_source(&buf_name);
                if let Err(error) = editor.runtime_mut().eval_str(&layout_source) {
                    editor.handle_host_event(HostEvent::Error(format!(
                        "Failed to show patch editor: {error:?}"
                    )));
                    editor.remove_buffer_by_name(&buf_name);
                    return;
                }
                editor.refresh_runtime_side_effects();
                editor.remove_buffer_by_name(&old_buf);
                session.buffer_name = buf_name.clone();
                session.surface = EditorSurface::Patch;
                session.last_valid_source = source;
                session.visible_revision_valid = true;
                ctx.sessions.editor_buffer_name = Some(buf_name.clone());
                let rt = editor.runtime_mut();
                rt.set_reactive("SEQ", "editor-buffer-name", Value::String(buf_name));
                rt.set_reactive(
                    "SEQ",
                    "editor-surface",
                    Value::String("patch".to_string()),
                );
                rt.set_reactive("SEQ", "editor-error", Value::String(String::new()));
                rt.run_reactive_cycle();
                editor.refresh_runtime_side_effects();
                editor.handle_host_event(HostEvent::Status(
                    "Opened as patch".to_string(),
                ));
            } else if let Some(session) = ctx.sessions.effect_edit_session.as_mut() {
                if session.surface != EditorSurface::Code {
                    return;
                }
                let Some(source) = editor.read_buffer_text(&session.buffer_name) else {
                    editor.handle_host_event(HostEvent::Error(format!(
                        "Code buffer '{}' is missing",
                        session.buffer_name
                    )));
                    return;
                };
                if let Err(error) = std::fs::write(&session.path, &source) {
                    editor.handle_host_event(HostEvent::Error(format!(
                        "Failed to save before promotion: {error}"
                    )));
                    return;
                }
                if let Err(error) =
                    eseqlisp::widget_render::patcher::promote_source_to_patch(
                        &session.path,
                        &source,
                        eseqlisp::widget_render::patcher::PatcherIntent::Effect,
                    )
                {
                    let rt = editor.runtime_mut();
                    rt.set_reactive("SEQ", "editor-error", Value::String(error));
                    rt.run_reactive_cycle();
                    editor.refresh_runtime_side_effects();
                    return;
                }
                let old_buf = session.buffer_name.clone();
                let buf_name = format!("*effect-patcher:{}*", session.name);
                editor.remove_buffer_by_name(&buf_name);
                editor.create_scratch_buffer(&buf_name, "", BufferMode::ESeqLisp);
                let patcher_source =
                    effect_patcher_buffer_source(&buf_name, &session.path);
                if let Err(error) = editor.runtime_mut().eval_str(&patcher_source) {
                    editor.handle_host_event(HostEvent::Error(format!(
                        "Failed to build patch editor: {error:?}"
                    )));
                    editor.remove_buffer_by_name(&buf_name);
                    return;
                }
                reset_effect_patcher_state(&session.path);
                let layout_source = show_instrument_patcher_layout_source(&buf_name);
                if let Err(error) = editor.runtime_mut().eval_str(&layout_source) {
                    editor.handle_host_event(HostEvent::Error(format!(
                        "Failed to show patch editor: {error:?}"
                    )));
                    editor.remove_buffer_by_name(&buf_name);
                    return;
                }
                editor.refresh_runtime_side_effects();
                editor.remove_buffer_by_name(&old_buf);
                session.buffer_name = buf_name.clone();
                session.surface = EditorSurface::Patch;
                session.last_valid_source = source;
                session.visible_revision_valid = true;
                ctx.sessions.editor_buffer_name = Some(buf_name.clone());
                let rt = editor.runtime_mut();
                rt.set_reactive("SEQ", "editor-buffer-name", Value::String(buf_name));
                rt.set_reactive(
                    "SEQ",
                    "editor-surface",
                    Value::String("patch".to_string()),
                );
                rt.set_reactive("SEQ", "editor-error", Value::String(String::new()));
                rt.run_reactive_cycle();
                editor.refresh_runtime_side_effects();
                editor.handle_host_event(HostEvent::Status(
                    "Opened as patch".to_string(),
                ));
            }
        }

        "eject-editor-to-code" => {
            // §3.4 "Eject to code": persist the canonical generated source,
            // flip the sidecar's authored flag (keeping layout data for
            // re-promotion), and reopen in the code editor. Edit-existing
            // sessions only — drafts must be finalized first.
            if let Some(session) = ctx.sessions.instrument_edit_session.as_mut() {
                if session.surface != EditorSurface::Patch {
                    return;
                }
                if !matches!(session.mode, InstrumentEditMode::EditExisting { .. }) {
                    editor.handle_host_event(HostEvent::Status(
                        "Finalize the draft before ejecting to code".to_string(),
                    ));
                    return;
                }
                let source = session.last_valid_source.clone();
                if let Err(error) = std::fs::write(&session.path, &source) {
                    editor.handle_host_event(HostEvent::Error(format!(
                        "Failed to save before eject: {error}"
                    )));
                    return;
                }
                if let Some(layout) = session.last_valid_layout.as_deref() {
                    if let Err(error) =
                        write_patcher_layout_sidecar(&session.path, layout)
                    {
                        editor.handle_host_event(HostEvent::Error(format!(
                            "Failed to save layout before eject: {error}"
                        )));
                        return;
                    }
                }
                if let Err(error) =
                    eseqlisp::widget_render::patcher::eject_patch_authored_sidecar(
                        &session.path,
                    )
                {
                    editor.handle_host_event(HostEvent::Error(format!(
                        "Failed to eject to code: {error}"
                    )));
                    return;
                }
                let old_buf = session.buffer_name.clone();
                let buf_name = instrument_code_buffer_name(&session.name);
                editor.remove_buffer_by_name(&buf_name);
                editor.create_scratch_buffer(&buf_name, &source, BufferMode::DGenLisp);
                let layout_source = show_instrument_patcher_layout_source(&buf_name);
                if let Err(error) = editor.runtime_mut().eval_str(&layout_source) {
                    editor.handle_host_event(HostEvent::Error(format!(
                        "Failed to show code editor: {error:?}"
                    )));
                    editor.remove_buffer_by_name(&buf_name);
                    return;
                }
                editor.refresh_runtime_side_effects();
                editor.remove_buffer_by_name(&old_buf);
                session.buffer_name = buf_name.clone();
                session.surface = EditorSurface::Code;
                ctx.sessions.editor_buffer_name = Some(buf_name.clone());
                let rt = editor.runtime_mut();
                rt.set_reactive("SEQ", "editor-buffer-name", Value::String(buf_name));
                rt.set_reactive(
                    "SEQ",
                    "editor-surface",
                    Value::String("code".to_string()),
                );
                rt.set_reactive("SEQ", "editor-error", Value::String(String::new()));
                rt.run_reactive_cycle();
                editor.refresh_runtime_side_effects();
                editor.handle_host_event(HostEvent::Status(
                    "Ejected to code".to_string(),
                ));
            } else if let Some(session) = ctx.sessions.effect_edit_session.as_mut() {
                if session.surface != EditorSurface::Patch {
                    return;
                }
                if !matches!(session.mode, EffectEditMode::EditExisting { .. }) {
                    editor.handle_host_event(HostEvent::Status(
                        "Finalize the draft before ejecting to code".to_string(),
                    ));
                    return;
                }
                let source = session.last_valid_source.clone();
                if let Err(error) = std::fs::write(&session.path, &source) {
                    editor.handle_host_event(HostEvent::Error(format!(
                        "Failed to save before eject: {error}"
                    )));
                    return;
                }
                if let Some(layout) = session.last_valid_layout.as_deref() {
                    if let Err(error) =
                        write_patcher_layout_sidecar(&session.path, layout)
                    {
                        editor.handle_host_event(HostEvent::Error(format!(
                            "Failed to save layout before eject: {error}"
                        )));
                        return;
                    }
                }
                if let Err(error) =
                    eseqlisp::widget_render::patcher::eject_patch_authored_sidecar(
                        &session.path,
                    )
                {
                    editor.handle_host_event(HostEvent::Error(format!(
                        "Failed to eject to code: {error}"
                    )));
                    return;
                }
                let old_buf = session.buffer_name.clone();
                let buf_name = effect_code_buffer_name(&session.name);
                editor.remove_buffer_by_name(&buf_name);
                editor.create_scratch_buffer(&buf_name, &source, BufferMode::DGenLisp);
                let layout_source = show_instrument_patcher_layout_source(&buf_name);
                if let Err(error) = editor.runtime_mut().eval_str(&layout_source) {
                    editor.handle_host_event(HostEvent::Error(format!(
                        "Failed to show code editor: {error:?}"
                    )));
                    editor.remove_buffer_by_name(&buf_name);
                    return;
                }
                editor.refresh_runtime_side_effects();
                editor.remove_buffer_by_name(&old_buf);
                session.buffer_name = buf_name.clone();
                session.surface = EditorSurface::Code;
                ctx.sessions.editor_buffer_name = Some(buf_name.clone());
                let rt = editor.runtime_mut();
                rt.set_reactive("SEQ", "editor-buffer-name", Value::String(buf_name));
                rt.set_reactive(
                    "SEQ",
                    "editor-surface",
                    Value::String("code".to_string()),
                );
                rt.set_reactive("SEQ", "editor-error", Value::String(String::new()));
                rt.run_reactive_cycle();
                editor.refresh_runtime_side_effects();
                editor.handle_host_event(HostEvent::Status(
                    "Ejected to code".to_string(),
                ));
            }
        }

        "toggle-instrument-patcher-source" => {
            let (buffer_name, path, last_valid_source) =
                if let Some(session) = ctx.sessions.instrument_edit_session.as_ref() {
                    if session.surface == EditorSurface::Code {
                        return;
                    }
                    (
                        session.buffer_name.clone(),
                        session.path.clone(),
                        session.last_valid_source.clone(),
                    )
                } else if let Some(session) = ctx.sessions.effect_edit_session.as_ref() {
                    if session.surface == EditorSurface::Code {
                        return;
                    }
                    (
                        session.buffer_name.clone(),
                        session.path.clone(),
                        session.last_valid_source.clone(),
                    )
                } else {
                    editor.handle_host_event(HostEvent::Status(
                        "No patch edit session is active".to_string(),
                    ));
                    return;
                };
            if !path.exists() {
                editor.handle_host_event(HostEvent::Status(format!(
                    "Patch source no longer exists: {}",
                    path.display()
                )));
                return;
            }
            let source_buffer_name =
                eseqlisp::widget_render::patcher::emitted_source_buffer_name(
                    &path.to_string_lossy(),
                );
            let layout_source =
                if editor_has_visible_buffer(&editor, &source_buffer_name) {
                    show_instrument_patcher_layout_source(&buffer_name)
                } else {
                    let source_buffer_name = match editor
                        .upsert_patcher_emitted_source_buffer(
                            &buffer_name,
                            &path,
                            &last_valid_source,
                        ) {
                        Ok(name) => name,
                        Err(error) => {
                            editor.handle_host_event(HostEvent::Error(error));
                            return;
                        }
                    };
                    show_instrument_patcher_source_layout_source(
                        &buffer_name,
                        &source_buffer_name,
                    )
                };
            match editor.runtime_mut().eval_str(&layout_source) {
                Ok(_) => editor.refresh_runtime_side_effects(),
                Err(error) => editor.handle_host_event(HostEvent::Error(format!(
                    "Failed to show patch source layout: {error:?}"
                ))),
            }
        }

        "enter-new-effect-editor" => {
            if ctx.sessions.editor_mode.is_some()
                || ctx.sessions.instrument_edit_session.is_some()
                || ctx.sessions.effect_edit_session.is_some()
            {
                editor.handle_host_event(HostEvent::Error(format!(
                    "Close the current editor before creating a new effect"
                )));
                return;
            }
            if app.tracks.is_empty() {
                editor.handle_host_event(HostEvent::Error(
                    "No current track is available for a new effect".to_string(),
                ));
                return;
            }
            let track = current_track.load(Ordering::Relaxed);
            app.ui.cursor_track = track;
            let Some(slot) = app.next_free_custom_slot() else {
                editor.handle_host_event(HostEvent::Error(
                    "No free effect slots available".to_string(),
                ));
                return;
            };
            let temp_dir = match create_new_effect_draft_dir() {
                Ok(dir) => dir,
                Err(error) => {
                    editor.handle_host_event(HostEvent::Error(error));
                    return;
                }
            };
            let file_path = temp_dir.join("dsp.lisp");
            if let Err(error) =
                std::fs::write(&file_path, sequencer::lisp_host::EFFECT_TEMPLATE)
            {
                let _ = std::fs::remove_dir_all(&temp_dir);
                editor.handle_host_event(HostEvent::Error(format!(
                    "Failed to write starter effect: {error}"
                )));
                return;
            }
            match sequencer::lisp_host::compile_and_load_with_origin(
                sequencer::lisp_host::EFFECT_TEMPLATE,
                app.graph.sample_rate,
                file_path.parent(),
                sequencer::lisp_host::DGenSourceOrigin::Draft,
            )
            .and_then(|result| {
                app.apply_compiled_effect_to_slot_sync(
                    result,
                    NEW_EFFECT_DRAFT_NAME,
                    slot,
                    track,
                )
            }) {
                Ok(()) => {}
                Err(error) => {
                    let _ = std::fs::remove_dir_all(&temp_dir);
                    editor.handle_host_event(HostEvent::Error(format!(
                        "Failed to create draft effect: {error}"
                    )));
                    return;
                }
            }

            let buf_name = "*effect-patcher:new-effect*".to_string();
            editor.remove_buffer_by_name(&buf_name);
            editor.create_scratch_buffer(&buf_name, "", BufferMode::ESeqLisp);
            let patcher_source = effect_patcher_buffer_source(&buf_name, &file_path);
            if let Err(error) = editor.runtime_mut().eval_str(&patcher_source) {
                let _ = app
                    .graph_controller()
                    .delete_custom_effect_slot(track, slot);
                let _ = std::fs::remove_dir_all(&temp_dir);
                editor.handle_host_event(HostEvent::Error(format!(
                    "Failed to build patch editor: {error:?}"
                )));
                editor.remove_buffer_by_name(&buf_name);
                return;
            }
            reset_effect_patcher_state(&file_path);
            let layout_source = show_instrument_patcher_layout_source(&buf_name);
            if let Err(error) = editor.runtime_mut().eval_str(&layout_source) {
                let _ = app
                    .graph_controller()
                    .delete_custom_effect_slot(track, slot);
                let _ = std::fs::remove_dir_all(&temp_dir);
                editor.handle_host_event(HostEvent::Error(format!(
                    "Failed to show patch editor: {error:?}"
                )));
                editor.remove_buffer_by_name(&buf_name);
                return;
            }
            ctx.sessions.editor_buffer_name = Some(buf_name.clone());
            ctx.sessions.editor_mode = Some("new-effect".to_string());
            ctx.sessions.effect_edit_session = Some(EffectEditSession::begin_create_draft(
                NEW_EFFECT_DRAFT_NAME.to_string(),
                file_path,
                buf_name.clone(),
                EffectEditTarget::Track { track, slot },
                sequencer::lisp_host::EFFECT_TEMPLATE.to_string(),
                temp_dir,
            ));
            let rt = editor.runtime_mut();
            let _ = rt.eval_str("(set! sbrowser-editor-name \"\")");
            rt.set_reactive("SEQ", "editor-active", Value::Bool(true));
            rt.set_reactive(
                "SEQ",
                "editor-mode",
                Value::String("new-effect".to_string()),
            );
            rt.set_reactive("SEQ", "editor-error", Value::String(String::new()));
            rt.set_reactive(
                "SEQ",
                "editor-buffer-name",
                Value::String(buf_name.clone()),
            );
            rt.set_reactive(
                "SEQ",
                "editor-surface",
                Value::String("patch".to_string()),
            );
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
            rt.run_reactive_cycle();
            if let Err(error) = rt.eval_str("(sbrowser-refresh-buffer)") {
                let _ = app
                    .graph_controller()
                    .delete_custom_effect_slot(track, slot);
                if let Some(EffectEditSession {
                    mode: EffectEditMode::CreateDraft { temp_dir },
                    ..
                }) = ctx.sessions.effect_edit_session.take()
                {
                    let _ = std::fs::remove_dir_all(temp_dir);
                }
                editor.remove_buffer_by_name(&buf_name);
                ctx.sessions.editor_buffer_name = None;
                ctx.sessions.editor_mode = None;
                let rt = editor.runtime_mut();
                rt.set_reactive("SEQ", "editor-active", Value::Bool(false));
                rt.set_reactive("SEQ", "editor-mode", Value::String(String::new()));
                rt.set_reactive("SEQ", "editor-error", Value::String(String::new()));
                rt.set_reactive(
                    "SEQ",
                    "editor-buffer-name",
                    Value::String(String::new()),
                );
                rt.run_reactive_cycle();
                editor.refresh_runtime_side_effects();
                editor.handle_host_event(HostEvent::Error(format!(
                    "Failed to refresh effect editor sidebar: {error:?}"
                )));
                return;
            }
            editor.refresh_runtime_side_effects();
            editor.refresh_visible_layouts_for_buffer_named("*samples*");
            editor.handle_host_event(HostEvent::Status(format!(
                "Created draft effect in slot {}",
                slot + 1
            )));
        }

        "save-new-effect" => {
            if let Value::Map(ref map) = payload {
                if let Some(cell) = map.get("name") {
                    if let Value::String(effect_name) = &*cell.borrow() {
                        let effect_name = effect_name.trim().to_string();
                        if effect_name.is_empty() {
                            let rt = editor.runtime_mut();
                            rt.set_reactive(
                                "SEQ",
                                "editor-error",
                                Value::String("Name cannot be empty".to_string()),
                            );
                            rt.run_reactive_cycle();
                            editor.refresh_runtime_side_effects();
                            return;
                        }
                        let Some(session) = ctx.sessions.effect_edit_session.as_ref() else {
                            let rt = editor.runtime_mut();
                            rt.set_reactive(
                                "SEQ",
                                "editor-error",
                                Value::String(
                                    "No draft effect session is active".to_string(),
                                ),
                            );
                            rt.run_reactive_cycle();
                            editor.refresh_runtime_side_effects();
                            return;
                        };
                        if !matches!(&session.mode, EffectEditMode::CreateDraft { .. })
                        {
                            let rt = editor.runtime_mut();
                            rt.set_reactive(
                                "SEQ",
                                "editor-error",
                                Value::String(
                                    "Current editor session is not a draft effect"
                                        .to_string(),
                                ),
                            );
                            rt.run_reactive_cycle();
                            editor.refresh_runtime_side_effects();
                            return;
                        }
                        if !session.visible_revision_valid {
                            let rt = editor.runtime_mut();
                            rt.set_reactive(
                                "SEQ",
                                "editor-error",
                                Value::String(
                                    "Cannot finalize: the current patch has errors"
                                        .to_string(),
                                ),
                            );
                            rt.run_reactive_cycle();
                            editor.refresh_runtime_side_effects();
                            return;
                        }

                        let flushed_macros =
                            match flush_staged_effect_library_macro_edits(session) {
                                Ok(macros) => macros,
                                Err(error) => {
                                    let rt = editor.runtime_mut();
                                    rt.set_reactive(
                                        "SEQ",
                                        "editor-error",
                                        Value::String(format!(
                                            "Failed to save library macro edits: {error}"
                                        )),
                                    );
                                    rt.run_reactive_cycle();
                                    editor.refresh_runtime_side_effects();
                                    return;
                                }
                            };

                        let final_slug =
                            sequencer::agent::actions::normalize_patch_name(
                                &effect_name,
                                "new-effect",
                            );
                        let final_name = format!("{final_slug}/");
                        let (final_dir, legacy_file) =
                            finalized_effect_storage_paths(&final_slug);
                        if final_dir.exists() || legacy_file.exists() {
                            let rt = editor.runtime_mut();
                            rt.set_reactive(
                                "SEQ",
                                "editor-error",
                                Value::String(format!(
                                    "Effect '{final_slug}' already exists"
                                )),
                            );
                            rt.run_reactive_cycle();
                            editor.refresh_runtime_side_effects();
                            return;
                        }

                        let source = session.last_valid_source.clone();
                        if let Err(e) =
                            sequencer::lisp_host::save_effect(&final_name, &source)
                        {
                            let rt = editor.runtime_mut();
                            rt.set_reactive(
                                "SEQ",
                                "editor-error",
                                Value::String(format!("Failed to save: {e}")),
                            );
                            rt.run_reactive_cycle();
                            editor.refresh_runtime_side_effects();
                            return;
                        }
                        let final_dsp =
                            sequencer::lisp_host::effect_source_path(&final_name);
                        if let Some(layout) = session.last_valid_layout.as_deref() {
                            if let Err(e) =
                                write_patcher_layout_sidecar(&final_dsp, layout)
                            {
                                let _ = std::fs::remove_dir_all(&final_dir);
                                let rt = editor.runtime_mut();
                                rt.set_reactive(
                                    "SEQ",
                                    "editor-error",
                                    Value::String(format!(
                                        "Failed to save layout: {e}"
                                    )),
                                );
                                rt.run_reactive_cycle();
                                editor.refresh_runtime_side_effects();
                                return;
                            }
                        } else if let EffectEditMode::CreateDraft { temp_dir } =
                            &session.mode
                        {
                            let source_dsp = temp_dir.join("dsp.lisp");
                            if let Err(e) =
                                copy_patcher_layout_sidecar(&source_dsp, &final_dsp)
                            {
                                let _ = std::fs::remove_dir_all(&final_dir);
                                let rt = editor.runtime_mut();
                                rt.set_reactive(
                                    "SEQ",
                                    "editor-error",
                                    Value::String(format!(
                                        "Failed to save layout: {e}"
                                    )),
                                );
                                rt.run_reactive_cycle();
                                editor.refresh_runtime_side_effects();
                                return;
                            }
                        }
                        let (track, slot) = match session.target {
                            EffectEditTarget::Track { track, slot } => (track, slot),
                            EffectEditTarget::Bus { .. } => {
                                let _ = std::fs::remove_dir_all(&final_dir);
                                editor.handle_host_event(HostEvent::Error(
                                    "Draft effects can only target track effect slots"
                                        .to_string(),
                                ));
                                return;
                            }
                        };
                        if let Err(error) =
                            app.load_saved_effect_to_slot_sync(track, slot, &final_name)
                        {
                            let _ = std::fs::remove_dir_all(&final_dir);
                            let rt = editor.runtime_mut();
                            rt.set_reactive(
                                "SEQ",
                                "editor-error",
                                Value::String(format!(
                                    "Failed to load finalized effect: {error}"
                                )),
                            );
                            rt.run_reactive_cycle();
                            editor.refresh_runtime_side_effects();
                            return;
                        }
                        let session =
                            ctx.sessions.effect_edit_session.take().expect("session exists");
                        if let EffectEditMode::CreateDraft { temp_dir } = session.mode {
                            let _ = std::fs::remove_dir_all(temp_dir);
                        }
                        reset_effect_patcher_state(&session.path);
                        if let Err(error) = editor
                            .runtime_mut()
                            .eval_str(restore_instrument_patcher_layout_source())
                        {
                            editor.handle_host_event(HostEvent::Error(format!(
                                "Failed to restore main editor layout: {error:?}"
                            )));
                            return;
                        }
                        editor.refresh_runtime_side_effects();
                        editor.remove_buffer_by_name(&session.buffer_name);
                        if let Some(status) =
                            staged_library_macro_flush_status(&flushed_macros)
                        {
                            editor.handle_host_event(HostEvent::Status(status));
                        }
                        ctx.sessions.editor_buffer_name = None;
                        ctx.sessions.editor_mode = None;

                        let rt = editor.runtime_mut();
                        rt.set_reactive("SEQ", "editor-active", Value::Bool(false));
                        rt.set_reactive(
                            "SEQ",
                            "editor-mode",
                            Value::String(String::new()),
                        );
                        rt.set_reactive(
                            "SEQ",
                            "editor-error",
                            Value::String(String::new()),
                        );
                        rt.set_reactive(
                            "SEQ",
                            "editor-buffer-name",
                            Value::String(String::new()),
                        );
                        rt.set_reactive(
                            "SEQ",
                            "available-builtin-effects",
                            build_available_builtin_effects(),
                        );
                        rt.set_reactive(
                            "SEQ",
                            "available-effects",
                            build_available_effects(),
                        );
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
                        rt.run_reactive_cycle();
                        editor.refresh_runtime_side_effects();
                        editor.refresh_visible_layouts_for_buffer_named("*fx*");
                        fx_epoch.fetch_add(1, Ordering::Relaxed);
                        ui_epoch.fetch_add(1, Ordering::Relaxed);
                        editor.handle_host_event(HostEvent::Status(format!(
                            "Finalized effect '{}' in slot {}",
                            display_instrument_name(&final_name),
                            slot + 1
                        )));
                    }
                }
            }
        }

        "enter-edit-effect" => {
            if let Value::Map(ref map) = payload {
                if let Some(cell) = map.get("name") {
                    if let Value::String(effect_name) = &*cell.borrow() {
                        let effect_name = effect_name.clone();
                        let slot_idx =
                            map.get("slot").and_then(|cell| match &*cell.borrow() {
                                Value::Number(n) => Some(*n as usize),
                                _ => None,
                            });
                        let bus_idx =
                            map.get("bus").and_then(|cell| match &*cell.borrow() {
                                Value::Number(n) => Some(*n as usize),
                                _ => None,
                            });
                        let file_path =
                            sequencer::lisp_host::effect_source_path(&effect_name);
                        if !file_path.exists() {
                            editor.handle_host_event(HostEvent::Error(format!(
                                "Effect file not found: {}",
                                file_path.display()
                            )));
                            return;
                        }
                        let target = match (bus_idx, slot_idx) {
                            (Some(bus), Some(slot)) => {
                                EffectEditTarget::Bus { bus, slot }
                            }
                            (None, Some(slot)) => EffectEditTarget::Track {
                                track: current_track.load(Ordering::Relaxed),
                                slot,
                            },
                            _ => {
                                editor.handle_host_event(HostEvent::Error(
                                    "Effect edit command did not include a target slot"
                                        .to_string(),
                                ));
                                return;
                            }
                        };
                        let persisted_source = match std::fs::read_to_string(&file_path)
                        {
                            Ok(source) => source,
                            Err(error) => {
                                editor.handle_host_event(HostEvent::Error(format!(
                                    "Failed to read '{}': {error}",
                                    file_path.display()
                                )));
                                return;
                            }
                        };
                        let surface = editor_surface_for_existing(
                            &file_path,
                            &persisted_source,
                            eseqlisp::widget_render::patcher::PatcherIntent::Effect,
                        );
                        let buf_name = match surface {
                            EditorSurface::Patch => {
                                let buf_name =
                                    format!("*effect-patcher:{effect_name}*");
                                editor.remove_buffer_by_name(&buf_name);
                                editor.create_scratch_buffer(
                                    &buf_name,
                                    "",
                                    BufferMode::ESeqLisp,
                                );
                                let patcher_source = effect_patcher_buffer_source(
                                    &buf_name, &file_path,
                                );
                                if let Err(error) =
                                    editor.runtime_mut().eval_str(&patcher_source)
                                {
                                    editor.handle_host_event(HostEvent::Error(
                                        format!(
                                            "Failed to build patch editor: {error:?}"
                                        ),
                                    ));
                                    editor.remove_buffer_by_name(&buf_name);
                                    return;
                                }
                                reset_effect_patcher_state(&file_path);
                                buf_name
                            }
                            EditorSurface::Code => {
                                let buf_name = effect_code_buffer_name(&effect_name);
                                editor.remove_buffer_by_name(&buf_name);
                                editor.create_scratch_buffer(
                                    &buf_name,
                                    &persisted_source,
                                    BufferMode::DGenLisp,
                                );
                                buf_name
                            }
                        };
                        let layout_source =
                            show_instrument_patcher_layout_source(&buf_name);
                        if let Err(error) =
                            editor.runtime_mut().eval_str(&layout_source)
                        {
                            editor.handle_host_event(HostEvent::Error(format!(
                                "Failed to show patch editor: {error:?}"
                            )));
                            editor.remove_buffer_by_name(&buf_name);
                            return;
                        }
                        ctx.sessions.editor_buffer_name = Some(buf_name.clone());
                        ctx.sessions.editor_mode = Some("edit-effect".to_string());
                        ctx.sessions.effect_edit_session =
                            Some(EffectEditSession::begin_edit_existing(
                                effect_name.clone(),
                                file_path,
                                buf_name.clone(),
                                target,
                                persisted_source,
                                surface,
                            ));
                        let rt = editor.runtime_mut();
                        rt.set_reactive("SEQ", "editor-active", Value::Bool(true));
                        rt.set_reactive(
                            "SEQ",
                            "editor-mode",
                            Value::String("edit-effect".to_string()),
                        );
                        rt.set_reactive(
                            "SEQ",
                            "editor-error",
                            Value::String(String::new()),
                        );
                        rt.set_reactive(
                            "SEQ",
                            "editor-buffer-name",
                            Value::String(buf_name),
                        );
                        rt.set_reactive(
                            "SEQ",
                            "editor-surface",
                            Value::String(
                                editor_surface_label(surface).to_string(),
                            ),
                        );
                        rt.run_reactive_cycle();
                        editor.refresh_runtime_side_effects();
                    }
                }
            }
        }

        "update-effect" => {
            let Some(session) = ctx.sessions.effect_edit_session.as_ref() else {
                editor.handle_host_event(HostEvent::Error(
                    "No effect being edited".to_string(),
                ));
                return;
            };
            let code_surface = session.surface == EditorSurface::Code;
            if !code_surface && !session.visible_revision_valid {
                let rt = editor.runtime_mut();
                rt.set_reactive(
                    "SEQ",
                    "editor-error",
                    Value::String(
                        "Cannot save: the current patch has errors".to_string(),
                    ),
                );
                rt.run_reactive_cycle();
                editor.refresh_runtime_side_effects();
                return;
            }
            let flushed_macros = if code_surface {
                Vec::new()
            } else {
                match flush_staged_effect_library_macro_edits(session) {
                    Ok(macros) => macros,
                    Err(error) => {
                        let rt = editor.runtime_mut();
                        rt.set_reactive(
                            "SEQ",
                            "editor-error",
                            Value::String(format!(
                                "Failed to save library macro edits: {error}"
                            )),
                        );
                        rt.run_reactive_cycle();
                        editor.refresh_runtime_side_effects();
                        return;
                    }
                }
            };
            // Code sessions save the buffer text verbatim so comments and
            // formatting survive; patch sessions save the compiled emission.
            let source_to_save = if code_surface {
                match editor.read_buffer_text(&session.buffer_name) {
                    Some(text) => text,
                    None => {
                        editor.handle_host_event(HostEvent::Error(format!(
                            "Code buffer '{}' is missing",
                            session.buffer_name
                        )));
                        return;
                    }
                }
            } else {
                session.last_valid_source.clone()
            };
            let unevaluated_changes =
                code_surface && source_to_save != session.last_valid_source;
            if let Err(e) = std::fs::write(&session.path, &source_to_save) {
                let rt = editor.runtime_mut();
                rt.set_reactive(
                    "SEQ",
                    "editor-error",
                    Value::String(format!("Failed to save: {e}")),
                );
                rt.run_reactive_cycle();
                editor.refresh_runtime_side_effects();
                return;
            }
            if let Some(layout) = session.last_valid_layout.as_deref() {
                if let Err(e) = write_patcher_layout_sidecar(&session.path, layout) {
                    let rt = editor.runtime_mut();
                    rt.set_reactive(
                        "SEQ",
                        "editor-error",
                        Value::String(format!("Failed to save layout: {e}")),
                    );
                    rt.run_reactive_cycle();
                    editor.refresh_runtime_side_effects();
                    return;
                }
            }
            let session = ctx.sessions.effect_edit_session.take().expect("session exists");
            reset_effect_patcher_state(&session.path);
            if let Err(error) = editor
                .runtime_mut()
                .eval_str(restore_instrument_patcher_layout_source())
            {
                editor.handle_host_event(HostEvent::Error(format!(
                    "Failed to restore main editor layout: {error:?}"
                )));
                ctx.sessions.effect_edit_session = Some(session);
                return;
            }
            editor.refresh_runtime_side_effects();
            editor.remove_buffer_by_name(&session.buffer_name);
            if let Some(status) = staged_library_macro_flush_status(&flushed_macros) {
                editor.handle_host_event(HostEvent::Status(status));
            }
            ctx.sessions.editor_buffer_name = None;
            ctx.sessions.editor_mode = None;

            let rt = editor.runtime_mut();
            rt.set_reactive("SEQ", "editor-active", Value::Bool(false));
            rt.set_reactive("SEQ", "editor-mode", Value::String(String::new()));
            rt.set_reactive("SEQ", "editor-error", Value::String(String::new()));
            rt.set_reactive("SEQ", "editor-buffer-name", Value::String(String::new()));
            match session.target {
                EffectEditTarget::Track { track, .. } => {
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
                EffectEditTarget::Bus { .. } => {
                    *bus_state.lock().unwrap() = app.buses.clone();
                    sync_bus_mixer_state(rt, &app);
                    rt.set_reactive(
                        "SEQ",
                        "bus-effects",
                        build_bus_effects_value_for_selection(
                            &app,
                            Some(&selected_steps),
                        ),
                    );
                }
            }
            rt.run_reactive_cycle();
            editor.refresh_runtime_side_effects();
            editor.refresh_visible_layouts_for_buffer_named("*fx*");
            fx_epoch.fetch_add(1, Ordering::Relaxed);
            ui_epoch.fetch_add(1, Ordering::Relaxed);
            editor.handle_host_event(HostEvent::Status(if unevaluated_changes {
                format!(
                    "Saved effect '{}' with unevaluated changes (not hot-swapped)",
                    session.name
                )
            } else {
                format!("Saved effect '{}'", session.name)
            }));
        }

        "cancel-editor" => {
            if ctx.sessions.pending_instrument_cancel_restore.is_some()
                || ctx.sessions.pending_effect_cancel_restore.is_some()
            {
                return;
            }
            let cancelled_patcher =
                ctx.sessions.instrument_edit_session.is_some() || ctx.sessions.effect_edit_session.is_some();
            if let Some(session) = ctx.sessions.instrument_edit_session.take() {
                ctx.sessions.pending_instrument_preview = None;
                reset_instrument_patcher_state(&session.path);
                match session.mode.clone() {
                    InstrumentEditMode::EditExisting { persisted_source } => {
                        let source = persisted_source.clone();
                        let sample_rate = app.graph.sample_rate;
                        let asset_base =
                            session.path.parent().map(|parent| parent.to_path_buf());
                        let (tx, rx) = std::sync::mpsc::channel();
                        std::thread::spawn(move || {
                            let result = sequencer::lisp_host::compile_and_load_instrument_with_asset_base(
                                &source,
                                sample_rate,
                                asset_base.as_deref(),
                            );
                            let _ = tx.send(result);
                        });
                        ctx.sessions.pending_instrument_cancel_restore =
                            Some(PendingInstrumentCancelRestore {
                                session,
                                persisted_source,
                                receiver: rx,
                            });
                        let rt = editor.runtime_mut();
                        rt.set_reactive("SEQ", "editor-canceling", Value::Bool(true));
                        rt.set_reactive(
                            "SEQ",
                            "editor-error",
                            Value::String(String::new()),
                        );
                        rt.run_reactive_cycle();
                        editor.refresh_runtime_side_effects();
                        editor.mark_needs_redraw();
                        return;
                    }
                    InstrumentEditMode::CreateDraft {
                        temp_dir,
                        draft_track,
                        original_track,
                    } => {
                        let delete_result = if app.tracks.len() > 1 {
                            app.graph_controller().delete_track(draft_track)
                        } else {
                            app.graph_controller().clear_track_in_place(draft_track)
                        };
                        match delete_result {
                            Ok(_) => {
                                let restored_track = if app.tracks.is_empty() {
                                    0
                                } else {
                                    original_track.min(app.tracks.len() - 1)
                                };
                                current_track.store(restored_track, Ordering::Relaxed);
                                app.ui.cursor_track = restored_track;
                                {
                                    let mut pan_ids = track_pan_ids.lock().unwrap();
                                    *pan_ids = app
                                        .graph
                                        .track_node_ids
                                        .iter()
                                        .map(|ids| ids.pan_id)
                                        .collect();
                                    push_solo_mutes(lg_raw, &state, &pan_ids);
                                }
                                *record_armed.lock().unwrap() =
                                    app.graph.record_armed.clone();
                                let rt = editor.runtime_mut();
                                sync_track_topology_state(
                                    rt,
                                    &app,
                                    &state,
                                    &mut *ctx.track_names,
                                    restored_track,
                                    &selected_steps,
                                    &piano_roll_selection,
                                    &accumulator_names,
                                    &record_armed,
                                    &ctx.meters.cached_track_peak_levels,
                                );
                                rt.clear_subtree_effects_for_named_target(
                                    "*sequencer*",
                                );
                                rt.run_reactive_cycle();
                                editor.refresh_runtime_side_effects();
                                refresh_visible_track_topology_layouts(&mut editor);
                                ui_epoch.fetch_add(1, Ordering::Relaxed);
                            }
                            Err(error) => {
                                editor.handle_host_event(HostEvent::Error(format!(
                                    "Failed to remove draft instrument track: {error}"
                                )));
                            }
                        }
                        let _ = std::fs::remove_dir_all(temp_dir);
                    }
                }
            }
            if let Some(session) = ctx.sessions.effect_edit_session.take() {
                ctx.sessions.pending_effect_preview = None;
                reset_effect_patcher_state(&session.path);
                match session.mode.clone() {
                    EffectEditMode::EditExisting { persisted_source } => {
                        let source = persisted_source.clone();
                        let sample_rate = app.graph.sample_rate;
                        let asset_base =
                            session.path.parent().map(|parent| parent.to_path_buf());
                        let (tx, rx) = std::sync::mpsc::channel();
                        std::thread::spawn(move || {
                            let result =
                                sequencer::lisp_host::compile_and_load_with_asset_base(
                                    &source,
                                    sample_rate,
                                    asset_base.as_deref(),
                                );
                            let _ = tx.send(result);
                        });
                        ctx.sessions.pending_effect_cancel_restore =
                            Some(PendingEffectCancelRestore {
                                session,
                                receiver: rx,
                            });
                        let rt = editor.runtime_mut();
                        rt.set_reactive("SEQ", "editor-canceling", Value::Bool(true));
                        rt.set_reactive(
                            "SEQ",
                            "editor-error",
                            Value::String(String::new()),
                        );
                        rt.run_reactive_cycle();
                        editor.refresh_runtime_side_effects();
                        editor.mark_needs_redraw();
                        return;
                    }
                    EffectEditMode::CreateDraft { temp_dir } => {
                        if let EffectEditTarget::Track { track, slot } = session.target
                        {
                            match app
                                .graph_controller()
                                .delete_custom_effect_slot(track, slot)
                            {
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
                                    rt.run_reactive_cycle();
                                    editor.refresh_runtime_side_effects();
                                    editor.refresh_visible_layouts_for_buffer_named(
                                        "*fx*",
                                    );
                                    fx_epoch.fetch_add(1, Ordering::Relaxed);
                                    ui_epoch.fetch_add(1, Ordering::Relaxed);
                                }
                                Err(error) => {
                                    editor.handle_host_event(HostEvent::Error(
                                        format!(
                                        "Failed to remove draft effect slot: {error}"
                                    ),
                                    ));
                                }
                            }
                        }
                        let _ = std::fs::remove_dir_all(temp_dir);
                    }
                }
            }
            if let Some(buf_name) = ctx.sessions.editor_buffer_name.take() {
                if cancelled_patcher {
                    if let Err(error) = editor
                        .runtime_mut()
                        .eval_str(restore_instrument_patcher_layout_source())
                    {
                        editor.handle_host_event(HostEvent::Error(format!(
                            "Failed to restore main editor layout: {error:?}"
                        )));
                    }
                    editor.refresh_runtime_side_effects();
                } else {
                    editor.swap_buffer_in_tile_showing(&buf_name, "*sequencer*");
                }
                editor.remove_buffer_by_name(&buf_name);
            }

            ctx.sessions.editor_mode = None;
            let rt = editor.runtime_mut();
            rt.set_reactive("SEQ", "editor-active", Value::Bool(false));
            rt.set_reactive("SEQ", "editor-canceling", Value::Bool(false));
            rt.set_reactive("SEQ", "editor-mode", Value::String(String::new()));
            rt.set_reactive("SEQ", "editor-error", Value::String(String::new()));
            rt.set_reactive("SEQ", "editor-buffer-name", Value::String(String::new()));
            rt.set_reactive(
                "SEQ",
                "editor-instrument-run-mode",
                Value::String("instrument".to_string()),
            );
            rt.run_reactive_cycle();
            editor.refresh_runtime_side_effects();
            editor.handle_host_event(HostEvent::Status("Editor cancelled".to_string()));
        }

        _ => {}
    }
}
