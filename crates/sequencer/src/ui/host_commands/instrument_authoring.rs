use crate::*;

pub(super) const COMMANDS: &[&str] = &[
    "open-current-instrument-ui-source",
    "enter-new-instrument-editor",
    "enter-fork-instrument-editor",
    "enter-fork-effect-editor",
    "fork-editor-session",
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
    "open-editor-macro-view",
    "enter-new-effect-editor",
    "save-new-effect",
    "enter-edit-effect",
    "update-effect",
    "cancel-editor",
    "editor-save",
    "editor-save-as",
    "editor-done",
    "editor-restore-release",
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
    let ui_epoch = ctx.shared.ui_epoch.clone();
    let fx_epoch = ctx.shared.fx_epoch.clone();
    let track_pan_ids = ctx.shared.track_pan_ids.clone();
    let track_collapsed = ctx.shared.track_collapsed.clone();
    let bus_state = ctx.shared.bus_state.clone();
    let record_armed = ctx.shared.record_armed.clone();
    let accumulator_names = ctx.shared.accumulator_names.clone();
    match name {
        "open-current-instrument-ui-source" => {
            let track = current_track.load(Ordering::Relaxed);
            if app.graph.track_instrument_types.get(track)
                != Some(&sequencer::sequencer::InstrumentType::Custom)
            {
                editor.handle_host_event(HostEvent::Error(
                    "The current track does not have a custom instrument UI source".to_string(),
                ));
                return;
            }
            let Some(instrument_name) =
                crate::state_values::current_custom_instrument_name(&app, track)
            else {
                editor.handle_host_event(HostEvent::Error(
                    "The current track's instrument could not be resolved".to_string(),
                ));
                return;
            };
            let ui_path = match sequencer::lisp_host::instrument_ui_path(&instrument_name) {
                Ok(path) if path.is_file() => path,
                Ok(path) => {
                    editor.handle_host_event(HostEvent::Error(format!(
                        "Instrument '{}' has no ui.lisp source at {}",
                        display_instrument_name(&instrument_name),
                        path.display()
                    )));
                    return;
                }
                Err(error) => {
                    editor.handle_host_event(HostEvent::Error(format!(
                        "Could not resolve the UI source for instrument '{}': {error}",
                        display_instrument_name(&instrument_name)
                    )));
                    return;
                }
            };
            match editor.open_eseqlisp_file_in_source_pane(&ui_path) {
                Ok(_) => editor.handle_host_event(HostEvent::Status(format!(
                    "Opened UI source for instrument '{}'",
                    display_instrument_name(&instrument_name)
                ))),
                Err(error) => editor.handle_host_event(HostEvent::Error(format!(
                    "Could not open instrument UI source '{}': {error:?}",
                    ui_path.display()
                ))),
            }
        }

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
                &accumulator_names,
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
                EditorSurface::Patch,
            ));
            let rt = editor.runtime_mut();
            let _ = rt.eval_str("(eseq.browser/clear-editor-name!)");
            present_editor_open(
                rt,
                "new-instrument",
                &buf_name,
                Some(CustomInstrumentRunMode::Instrument),
                EditorSurface::Patch,
            );
            rt.run_reactive_cycle();
            editor.refresh_runtime_side_effects();
            editor.handle_host_event(HostEvent::Status(format!(
                "Created draft instrument track {}",
                draft_track + 1
            )));
        }

        // Fork = the draft flow with a different seed. Everything downstream
        // (live preview, error gating, name collision refusal, finalize) is the
        // create flow, so this mirrors `enter-new-instrument-editor` and only
        // swaps step 2: copy an existing instrument into the draft dir instead
        // of writing the starter template. See docs/instrument-fork-spec.md.
        "enter-fork-instrument-editor" => {
            if ctx.sessions.editor_mode.is_some() || ctx.sessions.instrument_edit_session.is_some()
            {
                editor.handle_host_event(HostEvent::Error(
                    "Close the current editor before forking an instrument".to_string(),
                ));
                return;
            }
            let Some(source_name) = extract_string_from_payload(&payload, "source") else {
                editor.handle_host_event(HostEvent::Error(
                    "Fork request is missing a source instrument".to_string(),
                ));
                return;
            };
            let source_path = match sequencer::lisp_host::instrument_source_path(&source_name) {
                Ok(path) => path,
                Err(error) => {
                    editor.handle_host_event(HostEvent::Error(format!(
                        "Instrument '{source_name}' not found: {error}"
                    )));
                    return;
                }
            };
            let original_track = current_track.load(Ordering::Relaxed);
            let temp_dir = match create_new_instrument_draft_dir() {
                Ok(dir) => dir,
                Err(error) => {
                    editor.handle_host_event(HostEvent::Error(error));
                    return;
                }
            };
            if let Err(error) = sequencer::patch_fork::fork_patch_source(&source_path, &temp_dir) {
                let _ = std::fs::remove_dir_all(&temp_dir);
                editor.handle_host_event(HostEvent::Error(format!(
                    "Failed to fork '{source_name}': {error}"
                )));
                return;
            }
            let file_path = temp_dir.join("dsp.lisp");
            let starter_source = match std::fs::read_to_string(&file_path) {
                Ok(source) => source,
                Err(error) => {
                    let _ = std::fs::remove_dir_all(&temp_dir);
                    editor.handle_host_event(HostEvent::Error(format!(
                        "Failed to read forked instrument source: {error}"
                    )));
                    return;
                }
            };
            let run_mode = sequencer::lisp_host::load_instrument_run_mode(&source_name)
                .unwrap_or(CustomInstrumentRunMode::Instrument);

            let draft_track = match app.add_transient_instrument_track_sync(
                NEW_INSTRUMENT_DRAFT_NAME,
                &starter_source,
                Some(&temp_dir),
            ) {
                Ok(track) => track,
                Err(error) => {
                    let _ = std::fs::remove_dir_all(&temp_dir);
                    editor.handle_host_event(HostEvent::Error(format!(
                        "Failed to create forked instrument track: {error}"
                    )));
                    return;
                }
            };
            let _ = app.force_instrument_enabled(draft_track);
            if run_mode != CustomInstrumentRunMode::Instrument {
                if let Err(error) = app
                    .graph_controller()
                    .set_track_instrument_run_mode(draft_track, run_mode)
                {
                    editor.handle_host_event(HostEvent::Error(format!(
                        "Failed to apply forked instrument mode: {error}"
                    )));
                }
            }
            sync_after_instrument_track_apply(
                &mut app,
                &mut editor,
                &state,
                draft_track,
                &current_track,
                &mut *ctx.track_names,
                &track_pan_ids,
                &record_armed,
                &accumulator_names,
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
                    "Forked instrument track has no engine binding".to_string(),
                ));
                return;
            };

            // A fork inherits the source's surface: forking a hand-written
            // instrument with no authored sidecar (core/triton) must open the
            // code editor, not scramble it through the patcher.
            let surface = editor_surface_for_existing(
                &file_path,
                &starter_source,
                eseqlisp::widget_render::patcher::PatcherIntent::Instrument,
            );
            let buf_name = match surface {
                EditorSurface::Patch => {
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
                    buf_name
                }
                EditorSurface::Code => {
                    let buf_name = instrument_code_buffer_name(NEW_INSTRUMENT_DRAFT_NAME);
                    editor.remove_buffer_by_name(&buf_name);
                    editor.create_scratch_buffer(
                        &buf_name,
                        &starter_source,
                        BufferMode::DGenLisp,
                    );
                    buf_name
                }
            };
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
            let mut session = InstrumentEditSession::begin_create_draft(
                NEW_INSTRUMENT_DRAFT_NAME.to_string(),
                file_path,
                buf_name.clone(),
                engine_id,
                starter_source,
                temp_dir,
                draft_track,
                original_track,
                surface,
            );
            session.run_mode = run_mode;
            ctx.sessions.instrument_edit_session = Some(session);
            let rt = editor.runtime_mut();
            // The name field starts empty on purpose (spec 3.4): a prefilled
            // `<source>-2` is a default you Enter through, and naming the fork
            // is the moment you decide what it is.
            let _ = rt.eval_str("(eseq.browser/clear-editor-name!)");
            present_editor_open(rt, "new-instrument", &buf_name, Some(run_mode), surface);
            rt.run_reactive_cycle();
            editor.refresh_runtime_side_effects();
            editor.handle_host_event(HostEvent::Status(format!(
                "Forked '{}' — name it to finalize",
                display_instrument_name(&source_name)
            )));
        }

        "set-draft-instrument-run-mode" => {
            let requested = extract_string_from_payload(&payload, "run-mode")
                .unwrap_or_else(|| "instrument".to_string());
            let Some(run_mode) = instrument_run_mode_from_label(&requested) else {
                editor_error(editor, format!("Unknown instrument run mode '{requested}'"));
                return;
            };
            // Absolute (`editor.run-mode`'s setter): the current mode is a
            // no-op, with or without a draft session.
            let current = match ctx.sessions.instrument_edit_session.as_ref() {
                Some(session) => session.run_mode,
                None => crate::presented::presented(|p| {
                    instrument_run_mode_from_label(&p.editor.get().run_mode)
                })
                .unwrap_or(CustomInstrumentRunMode::Instrument),
            };
            if current == run_mode {
                return;
            }
            let Some(session) = ctx.sessions.instrument_edit_session.as_mut() else {
                editor.handle_host_event(HostEvent::Status(
                    "No instrument edit session is active".to_string(),
                ));
                return;
            };
            // An existing instrument switches live too; Save writes it to
            // instrument.json (docs/patcher-workspace-spec.md §3).
            match app
                .graph_controller()
                .set_track_instrument_run_mode(session.track, run_mode)
            {
                Ok(()) => {
                    session.run_mode = run_mode;
                    if let Some(engine_id) = app.graph.track_engine_ids[session.track] {
                        session.engine_id = engine_id;
                    }
                    let dirty = session.is_dirty();
                    let rt = editor.runtime_mut();
                    present_editor(rt, |e| {
                        e.run_mode = instrument_run_mode_label(run_mode).to_string();
                        e.error.clear();
                        e.dirty = dirty;
                    });
                    rt.run_reactive_cycle();
                    editor.refresh_runtime_side_effects();
                    editor.handle_host_event(HostEvent::Status(format!(
                        "Instrument mode: {}",
                        match run_mode {
                            CustomInstrumentRunMode::Instrument => "Instrument",
                            CustomInstrumentRunMode::FreePatch => "Free Patch",
                        }
                    )));
                }
                Err(error) => editor_error(editor, error),
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
                    present_editor(rt, |e| {
                        e.active_macro = editor_macro_action.0.clone();
                        e.active_macro_action = editor_macro_action.1.clone();
                        e.error.clear();
                    });
                    ctx.frame.prev_editor_macro_action = editor_macro_action;
                    rt.run_reactive_cycle();
                    editor.refresh_runtime_side_effects();
                    editor.refresh_visible_layouts_for_buffer_named("*samples*");
                    editor.handle_host_event(HostEvent::Status(action_status));
                }
                Ok(None) => {
                    editor_error(editor, "No active macro is selected");
                    editor.refresh_visible_layouts_for_buffer_named("*samples*");
                }
                Err(error) => {
                    editor_error(editor, error);
                    editor.refresh_visible_layouts_for_buffer_named("*samples*");
                }
            }
        }

        "save-new-instrument" => {
            if let Value::Map(ref map) = payload {
                // `stay`: the toolbar's Save (docs/patcher-workspace-spec.md
                // §4) writes the instrument and keeps editing it.
                let stay = extract_bool_from_payload(&payload, "stay");
                if let Some(cell) = map.get("name") {
                    if let Value::String(inst_name) = &*cell.borrow() {
                        let inst_name = inst_name.trim().to_string();
                        if inst_name.is_empty() {
                            editor_error(editor, "Name cannot be empty");
                            return;
                        }
                        let Some(session) = ctx.sessions.instrument_edit_session.as_ref() else {
                            editor_error(editor, "No draft instrument session is active");
                            return;
                        };
                        if !matches!(&session.mode, InstrumentEditMode::CreateDraft { .. }) {
                            editor_error(
                                editor,
                                "Current editor session is not a draft instrument",
                            );
                            return;
                        }
                        // A draft can sit on the code surface — `fork-editor-session`
                        // over a hand-written instrument, or a fork of one. Those
                        // sessions have no patcher revision and no staged macro
                        // edits, exactly like `update-instrument` treats them.
                        let code_surface = session.surface == EditorSurface::Code;
                        if !code_surface && !session.visible_revision_valid {
                            editor_error(editor, "Cannot finalize: the current patch has errors");
                            return;
                        }

                        let flushed_macros = if code_surface {
                            Vec::new()
                        } else {
                            match flush_staged_instrument_library_macro_edits(session) {
                                Ok(macros) => macros,
                                Err(error) => {
                                    editor_error(
                                        editor,
                                        format!("Failed to save library macro edits: {error}"),
                                    );
                                    return;
                                }
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
                            editor_error(
                                editor,
                                format!("Instrument '{final_slug}' already exists"),
                            );
                            return;
                        }

                        // Code sessions finalize the buffer text, not the last
                        // patcher emission — everything typed since the fork
                        // would otherwise be silently dropped. The source is
                        // compiled below by `replace_custom_instrument_track_sync`,
                        // so there is nothing "unevaluated" to warn about.
                        let source = if code_surface {
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
                        let (draft_track, draft_temp_dir) = match &session.mode {
                            InstrumentEditMode::CreateDraft {
                                draft_track,
                                temp_dir,
                                ..
                            } => (*draft_track, temp_dir.clone()),
                            InstrumentEditMode::EditExisting { .. } => unreachable!(),
                        };
                        if let Err(error) =
                            sequencer::lisp_host::save_instrument(&final_name, &source)
                        {
                            let _ = std::fs::remove_dir_all(&final_dir);
                            editor_error(
                                editor,
                                format!("Failed to save finalized instrument: {error}"),
                            );
                            return;
                        }
                        if let Err(error) =
                            sequencer::lisp_host::save_instrument_run_mode(
                                &final_name,
                                session.run_mode,
                            )
                        {
                            let _ = std::fs::remove_dir_all(&final_dir);
                            editor_error(
                                editor,
                                format!("Failed to save finalized instrument mode: {error}"),
                            );
                            return;
                        }
                        let target_dsp = final_dir.join("dsp.lisp");
                        if let Some(layout) = session.last_valid_layout.as_deref() {
                            if let Err(error) =
                                write_patcher_layout_sidecar(&target_dsp, layout)
                            {
                                let _ = std::fs::remove_dir_all(&final_dir);
                                editor_error(
                                    editor,
                                    format!("Failed to save finalized instrument layout: {error}"),
                                );
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
                                editor_error(
                                    editor,
                                    format!("Failed to save finalized instrument layout: {error}"),
                                );
                                return;
                            }
                        }
                        // Assets a fork carried (ui.lisp, waves/, instrument.json,
                        // the staged preset bank) live only in the draft dir;
                        // save_instrument writes dsp.lisp and nothing else. They
                        // must land before the compile below, which resolves
                        // `@file "waves/..."` relative to the instrument folder.
                        if let Err(error) = sequencer::patch_fork::materialize_forked_assets(
                            &draft_temp_dir,
                            &final_dir,
                            &final_name,
                        ) {
                            let _ = std::fs::remove_dir_all(&final_dir);
                            editor_error(
                                editor,
                                format!("Failed to save finalized instrument assets: {error}"),
                            );
                            return;
                        }
                        if let Err(error) = app.replace_custom_instrument_track_sync(
                            draft_track,
                            &final_name,
                            &source,
                        ) {
                            // The forked preset bank lives *inside* final_dir
                            // (that is where the loader resolves "<slug>/"), so
                            // removing the directory takes it with it.
                            let _ = std::fs::remove_dir_all(&final_dir);
                            editor_error(
                                editor,
                                format!("Failed to load finalized instrument: {error}"),
                            );
                            return;
                        }
                        if let Err(error) =
                            app.graph_controller().set_track_instrument_run_mode(
                                draft_track,
                                session.run_mode,
                            )
                        {
                            editor_error(
                                editor,
                                format!("Failed to apply finalized instrument mode: {error}"),
                            );
                            return;
                        }

                        if stay {
                            // Carry on as an edit of the saved instrument. The
                            // patcher stays on the draft dir (`path`), so Save
                            // writes to the instrument's home (`save_path`) and
                            // the draft dir goes when the editor closes.
                            let session = ctx
                                .sessions
                                .instrument_edit_session
                                .as_mut()
                                .expect("session checked");
                            session.mode = InstrumentEditMode::EditExisting {
                                persisted_source: source.clone(),
                            };
                            session.last_valid_source = source.clone();
                            session.name = final_name.clone();
                            session.save_path = Some(target_dsp.clone());
                            session.saved_draft_dir = Some(draft_temp_dir.clone());
                            session.persisted_run_mode = session.run_mode;
                            session.fork_restore = None;
                            // The new instrument's files are this session's
                            // until a project save could pin them (§5.2).
                            session.working_release = Some(0);
                            session.project_saved_since_release = false;
                            session.track = draft_track;
                            if let Some(engine_id) =
                                app.graph.track_engine_ids.get(draft_track).copied().flatten()
                            {
                                session.engine_id = engine_id;
                            }
                            ctx.sessions.editor_mode = Some("edit-instrument".to_string());
                            current_track.store(draft_track, Ordering::Relaxed);
                            app.ui.cursor_track = draft_track;
                            *ctx.track_names = app.tracks.clone();
                            sync_shared_track_collapsed(&track_collapsed, &app);
                            if let Some(status) =
                                staged_library_macro_flush_status(&flushed_macros)
                            {
                                editor.handle_host_event(HostEvent::Status(status));
                            }
                            let rt = editor.runtime_mut();
                            present_editor(rt, |e| {
                                "edit-instrument".clone_into(&mut e.mode);
                                e.error.clear();
                            });
                            present_editor_saved(
                                rt,
                                &display_instrument_name(&final_name),
                                editor_release_of(&final_name),
                                false,
                            );
                            present_editor_releases(rt, instrument_release_listing(app, &final_name));
                            crate::param_words::set_track_word_names(&app.tracks);
                            sync_sidebar_browser(&app, draft_track);
                            rt.run_reactive_cycle();
                            editor.refresh_runtime_side_effects();
                            editor.refresh_visible_layouts_for_buffer_named("*fx*");
                            ui_epoch.fetch_add(1, Ordering::Relaxed);
                            // The toolbar's Save reports through a toast (no
                            // confirm modal; docs/patcher-workspace-spec.md §4).
                            editor.show_toast(
                                format!("Saved {}", display_instrument_name(&final_name)),
                                eseqlisp::ToastKind::Success,
                            );
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
                        present_editor_closed(rt);
                        crate::param_words::set_track_word_names(&app.tracks);
                        sync_sidebar_browser(&app, draft_track);
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
                            &accumulator_names,
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
                                inst_name.clone(),
                                file_path,
                                buf_name.clone(),
                                engine_id,
                                track,
                                persisted_source,
                                run_mode,
                                surface,
                            ));
                        let rt = editor.runtime_mut();
                        present_editor_open(
                            rt,
                            "edit-instrument",
                            &buf_name,
                            Some(run_mode),
                            surface,
                        );
                        present_editor_saved(
                            rt,
                            &display_instrument_name(&inst_name),
                            editor_release_of(&inst_name),
                            false,
                        );
                        present_editor_releases(rt, instrument_release_listing(app, &inst_name));
                        rt.run_reactive_cycle();
                        editor.refresh_runtime_side_effects();
                        refresh_visible_track_topology_layouts(&mut editor);
                    }
                }
            }
        }

        "update-instrument" => {
            if let Value::Map(ref map) = payload {
                let stay = extract_bool_from_payload(&payload, "stay");
                if let Some(cell) = map.get("name") {
                    if let Value::String(inst_name) = &*cell.borrow() {
                        let inst_name = inst_name.clone();
                        if let Some(session) = ctx.sessions.instrument_edit_session.as_ref() {
                            let code_surface =
                                session.surface == EditorSurface::Code;
                            if !code_surface && !session.visible_revision_valid {
                                editor_error(editor, "Cannot save: the current patch has errors");
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
                                        editor_error(
                                            editor,
                                            format!("Failed to save library macro edits: {error}"),
                                        );
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
                            let target = match prepare_instrument_save_target(session) {
                                Ok(target) => target,
                                Err(error) => {
                                    editor_error(editor, format!("Failed to save: {error}"));
                                    return;
                                }
                            };
                            if let Err(e) = std::fs::write(
                                &target.source_path,
                                &source_to_save,
                            ) {
                                editor_error(editor, format!("Failed to save: {e}"));
                                return;
                            }
                            if session.run_mode != session.persisted_run_mode {
                                if let Err(error) =
                                    sequencer::lisp_host::save_instrument_run_mode(
                                        target.pinned_name.as_deref().unwrap_or(&inst_name),
                                        session.run_mode,
                                    )
                                {
                                    editor_error(
                                        editor,
                                        format!("Failed to save instrument mode: {error}"),
                                    );
                                    return;
                                }
                            }
                            // Only the patch surface owns the sidecar: every
                            // layout the patcher emits carries `authored: true`,
                            // so writing one from a code session would silently
                            // re-promote a hand-edited dsp.lisp.
                            if let Some(layout) = session
                                .last_valid_layout
                                .as_deref()
                                .filter(|_| !code_surface)
                            {
                                if let Err(e) =
                                    write_patcher_layout_sidecar(&target.source_path, layout)
                                {
                                    editor_error(editor, format!("Failed to save layout: {e}"));
                                    return;
                                }
                            }
                            if code_surface {
                                if let Err(e) = sync_sidecar_for_code_save(
                                    &target.source_path,
                                    &source_to_save,
                                    session.ejected_patch.as_ref(),
                                ) {
                                    editor_error(editor, format!("Failed to save layout: {e}"));
                                    return;
                                }
                            }
                            rebase_patcher_after_save(&session.path, &target.source_path);

                            if stay {
                                let session = ctx
                                    .sessions
                                    .instrument_edit_session
                                    .as_mut()
                                    .expect("session checked");
                                session.mode = InstrumentEditMode::EditExisting {
                                    persisted_source: source_to_save.clone(),
                                };
                                session.persisted_run_mode = session.run_mode;
                                session.working_release =
                                    Some(target.release.unwrap_or(0));
                                session.project_saved_since_release = false;
                                if let Some(pinned) = target.pinned_name.as_ref() {
                                    // The track now plays (and a project save
                                    // pins) the release just cut.
                                    let old_name =
                                        std::mem::replace(&mut session.name, pinned.clone());
                                    session.save_path = Some(target.source_path.clone());
                                    bind_engine_to_release(
                                        &mut app,
                                        &mut *ctx.track_names,
                                        session.engine_id,
                                        &old_name,
                                        pinned,
                                    );
                                }
                                let dirty = session.is_dirty();
                                let title = display_instrument_name(&session.name);
                                let session_name = session.name.clone();
                                if let Some(status) =
                                    staged_library_macro_flush_status(&flushed_macros)
                                {
                                    editor.handle_host_event(HostEvent::Status(status));
                                }
                                let rt = editor.runtime_mut();
                                present_editor(rt, |e| e.error.clear());
                                present_editor_saved(
                                    rt,
                                    &title,
                                    target.release.unwrap_or(0),
                                    dirty,
                                );
                                present_editor_releases(
                                    rt,
                                    instrument_release_listing(app, &session_name),
                                );
                                rt.run_reactive_cycle();
                                editor.refresh_runtime_side_effects();
                                ui_epoch.fetch_add(1, Ordering::Relaxed);
                                editor.show_toast(
                                    save_toast_message(
                                        &title,
                                        target.release,
                                        target.pinned_name.is_some(),
                                        unevaluated_changes,
                                    ),
                                    eseqlisp::ToastKind::Success,
                                );
                                return;
                            }

                            if let Some(dir) = session.saved_draft_dir.as_ref() {
                                let _ = std::fs::remove_dir_all(dir);
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
                            let session = ctx
                                .sessions
                                .instrument_edit_session
                                .take()
                                .expect("session checked");

                            if let Some(pinned) = target.pinned_name.as_ref() {
                                // As on a staying Save: the track plays the
                                // release just cut, not the one it opened on.
                                bind_engine_to_release(
                                    &mut app,
                                    &mut *ctx.track_names,
                                    session.engine_id,
                                    &session.name,
                                    pinned,
                                );
                            } else {
                                let ct = current_track.load(Ordering::Relaxed);
                                ctx.track_names[ct] = inst_name.clone();
                                crate::param_words::set_track_word_names(ctx.track_names);
                            }
                            let inst_name = target.pinned_name.clone().unwrap_or(inst_name);
                            let rt = editor.runtime_mut();
                            present_editor_closed(rt);
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
                            editor_error(editor, format!("Failed to save: {e}"));
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
                                crate::param_words::set_track_word_names(ctx.track_names);
                                let rt = editor.runtime_mut();
                                present_editor_closed(rt);
                                rt.run_reactive_cycle();
                                editor.refresh_runtime_side_effects();
                                ui_epoch.fetch_add(1, Ordering::Relaxed);
                                editor.handle_host_event(HostEvent::Status(format!(
                                    "Hot-swapped instrument '{inst_name}'"
                                )));
                            }
                            Err(e) => {
                                // Compile failed — stay in editor, show error
                                editor_error(editor, format!("{e}"));
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
                // A connect bubble carries the patch context the patcher
                // assembled (docs/patcher-agentic-connect-spec.md §5); it never
                // asks for source.
                let connect = if target == "connect-node" {
                    Some(sequencer::agent::agentic_bubble::AgenticBubbleConnect {
                        subject_node_id: extract_string_from_payload(
                            &payload,
                            "target-node-id",
                        )
                        .unwrap_or_default(),
                        context: extract_string_from_payload(&payload, "connect-context")
                            .unwrap_or_default(),
                    })
                } else {
                    None
                };
                let request = sequencer::agent::agentic_bubble::AgenticBubbleRequest {
                    prompt,
                    history: agentic_bubble_history(&payload),
                    suggested_macro_name: macro_name.clone(),
                    follow_up,
                    connect,
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
            if status != "valid" && status != "changed" {
                session.preview_generation = session.preview_generation.wrapping_add(1);
                session.visible_revision_valid = false;
                ctx.sessions.pending_instrument_preview = None;
                let diagnostic = extract_string_from_payload(&payload, "diagnostic")
                    .unwrap_or_else(|| "Patch writeback failed".to_string());
                editor_error(editor, diagnostic);
                return;
            }
            let input = match preview_source_from_payload(editor, &payload, &session.path) {
                Ok(input) => input,
                Err(error) => {
                    session.preview_generation = session.preview_generation.wrapping_add(1);
                    session.visible_revision_valid = false;
                    ctx.sessions.pending_instrument_preview = None;
                    editor_error(editor, error);
                    return;
                }
            };
            session.preview_generation = session.preview_generation.wrapping_add(1);
            session.visible_revision_valid = false;
            ctx.sessions.pending_instrument_preview = Some(PendingInstrumentPreview {
                generation: session.preview_generation,
                layout: None,
                receiver: spawn_preview_compile(
                    input, app.graph.sample_rate, session.path.parent().map(Path::to_path_buf),
                    eseqlisp::widget_render::patcher::PatcherIntent::Instrument,
                ),
            });
            editor_error(editor, "Preview compiling...");
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
                // A connect bubble carries the patch context the patcher
                // assembled (docs/patcher-agentic-connect-spec.md §5); it never
                // asks for source.
                let connect = if target == "connect-node" {
                    Some(sequencer::agent::agentic_bubble::AgenticBubbleConnect {
                        subject_node_id: extract_string_from_payload(
                            &payload,
                            "target-node-id",
                        )
                        .unwrap_or_default(),
                        context: extract_string_from_payload(&payload, "connect-context")
                            .unwrap_or_default(),
                    })
                } else {
                    None
                };
                let request = sequencer::agent::agentic_bubble::AgenticBubbleRequest {
                    prompt,
                    history: agentic_bubble_history(&payload),
                    suggested_macro_name: macro_name,
                    follow_up,
                    connect,
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
            if status != "valid" && status != "changed" {
                session.preview_generation = session.preview_generation.wrapping_add(1);
                session.visible_revision_valid = false;
                ctx.sessions.pending_effect_preview = None;
                let diagnostic = extract_string_from_payload(&payload, "diagnostic")
                    .unwrap_or_else(|| "Patch writeback failed".to_string());
                editor_error(editor, diagnostic);
                return;
            }
            let input = match preview_source_from_payload(editor, &payload, &session.path) {
                Ok(input) => input,
                Err(error) => {
                    session.preview_generation = session.preview_generation.wrapping_add(1);
                    session.visible_revision_valid = false;
                    ctx.sessions.pending_effect_preview = None;
                    editor_error(editor, error);
                    return;
                }
            };
            session.preview_generation = session.preview_generation.wrapping_add(1);
            session.visible_revision_valid = false;
            ctx.sessions.pending_effect_preview = Some(PendingEffectPreview {
                generation: session.preview_generation,
                layout: None,
                receiver: spawn_preview_compile(
                    input, app.graph.sample_rate, session.path.parent().map(Path::to_path_buf),
                    eseqlisp::widget_render::patcher::PatcherIntent::Effect,
                ),
            });
            editor_error(editor, "Preview compiling...");
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
                queue_instrument_preview_compile(
                    session, &mut ctx.sessions.pending_instrument_preview, source, app.graph.sample_rate,
                );
                editor_error(editor, "Preview compiling...");
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
                queue_effect_preview_compile(
                    session, &mut ctx.sessions.pending_effect_preview, source, app.graph.sample_rate,
                );
                editor_error(editor, "Preview compiling...");
            }
        }

        "promote-editor-to-patch" => {
            // §3.3 "Open as patch": project the CURRENT buffer text (or, when
            // untouched since the switch to code, restore the patch it came
            // from) and reopen the patch editor on it, staged in memory. Disk
            // changes only on an explicit Save.
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
                // A view switch, not a save: the patch is staged in memory
                // and nothing reaches disk until an explicit Save.
                let (layout, unchanged) =
                    layout_for_promote(session.ejected_patch.as_ref(), &source, &session.path);
                reset_instrument_patcher_state(&session.path);
                let layout = match eseqlisp::widget_render::patcher::stage_promoted_patch(
                    &session.path,
                    &source,
                    layout.as_deref(),
                    unchanged,
                    eseqlisp::widget_render::patcher::PatcherIntent::Instrument,
                ) {
                    Ok(layout) => layout,
                    Err(error) => {
                        editor_error(editor, error);
                        return;
                    }
                };
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
                    reset_instrument_patcher_state(&session.path);
                    editor.remove_buffer_by_name(&buf_name);
                    return;
                }
                let layout_source = show_instrument_patcher_layout_source(&buf_name);
                if let Err(error) = editor.runtime_mut().eval_str(&layout_source) {
                    editor.handle_host_event(HostEvent::Error(format!(
                        "Failed to show patch editor: {error:?}"
                    )));
                    reset_instrument_patcher_state(&session.path);
                    editor.remove_buffer_by_name(&buf_name);
                    return;
                }
                editor.refresh_runtime_side_effects();
                editor.remove_buffer_by_name(&old_buf);
                session.buffer_name = buf_name.clone();
                session.surface = EditorSurface::Patch;
                session.last_valid_source = source;
                // Save writes this authored layout, promoting the item on disk.
                session.last_valid_layout = Some(layout);
                session.ejected_patch = None;
                session.visible_revision_valid = true;
                ctx.sessions.editor_buffer_name = Some(buf_name.clone());
                let rt = editor.runtime_mut();
                present_editor(rt, |e| {
                    e.buffer = buf_name;
                    e.surface = "patch".to_string();
                    e.error.clear();
                });
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
                // A view switch, not a save: the patch is staged in memory
                // and nothing reaches disk until an explicit Save.
                let (layout, unchanged) =
                    layout_for_promote(session.ejected_patch.as_ref(), &source, &session.path);
                reset_effect_patcher_state(&session.path);
                let layout = match eseqlisp::widget_render::patcher::stage_promoted_patch(
                    &session.path,
                    &source,
                    layout.as_deref(),
                    unchanged,
                    eseqlisp::widget_render::patcher::PatcherIntent::Effect,
                ) {
                    Ok(layout) => layout,
                    Err(error) => {
                        editor_error(editor, error);
                        return;
                    }
                };
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
                    reset_effect_patcher_state(&session.path);
                    editor.remove_buffer_by_name(&buf_name);
                    return;
                }
                let layout_source = show_instrument_patcher_layout_source(&buf_name);
                if let Err(error) = editor.runtime_mut().eval_str(&layout_source) {
                    editor.handle_host_event(HostEvent::Error(format!(
                        "Failed to show patch editor: {error:?}"
                    )));
                    reset_effect_patcher_state(&session.path);
                    editor.remove_buffer_by_name(&buf_name);
                    return;
                }
                editor.refresh_runtime_side_effects();
                editor.remove_buffer_by_name(&old_buf);
                session.buffer_name = buf_name.clone();
                session.surface = EditorSurface::Patch;
                session.last_valid_source = source;
                // Save writes this authored layout, promoting the item on disk.
                session.last_valid_layout = Some(layout);
                session.ejected_patch = None;
                session.visible_revision_valid = true;
                ctx.sessions.editor_buffer_name = Some(buf_name.clone());
                let rt = editor.runtime_mut();
                present_editor(rt, |e| {
                    e.buffer = buf_name;
                    e.surface = "patch".to_string();
                    e.error.clear();
                });
                rt.run_reactive_cycle();
                editor.refresh_runtime_side_effects();
                editor.handle_host_event(HostEvent::Status(
                    "Opened as patch".to_string(),
                ));
            }
        }

        "eject-editor-to-code" => {
            // §3.4 "Eject to code": reopen the canonical generated source in
            // the code editor. A view switch — nothing is written; a Save from
            // the code surface decides the sidecar (sync_sidecar_for_code_save). Edit-existing
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
                // A view switch, not a save: nothing is written. What the
                // patch was is kept so switching back restores it exactly.
                let source = session.last_valid_source.clone();
                let layout = layout_for_eject(
                    session.last_valid_layout.as_deref(),
                    session.visible_revision_valid,
                    &session.path,
                );
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
                // A code-surface Save decides the sidecar from `ejected_patch`
                // (sync_sidecar_for_code_save), never from this layout.
                session.last_valid_layout = None;
                session.ejected_patch = Some(EjectedPatch { source, layout });
                ctx.sessions.editor_buffer_name = Some(buf_name.clone());
                let rt = editor.runtime_mut();
                present_editor(rt, |e| {
                    e.buffer = buf_name;
                    e.surface = "code".to_string();
                    e.error.clear();
                });
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
                // A view switch, not a save: nothing is written. What the
                // patch was is kept so switching back restores it exactly.
                let source = session.last_valid_source.clone();
                let layout = layout_for_eject(
                    session.last_valid_layout.as_deref(),
                    session.visible_revision_valid,
                    &session.path,
                );
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
                // Same as the instrument arm.
                session.last_valid_layout = None;
                session.ejected_patch = Some(EjectedPatch { source, layout });
                ctx.sessions.editor_buffer_name = Some(buf_name.clone());
                let rt = editor.runtime_mut();
                present_editor(rt, |e| {
                    e.buffer = buf_name;
                    e.surface = "code".to_string();
                    e.error.clear();
                });
                rt.run_reactive_cycle();
                editor.refresh_runtime_side_effects();
                editor.handle_host_event(HostEvent::Status(
                    "Ejected to code".to_string(),
                ));
            }
        }

        // Macro sidebar navigation: open a macro's patcher view (empty or
        // missing name = back to the root view). Preserves staged edits.
        "open-editor-macro-view" => {
            let (path, buffer_name) =
                if let Some(session) = ctx.sessions.instrument_edit_session.as_ref() {
                    (session.path.clone(), session.buffer_name.clone())
                } else if let Some(session) = ctx.sessions.effect_edit_session.as_ref() {
                    (session.path.clone(), session.buffer_name.clone())
                } else {
                    editor.handle_host_event(HostEvent::Status(
                        "No patch edit session is active".to_string(),
                    ));
                    return;
                };
            let macro_name = match &payload {
                Value::Map(map) => map.get("name").and_then(|cell| match &*cell.borrow() {
                    Value::String(name) if !name.is_empty() => Some(name.clone()),
                    _ => None,
                }),
                _ => None,
            };
            eseqlisp::widget_render::patcher::navigate_patcher_view_for_path(
                &path,
                macro_name.as_deref(),
            );
            editor.refresh_visible_layouts_for_buffer_named(&buffer_name);
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
                )?;
                // Retain the compiled draft source directly: the draft name
                // has no effects-library file, so the recorded-edit machinery
                // must never fall back to a name-based disk read (eseq-u2h).
                app.retain_compiled_effect_source_for_track_slot(
                    track,
                    slot,
                    NEW_EFFECT_DRAFT_NAME,
                    sequencer::lisp_host::EFFECT_TEMPLATE,
                    file_path.parent().map(|parent| parent.to_path_buf()),
                    sequencer::lisp_host::DGenSourceOrigin::Draft,
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
                EditorSurface::Patch,
            ));
            let rt = editor.runtime_mut();
            let _ = rt.eval_str("(eseq.browser/clear-editor-name!)");
            present_editor_open(rt, "new-effect", &buf_name, None, EditorSurface::Patch);
            rt.run_reactive_cycle();
            if let Err(error) = rt.eval_str("(eseq.browser/refresh-buffer)") {
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
                present_editor_closed(rt);
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

        // Effect parity for `enter-fork-instrument-editor`: mirrors
        // `enter-new-effect-editor` with the draft dir seeded from effects/<name>.
        "enter-fork-effect-editor" => {
            if ctx.sessions.editor_mode.is_some()
                || ctx.sessions.instrument_edit_session.is_some()
                || ctx.sessions.effect_edit_session.is_some()
            {
                editor.handle_host_event(HostEvent::Error(
                    "Close the current editor before forking an effect".to_string(),
                ));
                return;
            }
            let Some(source_name) = extract_string_from_payload(&payload, "source") else {
                editor.handle_host_event(HostEvent::Error(
                    "Fork request is missing a source effect".to_string(),
                ));
                return;
            };
            if app.tracks.is_empty() {
                editor.handle_host_event(HostEvent::Error(
                    "No current track is available for a forked effect".to_string(),
                ));
                return;
            }
            let source_path = sequencer::lisp_host::effect_source_path(&source_name);
            if !source_path.exists() {
                editor.handle_host_event(HostEvent::Error(format!(
                    "Effect '{source_name}' not found at {}",
                    source_path.display()
                )));
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
            if let Err(error) = sequencer::patch_fork::fork_patch_source(&source_path, &temp_dir) {
                let _ = std::fs::remove_dir_all(&temp_dir);
                editor.handle_host_event(HostEvent::Error(format!(
                    "Failed to fork '{source_name}': {error}"
                )));
                return;
            }
            let file_path = temp_dir.join("dsp.lisp");
            let starter_source = match std::fs::read_to_string(&file_path) {
                Ok(source) => source,
                Err(error) => {
                    let _ = std::fs::remove_dir_all(&temp_dir);
                    editor.handle_host_event(HostEvent::Error(format!(
                        "Failed to read forked effect source: {error}"
                    )));
                    return;
                }
            };
            if let Err(error) = sequencer::lisp_host::compile_and_load_with_origin(
                &starter_source,
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
                )?;
                // Same as enter-new-effect-editor: retain the forked draft's
                // compiled source so no path ever resolves the draft name
                // against the effects library (eseq-u2h).
                app.retain_compiled_effect_source_for_track_slot(
                    track,
                    slot,
                    NEW_EFFECT_DRAFT_NAME,
                    &starter_source,
                    file_path.parent().map(|parent| parent.to_path_buf()),
                    sequencer::lisp_host::DGenSourceOrigin::Draft,
                )
            }) {
                let _ = std::fs::remove_dir_all(&temp_dir);
                editor.handle_host_event(HostEvent::Error(format!(
                    "Failed to create forked effect: {error}"
                )));
                return;
            }

            // Same as the instrument fork: inherit the source's surface rather
            // than assuming the patcher can represent it.
            let surface = editor_surface_for_existing(
                &file_path,
                &starter_source,
                eseqlisp::widget_render::patcher::PatcherIntent::Effect,
            );
            let buf_name = match surface {
                EditorSurface::Patch => {
                    let buf_name = "*effect-patcher:new-effect*".to_string();
                    editor.remove_buffer_by_name(&buf_name);
                    editor.create_scratch_buffer(&buf_name, "", BufferMode::ESeqLisp);
                    let patcher_source =
                        effect_patcher_buffer_source(&buf_name, &file_path);
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
                    buf_name
                }
                EditorSurface::Code => {
                    let buf_name = effect_code_buffer_name(NEW_EFFECT_DRAFT_NAME);
                    editor.remove_buffer_by_name(&buf_name);
                    editor.create_scratch_buffer(
                        &buf_name,
                        &starter_source,
                        BufferMode::DGenLisp,
                    );
                    buf_name
                }
            };
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
                starter_source,
                temp_dir,
                surface,
            ));
            let rt = editor.runtime_mut();
            let _ = rt.eval_str("(eseq.browser/clear-editor-name!)");
            present_editor_open(rt, "new-effect", &buf_name, None, surface);
            rt.run_reactive_cycle();
            let _ = rt.eval_str("(eseq.browser/refresh-buffer)");
            editor.refresh_runtime_side_effects();
            editor.refresh_visible_layouts_for_buffer_named("*samples*");
            editor.handle_host_event(HostEvent::Status(format!(
                "Forked effect '{}' — name it to finalize",
                display_instrument_name(&source_name)
            )));
        }

        "save-new-effect" => {
            let stay = extract_bool_from_payload(&payload, "stay");
            if let Value::Map(ref map) = payload {
                if let Some(cell) = map.get("name") {
                    if let Value::String(effect_name) = &*cell.borrow() {
                        let effect_name = effect_name.trim().to_string();
                        if effect_name.is_empty() {
                            editor_error(editor, "Name cannot be empty");
                            return;
                        }
                        let Some(session) = ctx.sessions.effect_edit_session.as_ref() else {
                            editor_error(editor, "No draft effect session is active");
                            return;
                        };
                        if !matches!(&session.mode, EffectEditMode::CreateDraft { .. })
                        {
                            editor_error(editor, "Current editor session is not a draft effect");
                            return;
                        }
                        // See `save-new-instrument`: draft effects can sit on
                        // the code surface too.
                        let code_surface = session.surface == EditorSurface::Code;
                        if !code_surface && !session.visible_revision_valid {
                            editor_error(editor, "Cannot finalize: the current patch has errors");
                            return;
                        }

                        let flushed_macros = if code_surface {
                            Vec::new()
                        } else {
                            match flush_staged_effect_library_macro_edits(session) {
                                Ok(macros) => macros,
                                Err(error) => {
                                    editor_error(
                                        editor,
                                        format!("Failed to save library macro edits: {error}"),
                                    );
                                    return;
                                }
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
                            editor_error(editor, format!("Effect '{final_slug}' already exists"));
                            return;
                        }

                        // Code sessions finalize the buffer text — see the
                        // instrument twin. `load_saved_effect_to_slot_sync`
                        // below compiles what we just wrote.
                        let source = if code_surface {
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
                        if let Err(e) =
                            sequencer::lisp_host::save_effect(&final_name, &source)
                        {
                            editor_error(editor, format!("Failed to save: {e}"));
                            return;
                        }
                        let final_dsp =
                            sequencer::lisp_host::effect_source_path(&final_name);
                        if let Some(layout) = session.last_valid_layout.as_deref() {
                            if let Err(e) =
                                write_patcher_layout_sidecar(&final_dsp, layout)
                            {
                                let _ = std::fs::remove_dir_all(&final_dir);
                                editor_error(editor, format!("Failed to save layout: {e}"));
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
                                editor_error(editor, format!("Failed to save layout: {e}"));
                                return;
                            }
                        }
                        if let EffectEditMode::CreateDraft { temp_dir } = &session.mode {
                            // Forked effects carry ui.lisp and asset dirs that
                            // save_effect does not write.
                            if let Err(error) = sequencer::patch_fork::materialize_forked_assets(
                                temp_dir, &final_dir, &final_name,
                            ) {
                                let _ = std::fs::remove_dir_all(&final_dir);
                                editor_error(
                                    editor,
                                    format!("Failed to save finalized effect assets: {error}"),
                                );
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
                            editor_error(
                                editor,
                                format!("Failed to load finalized effect: {error}"),
                            );
                            return;
                        }
                        if stay {
                            // Carry on editing the saved effect, as the
                            // instrument path does (`save-new-instrument`).
                            let session = ctx
                                .sessions
                                .effect_edit_session
                                .as_mut()
                                .expect("session exists");
                            if let EffectEditMode::CreateDraft { temp_dir } = &session.mode {
                                session.saved_draft_dir = Some(temp_dir.clone());
                            }
                            session.mode = EffectEditMode::EditExisting {
                                persisted_source: source.clone(),
                            };
                            session.last_valid_source = source.clone();
                            session.name = final_name.clone();
                            session.save_path = Some(final_dsp.clone());
                            session.fork_restore = None;
                            ctx.sessions.editor_mode = Some("edit-effect".to_string());
                            if let Some(status) =
                                staged_library_macro_flush_status(&flushed_macros)
                            {
                                editor.handle_host_event(HostEvent::Status(status));
                            }
                            let rt = editor.runtime_mut();
                            present_editor(rt, |e| {
                                "edit-effect".clone_into(&mut e.mode);
                                e.error.clear();
                            });
                            present_editor_saved(
                                rt,
                                &display_instrument_name(&final_name),
                                0,
                                false,
                            );
                            rt.run_reactive_cycle();
                            editor.refresh_runtime_side_effects();
                            editor.refresh_visible_layouts_for_buffer_named("*fx*");
                            fx_epoch.fetch_add(1, Ordering::Relaxed);
                            ui_epoch.fetch_add(1, Ordering::Relaxed);
                            editor.show_toast(
                                format!("Saved {}", display_instrument_name(&final_name)),
                                eseqlisp::ToastKind::Success,
                            );
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
                        present_editor_closed(rt);
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
                        present_editor_open(rt, "edit-effect", &buf_name, None, surface);
                        present_editor_saved(rt, &effect_name, 0, false);
                        rt.run_reactive_cycle();
                        editor.refresh_runtime_side_effects();
                    }
                }
            }
        }

        "update-effect" => {
            let stay = extract_bool_from_payload(&payload, "stay");
            let Some(session) = ctx.sessions.effect_edit_session.as_ref() else {
                editor.handle_host_event(HostEvent::Error(
                    "No effect being edited".to_string(),
                ));
                return;
            };
            let code_surface = session.surface == EditorSurface::Code;
            if !code_surface && !session.visible_revision_valid {
                editor_error(editor, "Cannot save: the current patch has errors");
                return;
            }
            let flushed_macros = if code_surface {
                Vec::new()
            } else {
                match flush_staged_effect_library_macro_edits(session) {
                    Ok(macros) => macros,
                    Err(error) => {
                        editor_error(
                            editor,
                            format!("Failed to save library macro edits: {error}"),
                        );
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
            if let Err(e) = std::fs::write(session.save_target(), &source_to_save) {
                editor_error(editor, format!("Failed to save: {e}"));
                return;
            }
            // Patch surface only — see the instrument twin in `update-instrument`.
            if let Some(layout) = session
                .last_valid_layout
                .as_deref()
                .filter(|_| !code_surface)
            {
                if let Err(e) = write_patcher_layout_sidecar(session.save_target(), layout) {
                    editor_error(editor, format!("Failed to save layout: {e}"));
                    return;
                }
            }
            if code_surface {
                if let Err(e) = sync_sidecar_for_code_save(
                    session.save_target(),
                    &source_to_save,
                    session.ejected_patch.as_ref(),
                ) {
                    editor_error(editor, format!("Failed to save layout: {e}"));
                    return;
                }
            }
            rebase_patcher_after_save(&session.path, session.save_target());
            if stay {
                let session = ctx.sessions.effect_edit_session.as_mut().expect("session exists");
                session.mode = EffectEditMode::EditExisting {
                    persisted_source: source_to_save.clone(),
                };
                let dirty = session.is_dirty();
                let title = display_instrument_name(&session.name);
                if let Some(status) = staged_library_macro_flush_status(&flushed_macros) {
                    editor.handle_host_event(HostEvent::Status(status));
                }
                let rt = editor.runtime_mut();
                present_editor(rt, |e| e.error.clear());
                present_editor_saved(rt, &title, 0, dirty);
                rt.run_reactive_cycle();
                editor.refresh_runtime_side_effects();
                ui_epoch.fetch_add(1, Ordering::Relaxed);
                editor.show_toast(
                    save_toast_message(&title, None, false, unevaluated_changes),
                    eseqlisp::ToastKind::Success,
                );
                return;
            }
            let session = ctx.sessions.effect_edit_session.take().expect("session exists");
            if let Some(dir) = session.saved_draft_dir.as_ref() {
                let _ = std::fs::remove_dir_all(dir);
            }
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
            present_editor_closed(rt);
            match session.target {
                EffectEditTarget::Track { .. } => {}
                EffectEditTarget::Bus { .. } => {
                    *bus_state.lock().unwrap() = app.buses.clone();
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

        // In-editor Fork (spec §3.5): you started editing in place and now want
        // out without discarding your work. Copies the *in-editor* source — not
        // the on-disk source — into a fresh draft dir, converts the session from
        // EditExisting to CreateDraft, and flips `editor.mode` so the create
        // flow's (empty) name field appears and the primary button becomes
        // Finalize. The original on disk is never written.
        "fork-editor-session" => {
            enum ForkKind {
                Instrument,
                Effect,
            }
            let kind = if let Some(session) = ctx.sessions.instrument_edit_session.as_ref() {
                if !matches!(session.mode, InstrumentEditMode::EditExisting { .. }) {
                    editor.handle_host_event(HostEvent::Status(
                        "This editor is already a draft — name it and Finalize".to_string(),
                    ));
                    return;
                }
                ForkKind::Instrument
            } else if let Some(session) = ctx.sessions.effect_edit_session.as_ref() {
                if !matches!(session.mode, EffectEditMode::EditExisting { .. }) {
                    editor.handle_host_event(HostEvent::Status(
                        "This editor is already a draft — name it and Finalize".to_string(),
                    ));
                    return;
                }
                ForkKind::Effect
            } else {
                editor.handle_host_event(HostEvent::Error(
                    "No patch editor session is active".to_string(),
                ));
                return;
            };

            match kind {
                ForkKind::Instrument => {
                    let session = ctx
                        .sessions
                        .instrument_edit_session
                        .as_ref()
                        .expect("checked above");
                    let origin_path = session.path.clone();
                    let origin_buffer = session.buffer_name.clone();
                    let surface = session.surface;
                    let layout = session.last_valid_layout.clone();
                    let persisted_source = match &session.mode {
                        InstrumentEditMode::EditExisting { persisted_source } => {
                            persisted_source.clone()
                        }
                        InstrumentEditMode::CreateDraft { .. } => unreachable!(),
                    };
                    let in_editor_source = if surface == EditorSurface::Code {
                        match editor.read_buffer_text(&origin_buffer) {
                            Some(text) => text,
                            None => {
                                editor.handle_host_event(HostEvent::Error(format!(
                                    "Code buffer '{origin_buffer}' is missing"
                                )));
                                return;
                            }
                        }
                    } else {
                        session.last_valid_source.clone()
                    };
                    let track = session.track;

                    let temp_dir = match create_new_instrument_draft_dir() {
                        Ok(dir) => dir,
                        Err(error) => {
                            editor.handle_host_event(HostEvent::Error(error));
                            return;
                        }
                    };
                    if let Err(error) =
                        sequencer::patch_fork::fork_patch_source(&origin_path, &temp_dir)
                    {
                        let _ = std::fs::remove_dir_all(&temp_dir);
                        editor.handle_host_event(HostEvent::Error(format!(
                            "Failed to fork editor session: {error}"
                        )));
                        return;
                    }
                    let file_path = temp_dir.join("dsp.lisp");
                    if let Err(error) = std::fs::write(&file_path, &in_editor_source) {
                        let _ = std::fs::remove_dir_all(&temp_dir);
                        editor.handle_host_event(HostEvent::Error(format!(
                            "Failed to seed forked draft: {error}"
                        )));
                        return;
                    }
                    if let Some(layout) = layout.as_deref() {
                        if let Err(error) = write_patcher_layout_sidecar(&file_path, layout) {
                            let _ = std::fs::remove_dir_all(&temp_dir);
                            editor.handle_host_event(HostEvent::Error(format!(
                                "Failed to seed forked layout: {error}"
                            )));
                            return;
                        }
                    }

                    // Rebind the editor surface onto the draft path so every
                    // subsequent patcher write lands in the temp dir, never on
                    // the original.
                    reset_instrument_patcher_state(&origin_path);
                    let buf_name = match surface {
                        EditorSurface::Patch => "*instrument-patcher:new-instrument*".to_string(),
                        EditorSurface::Code => {
                            instrument_code_buffer_name(NEW_INSTRUMENT_DRAFT_NAME)
                        }
                    };
                    editor.remove_buffer_by_name(&buf_name);
                    match surface {
                        EditorSurface::Patch => {
                            editor.create_scratch_buffer(&buf_name, "", BufferMode::ESeqLisp);
                            let patcher_source =
                                instrument_patcher_buffer_source(&buf_name, &file_path);
                            if let Err(error) = editor.runtime_mut().eval_str(&patcher_source) {
                                let _ = std::fs::remove_dir_all(&temp_dir);
                                editor.remove_buffer_by_name(&buf_name);
                                editor.handle_host_event(HostEvent::Error(format!(
                                    "Failed to rebuild patch editor for the fork: {error:?}"
                                )));
                                return;
                            }
                            reset_instrument_patcher_state(&file_path);
                        }
                        EditorSurface::Code => {
                            editor.create_scratch_buffer(
                                &buf_name,
                                &in_editor_source,
                                BufferMode::DGenLisp,
                            );
                        }
                    }
                    let layout_source = show_instrument_patcher_layout_source(&buf_name);
                    if let Err(error) = editor.runtime_mut().eval_str(&layout_source) {
                        let _ = std::fs::remove_dir_all(&temp_dir);
                        editor.remove_buffer_by_name(&buf_name);
                        editor.handle_host_event(HostEvent::Error(format!(
                            "Failed to show forked patch editor: {error:?}"
                        )));
                        return;
                    }
                    if origin_buffer != buf_name {
                        editor.remove_buffer_by_name(&origin_buffer);
                    }

                    let session = ctx
                        .sessions
                        .instrument_edit_session
                        .as_mut()
                        .expect("checked above");
                    session.path = file_path;
                    session.buffer_name = buf_name.clone();
                    session.last_valid_source = in_editor_source;
                    session.last_valid_layout = layout;
                    session.visible_revision_valid = true;
                    session.mode = InstrumentEditMode::CreateDraft {
                        temp_dir,
                        draft_track: track,
                        original_track: track,
                    };
                    session.fork_restore = Some(ForkRestore {
                        origin_path,
                        persisted_source,
                    });
                    let origin_display = display_instrument_name(&session.name);

                    ctx.sessions.editor_buffer_name = Some(buf_name.clone());
                    ctx.sessions.editor_mode = Some("new-instrument".to_string());
                    let rt = editor.runtime_mut();
                    // Empty on purpose — see spec §3.4.
                    let _ = rt.eval_str("(eseq.browser/clear-editor-name!)");
                    present_editor(rt, |e| {
                        e.mode = "new-instrument".to_string();
                        e.error.clear();
                        e.buffer = buf_name;
                        // A draft now: nothing saved under its new name yet.
                        e.title.clear();
                        e.release = 0;
                        e.dirty = true;
                        e.releases.clear();
                        e.release_tracks.clear();
                    });
                    rt.run_reactive_cycle();
                    editor.refresh_runtime_side_effects();
                    editor.handle_host_event(HostEvent::Status(format!(
                        "Forked '{origin_display}' — name it to save; the original is untouched"
                    )));
                }
                ForkKind::Effect => {
                    let session = ctx
                        .sessions
                        .effect_edit_session
                        .as_ref()
                        .expect("checked above");
                    if matches!(session.target, EffectEditTarget::Bus { .. }) {
                        editor.handle_host_event(HostEvent::Error(
                            "Bus effects cannot be forked yet — finalize only targets track slots"
                                .to_string(),
                        ));
                        return;
                    }
                    let origin_path = session.path.clone();
                    let origin_buffer = session.buffer_name.clone();
                    let surface = session.surface;
                    let layout = session.last_valid_layout.clone();
                    let persisted_source = match &session.mode {
                        EffectEditMode::EditExisting { persisted_source } => {
                            persisted_source.clone()
                        }
                        EffectEditMode::CreateDraft { .. } => unreachable!(),
                    };
                    let in_editor_source = if surface == EditorSurface::Code {
                        match editor.read_buffer_text(&origin_buffer) {
                            Some(text) => text,
                            None => {
                                editor.handle_host_event(HostEvent::Error(format!(
                                    "Code buffer '{origin_buffer}' is missing"
                                )));
                                return;
                            }
                        }
                    } else {
                        session.last_valid_source.clone()
                    };

                    let temp_dir = match create_new_effect_draft_dir() {
                        Ok(dir) => dir,
                        Err(error) => {
                            editor.handle_host_event(HostEvent::Error(error));
                            return;
                        }
                    };
                    if let Err(error) =
                        sequencer::patch_fork::fork_patch_source(&origin_path, &temp_dir)
                    {
                        let _ = std::fs::remove_dir_all(&temp_dir);
                        editor.handle_host_event(HostEvent::Error(format!(
                            "Failed to fork editor session: {error}"
                        )));
                        return;
                    }
                    let file_path = temp_dir.join("dsp.lisp");
                    if let Err(error) = std::fs::write(&file_path, &in_editor_source) {
                        let _ = std::fs::remove_dir_all(&temp_dir);
                        editor.handle_host_event(HostEvent::Error(format!(
                            "Failed to seed forked draft: {error}"
                        )));
                        return;
                    }
                    if let Some(layout) = layout.as_deref() {
                        if let Err(error) = write_patcher_layout_sidecar(&file_path, layout) {
                            let _ = std::fs::remove_dir_all(&temp_dir);
                            editor.handle_host_event(HostEvent::Error(format!(
                                "Failed to seed forked layout: {error}"
                            )));
                            return;
                        }
                    }

                    reset_effect_patcher_state(&origin_path);
                    let buf_name = match surface {
                        EditorSurface::Patch => "*effect-patcher:new-effect*".to_string(),
                        EditorSurface::Code => effect_code_buffer_name(NEW_EFFECT_DRAFT_NAME),
                    };
                    editor.remove_buffer_by_name(&buf_name);
                    match surface {
                        EditorSurface::Patch => {
                            editor.create_scratch_buffer(&buf_name, "", BufferMode::ESeqLisp);
                            let patcher_source =
                                effect_patcher_buffer_source(&buf_name, &file_path);
                            if let Err(error) = editor.runtime_mut().eval_str(&patcher_source) {
                                let _ = std::fs::remove_dir_all(&temp_dir);
                                editor.remove_buffer_by_name(&buf_name);
                                editor.handle_host_event(HostEvent::Error(format!(
                                    "Failed to rebuild patch editor for the fork: {error:?}"
                                )));
                                return;
                            }
                            reset_effect_patcher_state(&file_path);
                        }
                        EditorSurface::Code => {
                            editor.create_scratch_buffer(
                                &buf_name,
                                &in_editor_source,
                                BufferMode::DGenLisp,
                            );
                        }
                    }
                    let layout_source = show_instrument_patcher_layout_source(&buf_name);
                    if let Err(error) = editor.runtime_mut().eval_str(&layout_source) {
                        let _ = std::fs::remove_dir_all(&temp_dir);
                        editor.remove_buffer_by_name(&buf_name);
                        editor.handle_host_event(HostEvent::Error(format!(
                            "Failed to show forked patch editor: {error:?}"
                        )));
                        return;
                    }
                    if origin_buffer != buf_name {
                        editor.remove_buffer_by_name(&origin_buffer);
                    }

                    let session = ctx
                        .sessions
                        .effect_edit_session
                        .as_mut()
                        .expect("checked above");
                    session.path = file_path;
                    session.buffer_name = buf_name.clone();
                    session.last_valid_source = in_editor_source;
                    session.last_valid_layout = layout;
                    session.visible_revision_valid = true;
                    session.mode = EffectEditMode::CreateDraft { temp_dir };
                    session.fork_restore = Some(ForkRestore {
                        origin_path,
                        persisted_source,
                    });
                    let origin_display = display_instrument_name(&session.name);

                    ctx.sessions.editor_buffer_name = Some(buf_name.clone());
                    ctx.sessions.editor_mode = Some("new-effect".to_string());
                    let rt = editor.runtime_mut();
                    let _ = rt.eval_str("(eseq.browser/clear-editor-name!)");
                    present_editor(rt, |e| {
                        e.mode = "new-effect".to_string();
                        e.error.clear();
                        e.buffer = buf_name;
                        // A draft now: nothing saved under its new name yet.
                        e.title.clear();
                        e.release = 0;
                        e.dirty = true;
                        e.releases.clear();
                        e.release_tracks.clear();
                    });
                    rt.run_reactive_cycle();
                    editor.refresh_runtime_side_effects();
                    editor.handle_host_event(HostEvent::Status(format!(
                        "Forked '{origin_display}' — name it to save; the original is untouched"
                    )));
                }
            }
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
                                fork_draft_dir: None,
                                receiver: rx,
                            });
                        let rt = editor.runtime_mut();
                        present_editor(rt, |e| {
                            e.canceling = true;
                            e.error.clear();
                        });
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
                        // A draft produced by forking a live edit session sits on
                        // a REAL track. Deleting it would take the user's track
                        // with it; restore the original instrument instead.
                        if let Some(restore) = session.fork_restore.clone() {
                            let source = restore.persisted_source.clone();
                            let sample_rate = app.graph.sample_rate;
                            let asset_base = restore
                                .origin_path
                                .parent()
                                .map(|parent| parent.to_path_buf());
                            let (tx, rx) = std::sync::mpsc::channel();
                            std::thread::spawn(move || {
                                let result = sequencer::lisp_host::compile_and_load_instrument_with_asset_base(
                                    &source,
                                    sample_rate,
                                    asset_base.as_deref(),
                                );
                                let _ = tx.send(result);
                            });
                            // The session is left pointing at the draft: if the
                            // restore compile fails it goes back into the
                            // editor as-is, still a usable draft. The draft dir
                            // is removed only once the restore lands.
                            ctx.sessions.pending_instrument_cancel_restore =
                                Some(PendingInstrumentCancelRestore {
                                    session,
                                    persisted_source: restore.persisted_source,
                                    fork_draft_dir: Some(temp_dir),
                                    receiver: rx,
                                });
                            let rt = editor.runtime_mut();
                            present_editor(rt, |e| {
                                e.canceling = true;
                                e.error.clear();
                            });
                            rt.run_reactive_cycle();
                            editor.refresh_runtime_side_effects();
                            editor.mark_needs_redraw();
                            return;
                        }
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
                                    push_solo_mutes(lg_raw, app, &state);
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
                                    &accumulator_names,
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
                                fork_draft_dir: None,
                                receiver: rx,
                            });
                        let rt = editor.runtime_mut();
                        present_editor(rt, |e| {
                            e.canceling = true;
                            e.error.clear();
                        });
                        rt.run_reactive_cycle();
                        editor.refresh_runtime_side_effects();
                        editor.mark_needs_redraw();
                        return;
                    }
                    EffectEditMode::CreateDraft { temp_dir } => {
                        // Same asymmetry as instruments: a forked draft occupies
                        // the slot the original effect was in, so cancel restores
                        // the original rather than deleting the slot.
                        if let Some(restore) = session.fork_restore.clone() {
                            let source = restore.persisted_source.clone();
                            let sample_rate = app.graph.sample_rate;
                            let asset_base = restore
                                .origin_path
                                .parent()
                                .map(|parent| parent.to_path_buf());
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
                            // Same deal as instruments: delete the draft dir
                            // only once the restore compile lands.
                            ctx.sessions.pending_effect_cancel_restore =
                                Some(PendingEffectCancelRestore {
                                    session,
                                    fork_draft_dir: Some(temp_dir),
                                    receiver: rx,
                                });
                            let rt = editor.runtime_mut();
                            present_editor(rt, |e| {
                                e.canceling = true;
                                e.error.clear();
                            });
                            rt.run_reactive_cycle();
                            editor.refresh_runtime_side_effects();
                            editor.mark_needs_redraw();
                            return;
                        }
                        if let EffectEditTarget::Track { track, slot } = session.target
                        {
                            match app
                                .graph_controller()
                                .delete_custom_effect_slot(track, slot)
                            {
                                Ok(()) => {
                                    let rt = editor.runtime_mut();
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
            present_editor_closed(rt);
            rt.run_reactive_cycle();
            editor.refresh_runtime_side_effects();
            editor.handle_host_event(HostEvent::Status("Editor cancelled".to_string()));
        }

        // The patch toolbar's actions (docs/patcher-workspace-spec.md §4),
        // dispatched on the open session onto the per-mode commands above.
        // Save writes and keeps editing (`:done true`: then leave); Save as
        // forks an existing instrument / effect into a draft to name; Done
        // leaves, discarding only a draft that was never saved (the toolbar
        // asks first while there are unsaved changes).
        "editor-save" => {
            let done = extract_bool_from_payload(&payload, "done");
            let name = extract_string_from_payload(&payload, "name").unwrap_or_default();
            let (command, name) = if let Some(session) =
                ctx.sessions.instrument_edit_session.as_ref()
            {
                match &session.mode {
                    InstrumentEditMode::CreateDraft { .. } => ("save-new-instrument", name),
                    InstrumentEditMode::EditExisting { .. } => {
                        ("update-instrument", session.name.clone())
                    }
                }
            } else if let Some(session) = ctx.sessions.effect_edit_session.as_ref() {
                match &session.mode {
                    EffectEditMode::CreateDraft { .. } => ("save-new-effect", name),
                    EffectEditMode::EditExisting { .. } => ("update-effect", String::new()),
                }
            } else {
                return;
            };
            if command.starts_with("save-new") && name.trim().is_empty() {
                editor_error(editor, "Name it to save it");
                return;
            }
            let payload = payload_map(&[
                ("name", Value::String(name)),
                ("stay", Value::Bool(!done)),
            ]);
            handle(command, payload, app, editor, ctx);
        }

        "editor-save-as" => {
            let existing = ctx.sessions.instrument_edit_session.as_ref().is_some_and(|s| {
                matches!(s.mode, InstrumentEditMode::EditExisting { .. })
            }) || ctx.sessions.effect_edit_session.as_ref().is_some_and(|s| {
                matches!(s.mode, EffectEditMode::EditExisting { .. })
            });
            if existing {
                handle("fork-editor-session", Value::Nil, app, editor, ctx);
            }
        }

        "editor-done" => {
            // `force`: close over the current state (a restore just replaced it).
            let force = extract_bool_from_payload(&payload, "force");
            let clean_existing = ctx
                .sessions
                .instrument_edit_session
                .as_ref()
                .is_some_and(|s| {
                    matches!(s.mode, InstrumentEditMode::EditExisting { .. })
                        && (force || !s.is_dirty())
                });
            let clean_effect = ctx.sessions.effect_edit_session.as_ref().is_some_and(|s| {
                matches!(s.mode, EffectEditMode::EditExisting { .. }) && !s.is_dirty()
            });
            if clean_effect {
                let session = ctx.sessions.effect_edit_session.take().expect("session checked");
                if let Some(dir) = session.saved_draft_dir.as_ref() {
                    let _ = std::fs::remove_dir_all(dir);
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
                ctx.sessions.editor_buffer_name = None;
                ctx.sessions.editor_mode = None;
                ctx.sessions.pending_effect_preview = None;
                let rt = editor.runtime_mut();
                present_editor_closed(rt);
                rt.run_reactive_cycle();
                editor.refresh_runtime_side_effects();
                editor.refresh_visible_layouts_for_buffer_named("*fx*");
                fx_epoch.fetch_add(1, Ordering::Relaxed);
                ui_epoch.fetch_add(1, Ordering::Relaxed);
                return;
            }
            if !clean_existing {
                // A draft never saved, or unsaved changes the toolbar
                // already confirmed discarding: cancel restores the last
                // saved state.
                handle("cancel-editor", Value::Nil, app, editor, ctx);
                return;
            }
            let session = ctx
                .sessions
                .instrument_edit_session
                .take()
                .expect("session checked");
            if let Some(dir) = session.saved_draft_dir.as_ref() {
                let _ = std::fs::remove_dir_all(dir);
            }
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
            editor.remove_buffer_by_name(&session.buffer_name);
            ctx.sessions.editor_buffer_name = None;
            ctx.sessions.editor_mode = None;
            ctx.sessions.pending_instrument_preview = None;
            let rt = editor.runtime_mut();
            present_editor_closed(rt);
            rt.run_reactive_cycle();
            editor.refresh_runtime_side_effects();
            refresh_visible_track_topology_layouts(&mut editor);
            ui_epoch.fetch_add(1, Ordering::Relaxed);
        }

        // Restore release K of the user instrument being edited (the toolbar's
        // release menu, docs/patcher-workspace-spec.md §3): cut a new release
        // holding K's files, play it on the track, and reopen the editor on
        // it. Never rewrites a release; the toolbar offers it only while the
        // patch is saved.
        "editor-restore-release" => {
            let release = extract_usize_from_payload(&payload, "release").unwrap_or(0) as u32;
            let Some(session) = ctx.sessions.instrument_edit_session.as_ref() else {
                return;
            };
            if !matches!(session.mode, InstrumentEditMode::EditExisting { .. }) {
                return;
            }
            if session.is_dirty() {
                editor_error(editor, "Save or discard your changes before restoring a release");
                return;
            }
            let Some(current) = editor_release_struct(&session.name) else {
                editor_error(editor, "This instrument has no releases to restore");
                return;
            };
            if current.tier != sequencer::app_paths::ContentTier::User {
                editor_error(editor, "Only your own instruments' releases can be restored");
                return;
            }
            let restored = match sequencer::lisp_host::instrument_release(&format!(
                "user:{}@{release}",
                current.lineage
            )) {
                Ok(Some(restored)) => restored,
                _ => {
                    editor_error(editor, format!("Release v{release} does not exist"));
                    return;
                }
            };
            let source = match std::fs::read_to_string(restored.folder.join("dsp.lisp")) {
                Ok(source) => source,
                Err(error) => {
                    editor_error(editor, format!("Failed to read v{release}: {error}"));
                    return;
                }
            };
            let cut = match sequencer::lisp_host::cut_user_release(session.save_target(), None) {
                Ok(cut) => cut,
                Err(error) => {
                    editor_error(editor, format!("Failed to restore v{release}: {error}"));
                    return;
                }
            };
            let top = cut.source_path.parent().map(Path::to_path_buf).unwrap_or_default();
            for file in ["dsp.lisp", "dsp.layout.json", "ui.lisp"] {
                let from = restored.folder.join(file);
                let to = top.join(file);
                let result = if from.is_file() {
                    std::fs::copy(&from, &to).map(|_| ())
                } else if to.is_file() {
                    std::fs::remove_file(&to)
                } else {
                    Ok(())
                };
                if let Err(error) = result {
                    editor_error(editor, format!("Failed to restore v{release}: {error}"));
                    return;
                }
            }
            sequencer::lisp_host::invalidate_instrument_release_index();
            let track = session.track;
            app.ui.cursor_track = track;
            current_track.store(track, Ordering::Relaxed);
            if let Err(error) = app.replace_current_custom_instrument_sync(&cut.id, &source) {
                editor_error(editor, format!("Failed to load v{release}: {error}"));
                return;
            }
            handle("editor-done", payload_map(&[("force", Value::Bool(true))]), app, editor, ctx);
            handle(
                "enter-edit-instrument",
                payload_map(&[("name", Value::String(cut.id.clone()))]),
                app,
                editor,
                ctx,
            );
            editor.handle_host_event(HostEvent::Status(format!(
                "Restored v{release} as v{}",
                cut.release
            )));
        }

        _ => {}
    }
}

/// The release `name` resolves to, a bare user id meaning its lineage's
/// current files; `None` when unversioned.
fn editor_release_struct(name: &str) -> Option<sequencer::lisp_host::InstrumentRelease> {
    if let Ok(Some(release)) = sequencer::lisp_host::instrument_release(name) {
        return Some(release);
    }
    if name.contains(':') {
        return None;
    }
    let lineage = name.trim_end_matches('/');
    match sequencer::lisp_host::instrument_release(&format!("user:{lineage}")) {
        Ok(Some(mut release)) => {
            release.release = release.current;
            Some(release)
        }
        _ => None,
    }
}

/// The releases of `name`'s lineage (1..=current) and how many tracks of the
/// open project play each, for the toolbar's release menu. Empty when
/// unversioned.
pub(super) fn instrument_release_listing(app: &app::App, name: &str) -> (Vec<i64>, Vec<i64>) {
    let Some(subject) = editor_release_struct(name) else {
        return (Vec::new(), Vec::new());
    };
    let mut tracks = vec![0i64; subject.current as usize];
    for track in 0..app.tracks.len() {
        let Some(track_name) = crate::state_values::current_custom_instrument_name(app, track)
        else {
            continue;
        };
        let Some(release) = editor_release_struct(&track_name) else {
            continue;
        };
        if release.tier != subject.tier || release.lineage != subject.lineage {
            continue;
        }
        if let Some(count) = tracks.get_mut(release.release.saturating_sub(1) as usize) {
            *count += 1;
        }
    }
    ((1..=i64::from(subject.current)).collect(), tracks)
}

/// What a toolbar Save did, for its toast: a new release (and what keeps the
/// previous one), an update of the release this session is amending, or a
/// plain in-place save.
pub(super) fn save_toast_message(
    title: &str,
    release: Option<u32>,
    cut: bool,
    unevaluated_changes: bool,
) -> String {
    let saved = match (release, cut) {
        (Some(release), true) if release > 1 => format!(
            "Saved {title} v{release} · projects using v{} keep it",
            release - 1
        ),
        (Some(release), true) => format!("Saved {title} v{release}"),
        (Some(release), false) => format!("Updated {title} v{release}"),
        (None, _) => format!("Saved {title}"),
    };
    if unevaluated_changes {
        format!("{saved} (code not evaluated yet)")
    } else {
        saved
    }
}

/// A host-command payload map of `entries`.
fn payload_map(entries: &[(&str, Value)]) -> Value {
    Value::Map(
        entries
            .iter()
            .map(|(key, value)| {
                (
                    (*key).to_string(),
                    std::rc::Rc::new(std::cell::RefCell::new(value.clone())),
                )
            })
            .collect(),
    )
}

/// Where an existing instrument's Save writes, and the release it lands in.
pub(super) struct InstrumentSaveTarget {
    pub(super) source_path: PathBuf,
    /// The release written; `None` for an unversioned (factory, in-place)
    /// save.
    pub(super) release: Option<u32>,
    /// The instrument's id after the save, when it moved (a pinned release
    /// or a migrated legacy file).
    pub(super) pinned_name: Option<String>,
}

/// Resolve where `session`'s Save writes (docs/patcher-workspace-spec.md
/// §5.2). Factory and package instruments are developer edits, written in
/// place. A user instrument's first Save in a session cuts a release, as does
/// the first Save after the project was saved (the project may now pin the
/// release being edited); other Saves amend the session's own release.
pub(super) fn prepare_instrument_save_target(
    session: &InstrumentEditSession,
) -> Result<InstrumentSaveTarget, String> {
    let path = session.save_target().to_path_buf();
    let user_root = sequencer::app_paths::app_paths().user_instruments_dir();
    let in_place = InstrumentSaveTarget {
        source_path: path.clone(),
        release: session.working_release.filter(|release| *release > 0),
        pinned_name: None,
    };
    if !path.starts_with(&user_root) {
        return Ok(InstrumentSaveTarget { release: None, ..in_place });
    }
    // Editing a retired release (a track still pinned to it): never write the
    // frozen folder; build on it as the lineage's next release instead.
    if let Some(top) = frozen_release_lineage_top(&session.name) {
        let cut = sequencer::lisp_host::cut_user_release(&top, None)?;
        return Ok(InstrumentSaveTarget {
            source_path: cut.source_path,
            release: Some(cut.release),
            pinned_name: Some(cut.id),
        });
    }
    if session.working_release.is_some() && !session.project_saved_since_release {
        return Ok(in_place);
    }
    let cut = sequencer::lisp_host::cut_user_release(&path, None)?;
    Ok(InstrumentSaveTarget {
        source_path: cut.source_path,
        release: Some(cut.release),
        pinned_name: Some(cut.id),
    })
}

/// The lineage's current-release `dsp.lisp` when `name` is a retired
/// (frozen) release of a user instrument; `None` otherwise.
fn frozen_release_lineage_top(name: &str) -> Option<PathBuf> {
    let release = editor_release_struct(name)?;
    if release.tier != sequencer::app_paths::ContentTier::User || release.release == release.current {
        return None;
    }
    let current = sequencer::lisp_host::instrument_release(&format!(
        "user:{}@{}",
        release.lineage, release.current
    ))
    .ok()
    .flatten()?;
    Some(current.folder.join("dsp.lisp"))
}

/// Point every track playing `engine_id` at `pinned` (a release a Save just
/// cut): the engine's name is what a project save writes and what Edit
/// reopens, so the open project follows the instrument being built.
fn bind_engine_to_release(
    app: &mut app::App,
    track_names: &mut Vec<String>,
    engine_id: usize,
    old_name: &str,
    pinned: &str,
) {
    app.rename_instrument_engine(engine_id, pinned);
    for track in 0..app.tracks.len() {
        if app.graph.track_engine_ids.get(track).copied().flatten() != Some(engine_id) {
            continue;
        }
        if app.tracks[track] == old_name {
            app.tracks[track] = pinned.to_string();
        }
    }
    *track_names = app.tracks.clone();
    crate::param_words::set_track_word_names(track_names);
    sync_sidebar_browser(app, app.ui.cursor_track);
}

/// The release of instrument `name` the toolbar shows; 0 when unversioned. A
/// bare user id names the folder's current files, so its release is the
/// lineage's `current`.
pub(super) fn editor_release_of(name: &str) -> u32 {
    editor_release_struct(name).map_or(0, |release| release.release)
}

/// The bubble conversation carried on an `agentic-submit` payload: a flat list
/// of alternating question/answer strings (patcher `agentic_submit_payload`),
/// rebuilt into turns. A trailing unpaired entry is dropped.
fn agentic_bubble_history(payload: &Value) -> Vec<(String, String)> {
    let Value::Map(map) = payload else {
        return Vec::new();
    };
    let Some(cell) = map.get("history") else {
        return Vec::new();
    };
    let Value::List(items) = &*cell.borrow() else {
        return Vec::new();
    };
    items
        .chunks_exact(2)
        .filter_map(|pair| match (&*pair[0].borrow(), &*pair[1].borrow()) {
            (Value::String(question), Value::String(answer)) => {
                Some((question.clone(), answer.clone()))
            }
            _ => None,
        })
        .collect()
}

#[cfg(test)]
mod toolbar_save_tests {
    use super::*;

    fn existing(path: &Path) -> InstrumentEditSession {
        InstrumentEditSession::begin_edit_existing(
            "probe/".to_string(),
            path.to_path_buf(),
            "*probe*".to_string(),
            0,
            0,
            "(out 0 1)".to_string(),
            CustomInstrumentRunMode::Instrument,
            EditorSurface::Patch,
        )
    }

    #[test]
    fn an_existing_session_is_dirty_only_while_its_patch_or_run_mode_differs() {
        let mut session = existing(Path::new("/tmp/eseq-toolbar-probe/dsp.lisp"));
        assert!(!session.is_dirty(), "an instrument opens clean");
        session.last_valid_source = "(out 1 1)".to_string();
        assert!(session.is_dirty(), "an evaluated edit is unsaved");
        session.last_valid_source = "(out 0 1)".to_string();
        session.run_mode = CustomInstrumentRunMode::FreePatch;
        assert!(session.is_dirty(), "a run mode switch is unsaved until Save writes it");
        session.persisted_run_mode = CustomInstrumentRunMode::FreePatch;
        assert!(!session.is_dirty());
    }

    #[test]
    fn the_save_toast_says_what_the_save_did() {
        assert_eq!(
            save_toast_message("pad", Some(4), true, false),
            "Saved pad v4 · projects using v3 keep it"
        );
        assert_eq!(save_toast_message("pad", Some(4), false, false), "Updated pad v4");
        assert_eq!(save_toast_message("pad", None, false, false), "Saved pad");
        assert_eq!(
            save_toast_message("pad", None, false, true),
            "Saved pad (code not evaluated yet)"
        );
    }

    #[test]
    fn a_pinned_id_displays_as_its_name() {
        assert_eq!(display_instrument_name("user:testsr1@2"), "testsr1");
        assert_eq!(display_instrument_name("factory:Synths/Digi Syn@2"), "Digi Syn");
        assert_eq!(display_instrument_name("pads/glass/"), "glass");
    }

    #[test]
    fn a_draft_is_always_dirty() {
        let session = InstrumentEditSession::begin_create_draft(
            "draft".to_string(),
            PathBuf::from("/tmp/eseq-toolbar-draft/dsp.lisp"),
            "*draft*".to_string(),
            0,
            "(out 0 1)".to_string(),
            PathBuf::from("/tmp/eseq-toolbar-draft"),
            0,
            0,
            EditorSurface::Patch,
        );
        assert!(session.is_dirty(), "nothing of a draft is saved yet");
    }

    /// A track still pinned to a retired release (or a bare id, which keeps
    /// resolving to release 1) opens a frozen folder. Save must build on it
    /// as the lineage's next release, never fail with "frozen".
    #[test]
    fn saving_a_frozen_release_cuts_the_next_release_from_the_lineage_top() {
        let root = sequencer::app_paths::app_paths().user_instruments_dir();
        std::fs::create_dir_all(&root).unwrap();
        let dir = tempfile::Builder::new().prefix("eseq-frozen-save-").tempdir_in(&root).unwrap();
        let lineage = dir.path().file_name().unwrap().to_str().unwrap().to_string();
        std::fs::write(dir.path().join("dsp.lisp"), "(out 0 1)").unwrap();
        let first = sequencer::lisp_host::cut_user_release(&dir.path().join("dsp.lisp"), None)
            .expect("first cut");
        assert_eq!((first.frozen, first.release), (1, 2));

        for name in [format!("user:{lineage}@1"), format!("user:{lineage}")] {
            let frozen = sequencer::lisp_host::instrument_source_path(&name).unwrap();
            assert!(frozen.starts_with(dir.path().join("versions")), "{name} opens the frozen folder");
            let mut session = existing(&frozen);
            session.name = name.clone();
            session.working_release = Some(1);
            let target = prepare_instrument_save_target(&session).expect("save must not fail");
            assert_eq!(target.source_path, dir.path().join("dsp.lisp"), "{name}");
            let release = target.release.expect("a release was cut");
            assert_eq!(target.pinned_name, Some(format!("user:{lineage}@{release}")));
            assert!(frozen_release_lineage_top(target.pinned_name.as_deref().unwrap()).is_none());
        }
    }

    #[test]
    fn a_non_user_instrument_saves_in_place_without_a_release() {
        let path = Path::new("/tmp/eseq-toolbar-factory/Synths/Probe/dsp.lisp");
        let mut session = existing(path);
        session.working_release = Some(4);
        let target = prepare_instrument_save_target(&session).expect("save target");
        assert_eq!(target.source_path, path);
        assert_eq!(target.release, None, "factory edits are developer edits, unversioned");
        assert_eq!(target.pinned_name, None);
    }

    #[test]
    fn an_effect_session_tracks_dirty_and_writes_to_its_saved_home() {
        let mut session = EffectEditSession::begin_edit_existing(
            "fx/".to_string(),
            PathBuf::from("/tmp/eseq-toolbar-fx-draft/dsp.lisp"),
            "*fx-probe*".to_string(),
            EffectEditTarget::Track { track: 0, slot: 0 },
            "(out (in 1) 1)".to_string(),
            EditorSurface::Patch,
        );
        assert!(!session.is_dirty());
        session.last_valid_source = "(out (* 0.5 (in 1)) 1)".to_string();
        assert!(session.is_dirty());
        assert_eq!(session.save_target(), Path::new("/tmp/eseq-toolbar-fx-draft/dsp.lisp"));
        session.save_path = Some(PathBuf::from("/tmp/eseq-toolbar-fx-home/dsp.lisp"));
        assert_eq!(session.save_target(), Path::new("/tmp/eseq-toolbar-fx-home/dsp.lisp"));
    }

    #[test]
    fn a_saved_draft_writes_to_its_new_home_not_the_draft_dir() {
        let mut session = existing(Path::new("/tmp/eseq-toolbar-draft/dsp.lisp"));
        session.save_path = Some(PathBuf::from("/tmp/eseq-toolbar-home/dsp.lisp"));
        let target = prepare_instrument_save_target(&session).expect("save target");
        assert_eq!(target.source_path, Path::new("/tmp/eseq-toolbar-home/dsp.lisp"));
    }
}
