use crate::*;

pub(super) const COMMANDS: &[&str] = &[
    "open-learn-patch",
    "set-learn-target",
    "start-learn-job",
    "stop-learn-job",
    "replan-learn-job",
    "apply-learn-result",
    "close-learn-patch",
    "set-learn",
];

/// The integer training settings: (field, min, max), the field the `learn`
/// kind's name. `set-learn` (the `learn` kind's setters) takes only a value
/// in the range.
const LEARN_INT_SETTINGS: [(&str, usize, usize); 9] = [
    ("epochs", 1, 2000),
    ("cma-generations", 1, 1000),
    ("cma-population", 0, 4096),
    ("cma-seed", 0, u32::MAX as usize),
    ("cma-forward-batch", 0, 4096),
    ("local-epochs", 0, 2000),
    ("cma-continue", 0, 4096),
    ("cma-refine-epochs", 0, 2000),
    ("cma-final-epochs", 0, 2000),
];

use crate::presented::{LEARN_METHODS, LEARN_REFINE_MODES};

pub(super) fn handle(
    name: &str,
    payload: Value,
    app: &mut app::App,
    editor: &mut Editor,
    ctx: &mut LoopCtx<'_>,
) {
    let current_track = ctx.shared.current_track.load(Ordering::Relaxed);
    match name {
        "open-learn-patch" => {
            if let Some(message) = sequencer::learn_job::training_unavailable_reason() {
                editor.handle_host_event(HostEvent::Status(message.to_string()));
                return;
            }
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
                current_track,
            );
            let path = extract_string_from_payload(&payload, "path")
                .filter(|path| !path.is_empty())
                .map(|path| sequencer::app_paths::resolve_sample_ref(Path::new(&path)));
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
                present_learn(editor.runtime_mut(), |l| {
                    l.reset();
                    l.target_path.clear();
                    l.target_name.clear();
                });
                finish_reactive(editor);
                return;
            }
            present_learn(editor.runtime_mut(), |l| {
                l.reset();
                l.target_path = path.as_ref().unwrap().to_string_lossy().into_owned();
                l.target_name = session.learn_target_name.clone().unwrap_or_default();
                l.phase = "planning".to_string();
            });
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
        "set-learn" => match set_learn(editor.runtime_mut(), &payload) {
            Ok(()) => finish_reactive(editor),
            Err(error) => {
                finish_reactive(editor);
                editor.handle_host_event(HostEvent::Status(format!("set-learn: {error}")));
            }
        },
        "start-learn-job" => {
            if let Some(message) = sequencer::learn_job::training_unavailable_reason() {
                editor.handle_host_event(HostEvent::Status(message.to_string()));
                return;
            }
            clear_learn_param_preview(
                app,
                editor.runtime_mut(),
                &mut ctx.sessions.learn_param_preview,
                current_track,
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
                    present_learn(rt, |l| {
                        l.phase = "training".to_string();
                        l.stage = "starting".to_string();
                        l.current_epoch = 0.0;
                        l.total_epochs = 0.0;
                        l.losses.clear();
                        l.optimization_losses.clear();
                        l.error.clear();
                    });
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
                current_track,
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
                current_track,
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
                    present_learn(editor.runtime_mut(), |l| l.phase = "planning".to_string());
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
                    present_learn(editor.runtime_mut(), |l| l.applied = true);
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
                current_track,
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

/// One training setting (`set-learn`: `:field`, `:value`), the `learn`
/// kind's setter: the current value always works, anything else must pass
/// [`validate_learn_setting`] (no clamping).
fn set_learn(rt: &mut Runtime, payload: &Value) -> Result<(), String> {
    let Value::Map(map) = payload else {
        return Err("needs a :field and a :value".to_string());
    };
    let (field, value) = super::track_settings::SetValue::field(map)?;
    let Some(current) = crate::presented::presented(|p| p.learn.get().setting(&field)) else {
        return Err(format!("no learn setting {field}"));
    };
    if &current == value.value() {
        return Ok(());
    }
    let next = validate_learn_setting(&field, value.value())?;
    present_learn(rt, |l| l.set_setting(&field, next));
    Ok(())
}

/// A training setting's value under the value rule (spec §14.2c), as
/// stored: the method or refine mode one of its labels (case-insensitive),
/// an integer setting an integer in its range (a population 0 or at least
/// 4), the sigma a number above 0 up to 10, the pitch a positive number and
/// the gate a positive integer. `set-learn` stores only what this accepts.
fn validate_learn_setting(field: &str, value: &Value) -> Result<Value, String> {
    let value = super::track_settings::SetValue::new(field, value.clone());
    Ok(match field {
        "method" => Value::String(LEARN_METHODS[value.choice(&LEARN_METHODS)?].to_string()),
        "cma-refine-mode" => {
            Value::String(LEARN_REFINE_MODES[value.choice(&LEARN_REFINE_MODES)?].to_string())
        }
        "cma-sigma" => match value.number(0.0, 10.0)? {
            sigma if sigma > 0.0 => Value::Number(sigma),
            _ => return value.fail("a number above 0 up to 10"),
        },
        "pitch-hz" => match value.finite()? {
            hz if hz > 0.0 => Value::Number(hz),
            _ => return value.fail("a positive number"),
        },
        "gate-frames" => Value::Number(value.integer(1, u32::MAX as usize)? as f64),
        "cma-population" => match value.integer(0, 4096)? {
            1..=3 => return value.fail("0 (auto) or an integer from 4 to 4096"),
            population => Value::Number(population as f64),
        },
        _ => {
            let (_, min, max) = LEARN_INT_SETTINGS
                .iter()
                .find(|(key, _, _)| *key == field)
                .ok_or_else(|| format!("no learn setting {field}"))?;
            Value::Number(value.integer(*min, *max)? as f64)
        }
    })
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
    present_learn_error(editor.runtime_mut(), error.clone());
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
