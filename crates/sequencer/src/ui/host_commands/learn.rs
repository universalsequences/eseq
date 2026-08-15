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
            let launched = launch_learn_job(
                app,
                session,
                LearnLaunchKind::Plan,
                sequencer::learn_job::LearnTrainingConfig::default(),
                None,
                None,
            );
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
            if let Some(method) = extract_string_from_payload(&payload, "method") {
                set_learn_method(rt, &method);
            }
            for (payload_key, reactive_key, min, max) in [
                ("epochs", "learn-epochs", 1, 2000),
                ("cma-generations", "learn-cma-generations", 1, 1000),
                ("cma-population", "learn-cma-population", 0, 4096),
                ("cma-seed", "learn-cma-seed", 0, u32::MAX as usize),
                ("cma-forward-batch", "learn-cma-forward-batch", 0, 4096),
                ("local-epochs", "learn-local-epochs", 0, 2000),
                ("cma-continue", "learn-cma-continue", 0, 4096),
                ("cma-refine-epochs", "learn-cma-refine-epochs", 0, 2000),
                ("cma-final-epochs", "learn-cma-final-epochs", 0, 2000),
            ] {
                if let Some(value) = extract_usize_from_payload(&payload, payload_key) {
                    rt.set_reactive("SEQ", reactive_key, Value::Number(value.clamp(min, max) as f64));
                }
            }
            if let Some(population) = extract_usize_from_payload(&payload, "cma-population") {
                if population > 0 && population < 4 {
                    rt.set_reactive("SEQ", "learn-cma-population", Value::Number(4.0));
                }
            }
            if let Some(sigma) = extract_number_from_payload(&payload, "cma-sigma") {
                if sigma.is_finite() && sigma > 0.0 {
                    rt.set_reactive("SEQ", "learn-cma-sigma", Value::Number(sigma.min(10.0)));
                }
            }
            if let Some(mode) = extract_string_from_payload(&payload, "cma-refine-mode") {
                if matches!(mode.as_str(), "Auto" | "Scalar" | "Batched") {
                    rt.set_reactive("SEQ", "learn-cma-refine-mode", Value::String(mode));
                }
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
            let training = match learn_training_config_from_payload(&payload) {
                Ok(training) => training,
                Err(error) => {
                    show_error(editor, error);
                    return;
                }
            };
            let pitch_hz = extract_number_from_payload(&payload, "pitch-hz").filter(|value| *value > 0.0);
            let gate_frames = extract_usize_from_payload(&payload, "gate-frames").filter(|value| *value > 0).map(|value| value as u64);
            match launch_learn_job(app, session, LearnLaunchKind::Train, training, pitch_hz, gate_frames) {
                Ok(job) => {
                    replace_learn_job(&mut ctx.sessions.pending_learn_job, job);
                    let rt = editor.runtime_mut();
                    rt.set_reactive("SEQ", "learn-phase", Value::String("training".to_string()));
                    rt.set_reactive("SEQ", "learn-stage", Value::String("starting".to_string()));
                    rt.set_reactive("SEQ", "learn-current-epoch", Value::Number(0.0));
                    rt.set_reactive("SEQ", "learn-total-epochs", Value::Number(0.0));
                    rt.set_reactive("SEQ", "learn-losses", Value::List(vec![]));
                    rt.set_reactive("SEQ", "learn-optimization-losses", Value::List(vec![]));
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
            match launch_learn_job(
                app,
                session,
                LearnLaunchKind::Plan,
                sequencer::learn_job::LearnTrainingConfig::default(),
                pitch_hz,
                gate_frames,
            ) {
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

fn set_learn_method(rt: &mut Runtime, method: &str) {
    if !matches!(
        method,
        "Local fit + basin check" | "Evolutionary search only" | "Evolutionary search + training"
    ) {
        return;
    }
    // Pipeline-specific defaults live in the reactive-state initializer. A
    // method switch must not erase values the user already tuned; search-only
    // enforces its disabled Adam stages when constructing the launch config.
    rt.set_reactive("SEQ", "learn-method", Value::String(method.to_string()));
}

fn learn_training_config_from_payload(
    payload: &Value,
) -> Result<sequencer::learn_job::LearnTrainingConfig, String> {
    use sequencer::learn_job::{CmaRefineMode, LearnTrainingConfig};
    let method = extract_string_from_payload(payload, "method")
        .unwrap_or_else(|| "Local fit + basin check".to_string());
    let integer = |key: &str, default: usize| {
        extract_usize_from_payload(payload, key).unwrap_or(default) as u64
    };
    match method.as_str() {
        "Local fit + basin check" => Ok(LearnTrainingConfig::Legacy {
            epochs: integer("epochs", 300),
        }),
        "Evolutionary search only" | "Evolutionary search + training" => {
            let refine_mode = match extract_string_from_payload(payload, "cma-refine-mode")
                .as_deref()
                .unwrap_or("Batched")
            {
                "Auto" => CmaRefineMode::Auto,
                "Scalar" => CmaRefineMode::Scalar,
                "Batched" => CmaRefineMode::Batched,
                mode => return Err(format!("Unknown CMA refinement mode: {mode}")),
            };
            let search_only = method == "Evolutionary search only";
            Ok(LearnTrainingConfig::CmaEs {
                generations: integer("cma-generations", 12),
                population: integer("cma-population", 0),
                sigma: extract_number_from_payload(payload, "cma-sigma").unwrap_or(0.2),
                seed: integer("cma-seed", 1),
                forward_batch: integer("cma-forward-batch", 0),
                local_epochs: if search_only { 0 } else { integer("local-epochs", 0) },
                continue_candidates: integer("cma-continue", 8),
                refine_epochs: if search_only { 0 } else { integer("cma-refine-epochs", 5) },
                refine_mode,
                final_epochs: if search_only { 0 } else { integer("cma-final-epochs", 300) },
            })
        }
        _ => Err(format!("Unknown Patch Learn training method: {method}")),
    }
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

#[cfg(test)]
mod tests {
    use super::learn_training_config_from_payload;
    use crate::{values, Value};
    use sequencer::learn_job::{CmaRefineMode, LearnTrainingConfig};

    #[test]
    fn patch_learn_payload_builds_a_fully_explicit_cma_pipeline() {
        let payload = values::map_value([
            ("method", Value::String("Evolutionary search + training".to_string())),
            ("cma-generations", Value::Number(20.0)),
            ("cma-population", Value::Number(96.0)),
            ("cma-sigma", Value::Number(0.12)),
            ("cma-seed", Value::Number(9.0)),
            ("cma-forward-batch", Value::Number(24.0)),
            ("local-epochs", Value::Number(40.0)),
            ("cma-continue", Value::Number(12.0)),
            ("cma-refine-epochs", Value::Number(7.0)),
            ("cma-refine-mode", Value::String("Scalar".to_string())),
            ("cma-final-epochs", Value::Number(600.0)),
        ]);
        assert_eq!(
            learn_training_config_from_payload(&payload).unwrap(),
            LearnTrainingConfig::CmaEs {
                generations: 20,
                population: 96,
                sigma: 0.12,
                seed: 9,
                forward_batch: 24,
                local_epochs: 40,
                continue_candidates: 12,
                refine_epochs: 7,
                refine_mode: CmaRefineMode::Scalar,
                final_epochs: 600,
            }
        );
    }

    #[test]
    fn search_only_forces_every_adam_stage_off() {
        let payload = values::map_value([
            ("method", Value::String("Evolutionary search only".to_string())),
            ("local-epochs", Value::Number(100.0)),
            ("cma-refine-epochs", Value::Number(50.0)),
            ("cma-final-epochs", Value::Number(500.0)),
        ]);
        let LearnTrainingConfig::CmaEs {
            local_epochs,
            refine_epochs,
            final_epochs,
            ..
        } = learn_training_config_from_payload(&payload).unwrap() else {
            panic!("search-only preset must use CMA-ES");
        };
        assert_eq!((local_epochs, refine_epochs, final_epochs), (0, 0, 0));
    }
}
