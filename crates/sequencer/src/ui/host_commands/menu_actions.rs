//! Menu-facing application actions. Definitions and enabling policy remain in Lisp.
use super::file_menu::{activate_dialog_tile, lisp_string};
use crate::*;

pub(super) const COMMANDS: &[&str] = &[
    "menu-pattern-double",
    "menu-pattern-half",
    "menu-pattern-clone",
    "menu-edit",
    "menu-pattern-transform",
    "menu-search-commands",
    "menu-import-samples",
    "menu-keyboard-shortcuts",
    "menu-open-recent",
    "menu-pattern-transpose",
    "menu-pattern-clear-request",
];

pub(super) fn handle(
    name: &str,
    payload: Value,
    app: &mut app::App,
    editor: &mut Editor,
    ctx: &mut LoopCtx<'_>,
) {
    let result = (|| -> Result<(), String> {
        match name {
            "menu-open-recent" => {
                let Value::Map(map) = &payload else {
                    return Err("Expected project name".into());
                };
                let name = map_string(map, "name").ok_or("Expected project name")?;
                if app.has_unsaved_changes() {
                    activate_dialog_tile(editor);
                    editor.runtime_mut().eval_str(&format!(
                        "(eseq.file-dialogs/open-confirm \"Discard unsaved changes and open this project?\" (lambda () (host-command \"load-project\" (dict :name {}))))", lisp_string(&name)
                    )).map_err(|e| format!("{e:?}"))?;
                } else {
                    super::project::handle("load-project", payload, app, editor, ctx);
                }
            }
            "menu-keyboard-shortcuts" => {
                activate_dialog_tile(editor);
                editor
                    .runtime_mut()
                    .eval_str("(eseq.manual/open-node \"customization\")")
                    .map_err(|e| format!("{e:?}"))?;
            }
            "menu-pattern-clear-request" => {
                let track = ctx.shared.current_track.load(Ordering::Relaxed);
                activate_dialog_tile(editor);
                editor.runtime_mut().eval_str(&format!(
                    "(eseq.file-dialogs/open-confirm \"Clear every step and parameter lock in this track pattern?\" (lambda () (host-command \"menu-pattern-transform\" (dict :operation \"clear\" :track {track}))))"
                )).map_err(|e| format!("{e:?}"))?;
            }
            "menu-pattern-transpose" => {
                let Value::Map(map) = &payload else {
                    return Err("Expected transpose amount".into());
                };
                if metal_has_selected_bus(editor) {
                    return Err("Select an individual track pattern".into());
                }
                let delta = map
                    .get("semitones")
                    .and_then(|v| {
                        if let Value::Number(n) = *v.borrow() {
                            Some(n as f32)
                        } else {
                            None
                        }
                    })
                    .ok_or("Expected semitones")?;
                if !delta.is_finite() {
                    return Err("Expected finite semitones".into());
                }
                let track = ctx.shared.current_track.load(Ordering::Relaxed);
                if track >= app.tracks.len() {
                    return Err("No track selected".into());
                }
                let steps: Vec<usize> =
                    (0..app.state.pattern.track_params[track].get_num_steps()).collect();
                app::edit::apply_recorded_step_mutation(
                    app,
                    track,
                    &steps,
                    "Transpose track pattern",
                    |app| {
                        for &step in &steps {
                            let value =
                                app.state.pattern.step_data[track].get(step, StepParam::Transpose);
                            app.state.set_step_param_no_publish(
                                track,
                                step,
                                StepParam::Transpose,
                                value + delta,
                            );
                        }
                        Ok(())
                    },
                )
                .map_err(|e| format!("{e:?}"))?;
                ctx.shared
                    .ui_invalidations
                    .push(UiInvalidation::StepBatch { track, steps });
                ctx.shared.ui_epoch.fetch_add(1, Ordering::Relaxed);
            }
            "menu-search-commands" => crate::application_menu::search_commands(editor)?,
            "menu-edit" => {
                let action = if let Value::Map(map) = &payload {
                    map_string(map, "action").unwrap_or_default()
                } else {
                    String::new()
                };
                if action == "select-all"
                    && editor.active_buffer().view_mode == ViewMode::UiOnly
                    && !focused_widget_captures_text_input(editor)
                    && !focused_widget_matches(editor, |node| node.widget_type == "patcher")
                {
                    editor
                        .runtime_mut()
                        .invoke_global("eseq.step-grid-interactions/seq-global-select-all", vec![])
                        .map_err(|e| format!("{e:?}"))?;
                    return Ok(());
                }
                if editor.perform_edit_action(&action) {
                    return Ok(());
                }
                if editor.active_buffer().name == "*fx*" {
                    let command = match action.as_str() {
                        "copy" | "cut" => Some("seq-copy-selected-effect"),
                        "paste" => Some("seq-paste-effect"),
                        "delete" => Some("eseq.effects.buffers/delete-key"),
                        _ => None,
                    };
                    if let Some(command) = command {
                        let copied = editor
                            .runtime_mut()
                            .invoke_global(command, vec![])
                            .map_err(|e| format!("{e:?}"))?;
                        if action == "cut" && matches!(copied, Some(Value::Bool(true))) {
                            editor
                                .runtime_mut()
                                .invoke_global("seq-delete-active-target", vec![])
                                .map_err(|e| format!("{e:?}"))?;
                        }
                        return Ok(());
                    }
                }
                let shared = ctx.shared;
                if app.tracks.is_empty() || metal_has_selected_bus(editor) {
                    return Ok(());
                }
                match action.as_str() {
                    "copy" | "cut" | "paste" => {
                        perform_step_clipboard_action(
                            editor,
                            if action == "paste" {
                                StepClipboardShortcut::Paste
                            } else {
                                StepClipboardShortcut::Copy
                            },
                            &shared.state,
                            &shared.current_track,
                            &shared.selected_steps,
                            &shared.step_clipboard,
                        );
                        if action == "cut" {
                            if shared.selected_steps.lock().unwrap().is_empty() {
                                if let Some(step) = current_metal_cursor_step(editor) {
                                    shared.selected_steps.lock().unwrap().insert(step);
                                }
                            }
                            editor
                                .runtime_mut()
                                .invoke_global("seq-delete-selected-steps", vec![])
                                .map_err(|e| format!("{e:?}"))?;
                        }
                    }
                    "select-all" => {
                        editor
                            .runtime_mut()
                            .invoke_global("seq-select-all-steps", vec![])
                            .map_err(|e| format!("{e:?}"))?;
                    }
                    "delete" => {
                        editor
                            .runtime_mut()
                            .invoke_global("eseq.sequencer-keys/delete-selected-steps", vec![])
                            .map_err(|e| format!("{e:?}"))?;
                    }
                    _ => return Err(format!("Unknown edit action: {action}")),
                }
            }
            "menu-pattern-transform" => {
                let Value::Map(map) = &payload else {
                    return Err("Expected pattern operation".into());
                };
                let operation = map_string(map, "operation").unwrap_or_default();
                if metal_has_selected_bus(editor) {
                    return Err("Select an individual track pattern".into());
                }
                let track = map_usize(map, "track")
                    .unwrap_or_else(|| ctx.shared.current_track.load(Ordering::Relaxed));
                if track >= app.tracks.len() {
                    return Err("No track pattern selected".into());
                }
                let steps: Vec<usize> =
                    (0..app.state.pattern.track_params[track].get_num_steps()).collect();
                let command = match operation.as_str() {
                    "clear" => app::AppCommand::ClearSteps {
                        track,
                        steps: steps.clone(),
                    },
                    "left" | "right" => app::AppCommand::RotateSteps {
                        track,
                        steps: steps.clone(),
                        direction: if operation == "left" { -1 } else { 1 },
                    },
                    _ => return Err(format!("Unknown pattern operation: {operation}")),
                };
                app::edit::apply_recorded_step_command(app, &command)
                    .map_err(|e| format!("{e:?}"))?;
                ctx.shared
                    .ui_invalidations
                    .push(UiInvalidation::StepBatch { track, steps });
                ctx.shared.ui_epoch.fetch_add(1, Ordering::Relaxed);
                ctx.shared.fx_epoch.fetch_add(1, Ordering::Relaxed);
            }
            "menu-import-samples" => {
                let paths = crate::application_menu::choose_sample_paths()?;
                if paths.is_empty() {
                    return Ok(());
                }
                let draft = SampleImportDraft::from_drop(
                    paths,
                    &sequencer::app_paths::app_paths().sample_db_path(),
                )?;
                if draft.is_empty() {
                    return Err("No supported audio files found".into());
                }
                install_draft(draft);
                open_sample_import_modal(editor);
            }
            "menu-pattern-double" | "menu-pattern-half" => {
                if app.tracks.is_empty() {
                    return Err("No track pattern is selected".into());
                }
                let operation = if name == "menu-pattern-double" {
                    PatternLengthShortcut::Double
                } else {
                    PatternLengthShortcut::Halve
                };
                resize_selected_pattern(editor, operation);
            }
            "menu-pattern-clone" => {
                if !app.tracks.is_empty() && !metal_has_selected_bus(editor) {
                    editor
                        .runtime_mut()
                        .invoke_global("seq-clone-active-track-pattern", Vec::new())
                        .map_err(|error| format!("{error:?}"))?;
                }
            }
            _ => {}
        }
        Ok(())
    })();
    if let Err(error) = result {
        editor.show_transient_message(error);
    }
    editor.runtime_mut().run_reactive_cycle();
    editor.refresh_runtime_side_effects();
    editor.mark_needs_redraw();
}
