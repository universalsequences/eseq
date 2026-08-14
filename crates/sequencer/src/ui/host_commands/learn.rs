use crate::*;

pub(super) const COMMANDS: &[&str] = &[
    "open-learn-patch",
    "set-learn-target",
    "configure-learn",
    "start-learn-job",
    "stop-learn-job",
    "replan-learn-job",
    "apply-learn-result",
    "close-learn-patch",
];

pub(super) fn handle(
    name: &str,
    payload: Value,
    app: &mut app::App,
    editor: &mut Editor,
    ctx: &mut LoopCtx<'_>,
) {
    match name {
        "open-learn-patch" => {
            let Some(session) = ctx.sessions.instrument_edit_session.as_ref() else {
                editor.handle_host_event(HostEvent::Status(
                    "Open Patch Learn from an instrument patcher buffer".to_string(),
                ));
                return;
            };
            let patcher_buffer = session.buffer_name.clone();
            let requested_buffer = extract_string_from_payload(&payload, "patcher-buffer");
            if requested_buffer.as_deref() != Some(patcher_buffer.as_str()) {
                editor.handle_host_event(HostEvent::Status(
                    "Open Patch Learn from the active instrument patcher buffer".to_string(),
                ));
                return;
            }
            match open_patch_learn_buffer(editor, &patcher_buffer) {
                Ok(()) => editor.handle_host_event(HostEvent::Status(
                    "Opened Patch Learn".to_string(),
                )),
                Err(error) => editor.handle_host_event(HostEvent::Status(error)),
            }
        }
        "set-learn-target" => {
            clear_learn_param_preview(
                app,
                editor.runtime_mut(),
                &mut ctx.sessions.learn_param_preview,
            );
            let path = extract_string_from_payload(&payload, "path")
                .filter(|path| !path.is_empty())
                .map(PathBuf::from);
            let target_name = path.as_ref().map(|path| {
                extract_string_from_payload(&payload, "name")
                    .map(|name| name.trim().to_string())
                    .filter(|name| !name.is_empty())
                    .or_else(|| sequencer::sample_db::display_title_for_sample_path(path))
                    .or_else(|| {
                        path.file_stem()
                            .and_then(|stem| stem.to_str())
                            .map(str::to_string)
                    })
                    .unwrap_or_else(|| "Untitled sample".to_string())
            });
            if let Some(path) = path.as_ref() {
                if !path.is_file() {
                    show_error(editor, format!("Learn target does not exist: {}", path.display()));
                    return;
                }
            }
            let Some(session) = ctx.sessions.instrument_edit_session.as_mut() else {
                show_error(editor, "Open an instrument patch before choosing a learn target".to_string());
                return;
            };
            session.learn_target_path = path.clone();
            session.learn_target_name = target_name;
            if path.is_none() {
                if let Some(pending) = ctx.sessions.pending_learn_job.take() {
                    let _ = pending.job.cancel();
                }
                let rt = editor.runtime_mut();
                reset_learn_reactive(rt);
                rt.set_reactive("SEQ", "learn-target-path", Value::String(String::new()));
                rt.set_reactive("SEQ", "learn-target-name", Value::String(String::new()));
                finish_reactive(editor);
                return;
            }
            {
                let rt = editor.runtime_mut();
                reset_learn_reactive(rt);
                rt.set_reactive(
                    "SEQ",
                    "learn-target-path",
                    Value::String(path.as_ref().unwrap().to_string_lossy().into_owned()),
                );
                rt.set_reactive(
                    "SEQ",
                    "learn-target-name",
                    Value::String(session.learn_target_name.clone().unwrap_or_default()),
                );
                rt.set_reactive("SEQ", "learn-phase", Value::String("planning".to_string()));
            }
            let launched = launch_learn_job(app, session, LearnLaunchKind::Plan, 300, None, None);
            match launched {
                Ok(job) => {
                    replace_learn_job(&mut ctx.sessions.pending_learn_job, job);
                    finish_reactive(editor);
                }
                Err(error) => show_error(editor, error),
            }
        }
        "configure-learn" => {
            let rt = editor.runtime_mut();
            if let Some(epochs) = extract_usize_from_payload(&payload, "epochs") {
                rt.set_reactive("SEQ", "learn-epochs", Value::Number(epochs.clamp(50, 1000) as f64));
            }
            if let Some(pitch_hz) = extract_number_from_payload(&payload, "pitch-hz") {
                if pitch_hz.is_finite() && pitch_hz > 0.0 {
                    rt.set_reactive("SEQ", "learn-pitch-hz", Value::Number(pitch_hz));
                }
            }
            if let Some(gate_frames) = extract_usize_from_payload(&payload, "gate-frames") {
                if gate_frames > 0 {
                    rt.set_reactive("SEQ", "learn-gate-frames", Value::Number(gate_frames as f64));
                }
            }
            finish_reactive(editor);
        }
        "start-learn-job" => {
            clear_learn_param_preview(
                app,
                editor.runtime_mut(),
                &mut ctx.sessions.learn_param_preview,
            );
            let Some(session) = ctx.sessions.instrument_edit_session.as_ref() else {
                show_error(editor, "No instrument patch editor is active".to_string());
                return;
            };
            let epochs = extract_usize_from_payload(&payload, "epochs").unwrap_or(300).clamp(50, 1000) as u64;
            let pitch_hz = extract_number_from_payload(&payload, "pitch-hz").filter(|value| *value > 0.0);
            let gate_frames = extract_usize_from_payload(&payload, "gate-frames").filter(|value| *value > 0).map(|value| value as u64);
            match launch_learn_job(app, session, LearnLaunchKind::Train, epochs, pitch_hz, gate_frames) {
                Ok(job) => {
                    replace_learn_job(&mut ctx.sessions.pending_learn_job, job);
                    let rt = editor.runtime_mut();
                    rt.set_reactive("SEQ", "learn-phase", Value::String("training".to_string()));
                    rt.set_reactive("SEQ", "learn-epochs", Value::Number(epochs as f64));
                    rt.set_reactive("SEQ", "learn-current-epoch", Value::Number(0.0));
                    rt.set_reactive("SEQ", "learn-losses", Value::List(vec![]));
                    rt.set_reactive("SEQ", "learn-error", Value::String(String::new()));
                    finish_reactive(editor);
                }
                Err(error) => show_error(editor, error),
            }
        }
        "stop-learn-job" => {
            let preview_cleared = clear_learn_param_preview(
                app,
                editor.runtime_mut(),
                &mut ctx.sessions.learn_param_preview,
            );
            let Some(pending) = ctx.sessions.pending_learn_job.as_mut() else {
                if preview_cleared {
                    finish_reactive(editor);
                }
                return;
            };
            pending.cancel_requested = true;
            if let Err(error) = pending.job.cancel() {
                show_error(editor, error);
            } else {
                editor.handle_host_event(HostEvent::Status("Stopping patch learning...".to_string()));
                finish_reactive(editor);
            }
        }
        "replan-learn-job" => {
            clear_learn_param_preview(
                app,
                editor.runtime_mut(),
                &mut ctx.sessions.learn_param_preview,
            );
            let Some(session) = ctx.sessions.instrument_edit_session.as_ref() else {
                return;
            };
            let pitch_hz = extract_number_from_payload(&payload, "pitch-hz")
                .filter(|value| value.is_finite() && *value > 0.0);
            let gate_frames = extract_usize_from_payload(&payload, "gate-frames")
                .filter(|value| *value > 0)
                .map(|value| value as u64);
            match launch_learn_job(app, session, LearnLaunchKind::Plan, 300, pitch_hz, gate_frames) {
                Ok(job) => {
                    replace_learn_job(&mut ctx.sessions.pending_learn_job, job);
                    editor.runtime_mut().set_reactive("SEQ", "learn-phase", Value::String("planning".to_string()));
                    finish_reactive(editor);
                }
                Err(error) => show_error(editor, error),
            }
        }
        "apply-learn-result" => {
            let Some(session) = ctx.sessions.instrument_edit_session.as_ref() else {
                show_error(editor, "No instrument patch editor is active".to_string());
                return;
            };
            if ctx
                .sessions
                .learn_param_preview
                .as_ref()
                .is_some_and(|preview| preview.track != session.track)
            {
                show_error(editor, "The learned result belongs to a different track".to_string());
                return;
            }
            match apply_learn_param_preview(app, &mut ctx.sessions.learn_param_preview) {
                Ok(_) => {
                    editor
                        .runtime_mut()
                        .set_reactive("SEQ", "learn-applied", Value::Bool(true));
                    ctx.shared.ui_epoch.fetch_add(1, Ordering::Relaxed);
                    finish_reactive(editor);
                    editor.handle_host_event(HostEvent::Status(
                        "Applied learned parameters as one undoable edit".to_string(),
                    ));
                }
                Err(error) => show_error(editor, error),
            }
        }
        "close-learn-patch" => {
            clear_learn_param_preview(
                app,
                editor.runtime_mut(),
                &mut ctx.sessions.learn_param_preview,
            );
            if let Some(pending) = ctx.sessions.pending_learn_job.take() {
                let _ = pending.job.cancel();
            }
            finish_reactive(editor);
        }
        _ => {}
    }
}

/// Mounts Patch Learn as its own render root before installing the split.
///
/// Named effects evaluated from inside another Lisp function are emitted as
/// subtree updates. That cannot initialize a tile-created scratch buffer, so
/// this editor-owned boundary deliberately evaluates `effect-buffer` as a
/// top-level form and commits it before changing the layout.
pub(crate) fn open_patch_learn_buffer(
    editor: &mut Editor,
    patcher_buffer: &str,
) -> Result<(), String> {
    editor
        .runtime_mut()
        .eval_str(
            r#"(effect-buffer "*patch-learn*"
                  (box :width :fill :height :fill
                    (eseq.patch-learn/panel)))"#,
        )
        .map_err(|error| format!("Could not create Patch Learn UI: {error:?}"))?;
    editor.refresh_runtime_side_effects();

    let patcher_buffer = escape_lisp_string(patcher_buffer);
    editor
        .runtime_mut()
        .eval_str(&format!(
            "(eseq.seq-layout/apply-instrument-patcher-learn-layout \"{patcher_buffer}\" \"*patch-learn*\")"
        ))
        .map_err(|error| format!("Could not open Patch Learn layout: {error:?}"))?;
    editor.refresh_runtime_side_effects();
    editor.mark_needs_redraw();
    Ok(())
}

fn extract_number_from_payload(payload: &Value, key: &str) -> Option<f64> {
    let Value::Map(map) = payload else { return None; };
    match map.get(key).map(|value| value.borrow().clone()) {
        Some(Value::Number(value)) => Some(value),
        _ => None,
    }
}

fn show_error(editor: &mut Editor, error: String) {
    let rt = editor.runtime_mut();
    rt.set_reactive("SEQ", "learn-phase", Value::String("error".to_string()));
    rt.set_reactive("SEQ", "learn-error", Value::String(error.clone()));
    finish_reactive(editor);
    editor.handle_host_event(HostEvent::Status(error));
}

fn finish_reactive(editor: &mut Editor) {
    editor.runtime_mut().run_reactive_cycle();
    editor.refresh_runtime_side_effects();
    editor.mark_needs_redraw();
}
