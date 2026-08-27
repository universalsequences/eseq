use crate::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum LearnLaunchKind {
    Plan,
    Train,
}

pub(crate) struct PendingLearnJob {
    pub(crate) job: sequencer::learn_job::LearnJob,
    pub(crate) kind: LearnLaunchKind,
    pub(crate) expected_seed: std::collections::BTreeMap<String, f64>,
    pub(crate) saw_plan: bool,
    pub(crate) saw_terminal: bool,
    pub(crate) saw_host_error: bool,
    pub(crate) cancel_requested: bool,
    pub(crate) losses: Vec<f64>,
}

/// Engine-only instrument parameter values streamed by Patch Learn.
///
/// The slot defaults remain the source of truth while this layer is active.
/// Keeping only the affected parameter indices is enough to restore the
/// effective document/macro value without ever creating an undo entry.
pub(crate) struct LearnParamPreview {
    pub(crate) track: usize,
    param_indices: Vec<usize>,
    stored_values: std::collections::BTreeMap<usize, f32>,
}

pub(crate) fn learn_seed_for_session(
    app: &app::App,
    session: &InstrumentEditSession,
) -> Result<sequencer::learn_job::LearnSeed, String> {
    let descriptor = app
        .graph
        .instrument_descriptors
        .get(session.track)
        .ok_or_else(|| "The edited track has no instrument descriptor".to_string())?;
    let slot = app
        .state
        .pattern
        .instrument_slots
        .get(session.track)
        .ok_or_else(|| "The edited track has no instrument parameter slot".to_string())?;
    let count = (slot.num_params.load(Ordering::Relaxed) as usize).min(descriptor.params.len());
    let params = descriptor
        .params
        .iter()
        .take(count)
        .enumerate()
        .filter(|(_, param)| {
            !param.name.starts_with("__host_")
                && !param.name.starts_with("__dgen_")
                && !param.name.starts_with("mod ")
        })
        .map(|(index, param)| {
            (
                param.name.clone(),
                f64::from(param.stored_to_user(slot.defaults.get(index))),
            )
        })
        .collect();
    Ok(sequencer::learn_job::LearnSeed { params })
}

pub(crate) fn launch_learn_job(
    app: &app::App,
    session: &InstrumentEditSession,
    kind: LearnLaunchKind,
    training: sequencer::learn_job::LearnTrainingConfig,
    pitch_hz: Option<f64>,
    gate_frames: Option<u64>,
) -> Result<PendingLearnJob, String> {
    let target_path = session
        .learn_target_path
        .clone()
        .ok_or_else(|| "Choose a target sample first".to_string())?;
    if !session.visible_revision_valid {
        return Err("Fix the current patch before starting learning".to_string());
    }
    let seed = learn_seed_for_session(app, session)?;
    let expected_seed = seed.params.clone();
    let patch_source = sequencer::lisp_host::effective_instrument_source(
        &session.last_valid_source,
        app.graph.sample_rate,
    )?;
    let spec = sequencer::learn_job::LearnJobSpec {
        patch_path: session.path.clone(),
        patch_source,
        target_path,
        seed,
        training,
        pitch_hz,
        gate_frames,
        plan_only: kind == LearnLaunchKind::Plan,
    };
    let launcher = sequencer::learn_job::LearnJobLauncher::from_app_paths(
        sequencer::app_paths::app_paths(),
    );
    Ok(PendingLearnJob {
        job: launcher.launch(spec)?,
        kind,
        expected_seed,
        saw_plan: false,
        saw_terminal: false,
        saw_host_error: false,
        cancel_requested: false,
        losses: Vec::new(),
    })
}

pub(crate) fn replace_learn_job(
    slot: &mut Option<PendingLearnJob>,
    replacement: PendingLearnJob,
) {
    if let Some(previous) = slot.take() {
        let _ = previous.job.cancel();
    }
    *slot = Some(replacement);
}

pub(crate) fn reset_learn_reactive(rt: &mut Runtime) {
    rt.set_reactive("SEQ", "learn-phase", Value::String("pick".to_string()));
    rt.set_reactive("SEQ", "learn-plan-params", Value::List(vec![]));
    rt.set_reactive("SEQ", "learn-stage", Value::String(String::new()));
    rt.set_reactive("SEQ", "learn-current-epoch", Value::Number(0.0));
    rt.set_reactive("SEQ", "learn-total-epochs", Value::Number(0.0));
    rt.set_reactive("SEQ", "learn-loss", Value::Number(0.0));
    rt.set_reactive("SEQ", "learn-losses", Value::List(vec![]));
    rt.set_reactive("SEQ", "learn-optimization-losses", Value::List(vec![]));
    rt.set_reactive("SEQ", "learn-epoch-params", Value::List(vec![]));
    rt.set_reactive("SEQ", "learn-checkpoint-wav", Value::String(String::new()));
    rt.set_reactive("SEQ", "learn-improvement-pct", Value::Number(0.0));
    rt.set_reactive("SEQ", "learn-abs-distance", Value::Number(0.0));
    rt.set_reactive("SEQ", "learn-basin-check", Value::String(String::new()));
    rt.set_reactive("SEQ", "learn-result-deltas", Value::List(vec![]));
    rt.set_reactive("SEQ", "learn-seeded-wav", Value::String(String::new()));
    rt.set_reactive("SEQ", "learn-final-wav", Value::String(String::new()));
    rt.set_reactive("SEQ", "learn-applied", Value::Bool(false));
    rt.set_reactive("SEQ", "learn-error", Value::String(String::new()));
}

pub(crate) fn poll_learn_job(
    app: &mut app::App,
    sessions: &mut EditSessionState,
    editor: &mut Editor,
    current_track: usize,
) {
    if sessions.instrument_edit_session.is_none() {
        if let Some(pending) = sessions.pending_learn_job.take() {
            let _ = pending.job.cancel();
        }
        if clear_learn_param_preview(
            app,
            editor.runtime_mut(),
            &mut sessions.learn_param_preview,
            current_track,
        ) {
            editor.runtime_mut().run_reactive_cycle();
            editor.refresh_runtime_side_effects();
            editor.mark_needs_redraw();
        }
        return;
    }
    let preview_track = sessions
        .instrument_edit_session
        .as_ref()
        .map(|session| session.track)
        .expect("instrument edit session checked above");
    let mut finished = false;
    let mut dirty = false;
    let mut clear_preview = false;
    let Some(pending) = sessions.pending_learn_job.as_mut() else {
        return;
    };
    let (updates, disconnected) = take_learn_updates_through_epoch(&pending.job.receiver);
    for update in updates {
        dirty = true;
        let rt = editor.runtime_mut();
        match update {
            sequencer::learn_job::LearnJobUpdate::Event(event) => {
                pending.saw_terminal |= event.is_terminal();
                pending.saw_plan |=
                    matches!(event, sequencer::learn_job::LearnEvent::Plan { .. });
                publish_event(rt, pending, &event);
                let preview_values = match &event {
                    sequencer::learn_job::LearnEvent::Epoch { params, .. } => {
                        Some(params.clone())
                    }
                    sequencer::learn_job::LearnEvent::Result { deltas, .. } => Some(
                        deltas
                            .iter()
                            .map(|(name, delta)| (name.clone(), delta.to))
                            .collect(),
                    ),
                    sequencer::learn_job::LearnEvent::Error { .. } => {
                        clear_preview = true;
                        None
                    }
                    _ => None,
                };
                if let Some(values) = preview_values {
                    if let Err(error) = set_learn_param_preview(
                        app,
                        rt,
                        &mut sessions.learn_param_preview,
                        preview_track,
                        current_track,
                        &values,
                    ) {
                        pending.saw_host_error = true;
                        let _ = pending.job.cancel();
                        rt.set_reactive("SEQ", "learn-phase", Value::String("error".to_string()));
                        rt.set_reactive("SEQ", "learn-error", Value::String(error));
                        clear_preview = true;
                    }
                }
            }
            sequencer::learn_job::LearnJobUpdate::ProtocolError { error, .. }
            | sequencer::learn_job::LearnJobUpdate::IoError(error) => {
                pending.saw_host_error = true;
                clear_preview = true;
                rt.set_reactive("SEQ", "learn-phase", Value::String("error".to_string()));
                rt.set_reactive("SEQ", "learn-error", Value::String(error));
            }
            sequencer::learn_job::LearnJobUpdate::Exited { success, code } => {
                if !pending.saw_terminal && !pending.saw_host_error {
                    if pending.kind == LearnLaunchKind::Plan && success && pending.saw_plan {
                        rt.set_reactive("SEQ", "learn-phase", Value::String("configure".to_string()));
                    } else if pending.cancel_requested {
                        clear_preview = true;
                        rt.set_reactive("SEQ", "learn-phase", Value::String("configure".to_string()));
                        rt.set_reactive("SEQ", "learn-error", Value::String(String::new()));
                    } else {
                        rt.set_reactive("SEQ", "learn-phase", Value::String("error".to_string()));
                        rt.set_reactive(
                            "SEQ",
                            "learn-error",
                            Value::String(format!("Learning job died without a terminal event (exit {code:?})")),
                        );
                    }
                }
                finished = true;
            }
        }
    }
    if disconnected {
        if !finished {
            let rt = editor.runtime_mut();
            rt.set_reactive("SEQ", "learn-phase", Value::String("error".to_string()));
            rt.set_reactive(
                "SEQ",
                "learn-error",
                Value::String("Learning job event stream disconnected".to_string()),
            );
            dirty = true;
            clear_preview = true;
        }
        finished = true;
    }
    if finished {
        sessions.pending_learn_job = None;
    }
    if clear_preview {
        clear_learn_param_preview(
            app,
            editor.runtime_mut(),
            &mut sessions.learn_param_preview,
            current_track,
        );
    }
    if dirty {
        editor.runtime_mut().run_reactive_cycle();
        editor.refresh_runtime_side_effects();
        editor.mark_needs_redraw();
    }
}

pub(crate) fn clear_learn_param_preview(
    app: &app::App,
    rt: &mut Runtime,
    preview: &mut Option<LearnParamPreview>,
    current_track: usize,
) -> bool {
    let Some(preview) = preview.take() else {
        return false;
    };
    for param_idx in preview.param_indices {
        if let Some(value) = app.effective_instrument_param_value(preview.track, param_idx) {
            app.send_instrument_param(preview.track, param_idx, value);
            if let Some(param) = app
                .graph
                .instrument_descriptors
                .get(preview.track)
                .and_then(|descriptor| descriptor.params.get(param_idx))
            {
                let value = Value::Number(f64::from(param.stored_to_user(value)));
                rt.set_reactive(
                    "SEQ",
                    &instrument_param_value_field(preview.track, param_idx, &param.name),
                    value.clone(),
                );
                if preview.track == current_track {
                    rt.set_reactive(
                        "SEQ",
                        &fx_instrument_param_value_field(param_idx, &param.name),
                        value,
                    );
                }
            }
        }
    }
    true
}

fn set_learn_param_preview(
    app: &app::App,
    rt: &mut Runtime,
    preview: &mut Option<LearnParamPreview>,
    track: usize,
    current_track: usize,
    params: &std::collections::BTreeMap<String, f64>,
) -> Result<(), String> {
    if preview.as_ref().is_some_and(|active| active.track != track) {
        clear_learn_param_preview(app, rt, preview, current_track);
    }
    let descriptor = app
        .graph
        .instrument_descriptors
        .get(track)
        .ok_or_else(|| "The edited track lost its instrument descriptor".to_string())?;
    let mut resolved = Vec::with_capacity(params.len());
    for (name, natural_value) in params {
        if !natural_value.is_finite() {
            return Err(format!("Trainer returned a non-finite value for {name}"));
        }
        let (param_idx, param) = descriptor
            .params
            .iter()
            .enumerate()
            .find(|(_, param)| param.name == *name)
            .ok_or_else(|| format!("Trainer returned unknown instrument parameter {name}"))?;
        let stored = param.clamp(param.user_input_to_stored(*natural_value as f32));
        resolved.push((param_idx, stored, param.name.clone()));
    }
    for (param_idx, stored, name) in &resolved {
        let param_idx = *param_idx;
        let stored = *stored;
        app.send_instrument_param(track, param_idx, stored);
        let display_value = descriptor.params[param_idx].stored_to_user(stored);
        let value = Value::Number(f64::from(display_value));
        rt.set_reactive(
            "SEQ",
            &instrument_param_value_field(track, param_idx, name),
            value.clone(),
        );
        if track == current_track {
            rt.set_reactive(
                "SEQ",
                &fx_instrument_param_value_field(param_idx, name),
                value,
            );
        }
    }
    let active = preview.get_or_insert_with(|| LearnParamPreview {
        track,
        param_indices: Vec::new(),
        stored_values: std::collections::BTreeMap::new(),
    });
    for (param_idx, stored, _) in resolved {
        if !active.param_indices.contains(&param_idx) {
            active.param_indices.push(param_idx);
        }
        active.stored_values.insert(param_idx, stored);
    }
    Ok(())
}

pub(crate) fn apply_learn_param_preview(
    app: &mut app::App,
    preview: &mut Option<LearnParamPreview>,
) -> Result<sequencer::app::edit::EditOutcome, String> {
    let active = preview
        .as_ref()
        .ok_or_else(|| "There is no learned result to apply".to_string())?;
    if active.stored_values.is_empty() {
        return Err("The learned result contains no parameter values".to_string());
    }
    let track = active.track;
    let values = active.stored_values.clone();
    let result = sequencer::app::edit::apply_recorded_instrument_values_mutation(
        app,
        track,
        "Apply Patch Learn result",
        move |app| {
            let slot = app
                .state
                .pattern
                .instrument_slots
                .get(track)
                .ok_or_else(|| "The learned instrument slot no longer exists".to_string())?;
            for (param_idx, value) in &values {
                slot.defaults.set(*param_idx, *value);
                app.send_instrument_param(track, *param_idx, *value);
            }
            if let Some(meta) = app
                .state
                .pattern
                .track_sound_state
                .lock()
                .unwrap()
                .get_mut(track)
            {
                meta.dirty = true;
            }
            Ok(())
        },
    )
    .map_err(|error| format!("Could not apply learned parameters: {error:?}"))?;
    *preview = None;
    Ok(result)
}

/// Drain status events eagerly, but stop after one visual progress unit.
/// Collapsing several epochs or optimizer iterations into one reactive cycle
/// makes training appear frozen even though the protocol is streaming.
fn take_learn_updates_through_epoch(
    receiver: &std::sync::mpsc::Receiver<sequencer::learn_job::LearnJobUpdate>,
) -> (Vec<sequencer::learn_job::LearnJobUpdate>, bool) {
    let mut updates = Vec::new();
    loop {
        match receiver.try_recv() {
            Ok(update) => {
                let is_progress = matches!(
                    &update,
                    sequencer::learn_job::LearnJobUpdate::Event(
                        sequencer::learn_job::LearnEvent::Epoch { .. }
                            | sequencer::learn_job::LearnEvent::OptimizationProgress { .. }
                    )
                );
                updates.push(update);
                if is_progress {
                    return (updates, false);
                }
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => return (updates, false),
            Err(std::sync::mpsc::TryRecvError::Disconnected) => return (updates, true),
        }
    }
}

fn number_list(values: &[f64]) -> Value {
    Value::List(
        values
            .iter()
            .map(|value| Rc::new(RefCell::new(Value::Number(*value))))
            .collect(),
    )
}

fn append_loss(losses: &mut Vec<f64>, loss: f64) -> Value {
    losses.push(loss);
    number_list(losses)
}

fn publish_event(rt: &mut Runtime, pending: &mut PendingLearnJob, event: &sequencer::learn_job::LearnEvent) {
    use sequencer::learn_job::LearnEvent;
    match event {
        LearnEvent::Plan {
            learnable,
            frozen,
            unsupported,
            seed_echo,
            pitch_hz,
            gate_frames,
            ..
        } => {
            if let Some(error) = seed_echo_error(&pending.expected_seed, seed_echo) {
                pending.saw_host_error = true;
                let _ = pending.job.cancel();
                rt.set_reactive("SEQ", "learn-phase", Value::String("error".to_string()));
                rt.set_reactive("SEQ", "learn-error", Value::String(error));
                return;
            }
            rt.set_reactive(
                "SEQ",
                "learn-phase",
                Value::String(if pending.kind == LearnLaunchKind::Plan { "configure" } else { "training" }.to_string()),
            );
            rt.set_reactive("SEQ", "learn-plan-params", plan_params_value(learnable, frozen, unsupported));
            rt.set_reactive("SEQ", "learn-pitch-hz", Value::Number(*pitch_hz));
            rt.set_reactive("SEQ", "learn-gate-frames", Value::Number(*gate_frames as f64));
        }
        LearnEvent::Stage { name, total } => {
            pending.losses.clear();
            rt.set_reactive("SEQ", "learn-phase", Value::String("training".to_string()));
            rt.set_reactive("SEQ", "learn-stage", Value::String(name.clone()));
            rt.set_reactive("SEQ", "learn-current-epoch", Value::Number(0.0));
            rt.set_reactive("SEQ", "learn-total-epochs", Value::Number(*total as f64));
            rt.set_reactive("SEQ", "learn-losses", Value::List(vec![]));
            rt.set_reactive("SEQ", "learn-optimization-losses", Value::List(vec![]));
        }
        LearnEvent::OptimizationProgress { current, total, losses } => {
            rt.set_reactive("SEQ", "learn-phase", Value::String("training".to_string()));
            rt.set_reactive("SEQ", "learn-current-epoch", Value::Number(*current as f64));
            rt.set_reactive("SEQ", "learn-total-epochs", Value::Number(*total as f64));
            rt.set_reactive("SEQ", "learn-optimization-losses", number_list(losses));
            if !losses.is_empty() {
                let loss = losses[0];
                rt.set_reactive("SEQ", "learn-loss", Value::Number(loss));
                rt.set_reactive(
                    "SEQ",
                    "learn-losses",
                    append_loss(&mut pending.losses, loss),
                );
            }
        }
        LearnEvent::Epoch { epoch, total, loss, params, steps } => {
            rt.set_reactive("SEQ", "learn-phase", Value::String("training".to_string()));
            rt.set_reactive("SEQ", "learn-current-epoch", Value::Number(*epoch as f64));
            rt.set_reactive("SEQ", "learn-total-epochs", Value::Number(*total as f64));
            rt.set_reactive("SEQ", "learn-loss", Value::Number(*loss));
            rt.set_reactive(
                "SEQ",
                "learn-losses",
                append_loss(&mut pending.losses, *loss),
            );
            rt.set_reactive(
                "SEQ",
                "learn-epoch-params",
                epoch_params_value(&pending.expected_seed, params, steps),
            );
        }
        LearnEvent::Checkpoint { wav, .. } => {
            rt.set_reactive("SEQ", "learn-checkpoint-wav", Value::String(wav.to_string_lossy().into_owned()));
        }
        LearnEvent::Result { improvement_pct, abs_distance, basin_check, deltas, seeded_wav, final_wav, .. } => {
            rt.set_reactive("SEQ", "learn-phase", Value::String("result".to_string()));
            rt.set_reactive("SEQ", "learn-improvement-pct", Value::Number(*improvement_pct));
            rt.set_reactive("SEQ", "learn-abs-distance", Value::Number(*abs_distance));
            rt.set_reactive("SEQ", "learn-basin-check", Value::String(basin_check.clone()));
            rt.set_reactive("SEQ", "learn-result-deltas", deltas_value(deltas));
            rt.set_reactive(
                "SEQ",
                "learn-seeded-wav",
                Value::String(
                    seeded_wav
                        .as_ref()
                        .map(|path| path.to_string_lossy().into_owned())
                        .unwrap_or_default(),
                ),
            );
            rt.set_reactive("SEQ", "learn-final-wav", Value::String(final_wav.to_string_lossy().into_owned()));
            rt.set_reactive("SEQ", "learn-applied", Value::Bool(false));
        }
        LearnEvent::Error { message } => {
            rt.set_reactive("SEQ", "learn-phase", Value::String("error".to_string()));
            rt.set_reactive("SEQ", "learn-error", Value::String(message.clone()));
        }
        LearnEvent::Unknown(_) => {}
    }
}

fn seed_echo_error(
    expected: &std::collections::BTreeMap<String, f64>,
    actual: &std::collections::BTreeMap<String, f64>,
) -> Option<String> {
    let mut mismatches = Vec::new();
    for (name, expected_value) in expected {
        match actual.get(name) {
            Some(actual_value)
                if (expected_value - actual_value).abs()
                    <= 1e-9 * expected_value.abs().max(actual_value.abs()).max(1.0) => {}
            Some(actual_value) => mismatches.push(format!(
                "{name}: sent {expected_value}, trainer read {actual_value}"
            )),
            None => mismatches.push(format!("{name}: missing from trainer echo")),
        }
    }
    for name in actual.keys() {
        if !expected.contains_key(name) {
            mismatches.push(format!("{name}: unexpected in trainer echo"));
        }
    }
    if mismatches.is_empty() {
        None
    } else {
        Some(format!(
            "Trainer seed mismatch; learning stopped to avoid using the wrong knob units: {}",
            mismatches.join("; ")
        ))
    }
}

fn plan_params_value(
    learnable: &[String],
    frozen: &[sequencer::learn_job::FrozenParam],
    unsupported: &[serde_json::Value],
) -> Value {
    let mut rows = learnable
        .iter()
        .map(|name| values::map_value([
            ("name", Value::String(name.clone())),
            ("status", Value::String("learnable".to_string())),
            ("reason", Value::String(String::new())),
        ]))
        .collect::<Vec<_>>();
    rows.extend(frozen.iter().map(|param| values::map_value([
        ("name", Value::String(param.name.clone())),
        ("status", Value::String("frozen".to_string())),
        ("reason", Value::String(param.reason.clone())),
    ])));
    rows.extend(unsupported.iter().map(|entry| {
        let name = entry.get("name").and_then(serde_json::Value::as_str)
            .or_else(|| entry.as_str()).unwrap_or("unsupported");
        let reason = entry.get("reason").and_then(serde_json::Value::as_str).unwrap_or("unsupported signal path");
        values::map_value([
            ("name", Value::String(name.to_string())),
            ("status", Value::String("unsupported".to_string())),
            ("reason", Value::String(reason.to_string())),
        ])
    }));
    Value::List(rows.into_iter().map(|value| Rc::new(RefCell::new(value))).collect())
}

fn epoch_params_value(
    seed: &std::collections::BTreeMap<String, f64>,
    params: &std::collections::BTreeMap<String, f64>,
    steps: &std::collections::BTreeMap<String, f64>,
) -> Value {
    Value::List(params.iter().map(|(name, value)| Rc::new(RefCell::new(values::map_value([
        ("name", Value::String(name.clone())),
        ("from", Value::Number(seed.get(name).copied().unwrap_or(*value))),
        ("value", Value::Number(*value)),
        ("change", Value::Number(*value - seed.get(name).copied().unwrap_or(*value))),
        ("step", Value::Number(steps.get(name).copied().unwrap_or(0.0))),
    ])))).collect())
}

fn deltas_value(deltas: &std::collections::BTreeMap<String, sequencer::learn_job::ParamDelta>) -> Value {
    Value::List(deltas.iter().map(|(name, delta)| Rc::new(RefCell::new(values::map_value([
        ("name", Value::String(name.clone())),
        ("from", Value::Number(delta.from)),
        ("to", Value::Number(delta.to)),
        ("change", Value::Number(delta.to - delta.from)),
    ])))).collect())
}

#[cfg(test)]
mod tests {
    use super::{
        append_loss, apply_learn_param_preview, clear_learn_param_preview, seed_echo_error,
        set_learn_param_preview,
        take_learn_updates_through_epoch,
    };
    use crate::{
        app, fx_instrument_param_value_field, instrument_param_value_field, Runtime, Value,
    };
    use sequencer::learn_job::{LearnEvent, LearnJobUpdate};
    use std::collections::BTreeMap;

    fn test_instrument_app(
        descriptor: sequencer::effects::EffectDescriptor,
    ) -> sequencer::app::App {
        let state = std::sync::Arc::new(sequencer::sequencer::SequencerState::new(
            1,
            vec![vec![]],
        ));
        state.pattern.instrument_slots[0].apply_descriptor(&descriptor, 0);
        let (keyboard_tx, _keyboard_rx) = std::sync::mpsc::channel();
        let mut app = app::App::new(
            state,
            sequencer::audiograph::LiveGraphPtr(std::ptr::null_mut()),
            44_100,
            app::AudioBuses {
                bus_l_id: 0,
                bus_r_id: 0,
                default_bus_nodes: Vec::new(),
                bus_effect_runtime: std::sync::Arc::new(std::sync::Mutex::new(
                    std::sync::Arc::new(Vec::new()),
                )),
                reverb_bus_id: 0,
                reverb_node_id: 0,
            },
            std::sync::Arc::new(sequencer::recorder::MasterRecorder::new(44_100, 2)),
            keyboard_tx,
        );
        app.tracks = vec!["Track 1".to_string()];
        app.track_registry =
            sequencer::sequencer::TrackRegistry::for_legacy_track_count(1).unwrap();
        app.graph.instrument_descriptors = vec![descriptor];
        app
    }

    fn reactive_number(runtime: &Runtime, field: &str) -> f64 {
        let Value::Map(seq) = runtime.global_value("SEQ").expect("SEQ namespace") else {
            panic!("SEQ should be a map");
        };
        let value = seq
            .get(field)
            .unwrap_or_else(|| panic!("missing reactive field {field}"))
            .borrow()
            .clone();
        let Value::Number(number) = value else {
            panic!("reactive field {field} should be numeric");
        };
        number
    }

    #[test]
    fn seed_echo_validation_accepts_json_rounding_but_rejects_unit_drift() {
        let expected = BTreeMap::from([
            ("cutoff".to_string(), 1_234.5),
            ("resonance".to_string(), 0.2),
        ]);
        let rounded = BTreeMap::from([
            ("cutoff".to_string(), 1_234.500_000_1),
            ("resonance".to_string(), 0.2),
        ]);
        assert_eq!(seed_echo_error(&expected, &rounded), None);

        let wrong_units = BTreeMap::from([
            ("cutoff".to_string(), 0.5),
            ("resonance".to_string(), 0.2),
        ]);
        let error = seed_echo_error(&expected, &wrong_units).expect("unit mismatch");
        assert!(error.contains("cutoff: sent 1234.5, trainer read 0.5"));
    }

    #[test]
    fn learn_update_batches_present_each_epoch_separately() {
        let (sender, receiver) = std::sync::mpsc::channel();
        sender
            .send(LearnJobUpdate::Event(LearnEvent::Stage {
                name: "fit".to_string(),
                total: 2,
            }))
            .unwrap();
        for epoch in 1..=2 {
            sender
                .send(LearnJobUpdate::Event(LearnEvent::Epoch {
                    epoch,
                    total: 2,
                    loss: 1.0 / epoch as f64,
                    params: BTreeMap::new(),
                    steps: BTreeMap::new(),
                }))
                .unwrap();
        }
        sender
            .send(LearnJobUpdate::Exited {
                success: true,
                code: Some(0),
            })
            .unwrap();
        drop(sender);

        let (first, disconnected) = take_learn_updates_through_epoch(&receiver);
        assert!(!disconnected);
        assert_eq!(first.len(), 2);
        assert!(matches!(
            first.last(),
            Some(LearnJobUpdate::Event(LearnEvent::Epoch { epoch: 1, .. }))
        ));

        let (second, disconnected) = take_learn_updates_through_epoch(&receiver);
        assert!(!disconnected);
        assert_eq!(second.len(), 1);
        assert!(matches!(
            second.last(),
            Some(LearnJobUpdate::Event(LearnEvent::Epoch { epoch: 2, .. }))
        ));

        let (terminal, disconnected) = take_learn_updates_through_epoch(&receiver);
        assert!(disconnected);
        assert!(matches!(terminal.as_slice(), [LearnJobUpdate::Exited { success: true, .. }]));
    }

    #[test]
    fn learn_update_batches_present_each_optimizer_iteration_separately() {
        let (sender, receiver) = std::sync::mpsc::channel();
        for current in 1..=2 {
            sender
                .send(LearnJobUpdate::Event(LearnEvent::OptimizationProgress {
                    current,
                    total: 2,
                    losses: vec![1.0 / current as f64],
                }))
                .unwrap();
        }
        drop(sender);

        let (first, disconnected) = take_learn_updates_through_epoch(&receiver);
        assert!(!disconnected);
        assert!(matches!(
            first.as_slice(),
            [LearnJobUpdate::Event(LearnEvent::OptimizationProgress { current: 1, .. })]
        ));
        let (second, disconnected) = take_learn_updates_through_epoch(&receiver);
        assert!(!disconnected);
        assert!(matches!(
            second.as_slice(),
            [LearnJobUpdate::Event(LearnEvent::OptimizationProgress { current: 2, .. })]
        ));
        assert!(take_learn_updates_through_epoch(&receiver).1);
    }

    #[test]
    fn loss_trajectory_retains_every_epoch() {
        let mut losses = Vec::new();
        let mut published = Value::Nil;
        for epoch in 0..250 {
            published = append_loss(&mut losses, 1.0 / (epoch + 1) as f64);
        }
        assert_eq!(losses.len(), 250);
        let Value::List(values) = published else {
            panic!("loss trajectory should be a list");
        };
        assert_eq!(values.len(), 250);
        assert_eq!(*values[0].borrow(), Value::Number(1.0));
        assert_eq!(*values[249].borrow(), Value::Number(1.0 / 250.0));
    }

    #[test]
    fn learn_preview_updates_knob_field_and_clear_restores_document_value() {
        let descriptor = sequencer::effects::EffectDescriptor::builtin_filter();
        let cutoff_idx = descriptor
            .params
            .iter()
            .position(|param| param.name == "cutoff")
            .expect("filter descriptor should include cutoff");
        let cutoff = &descriptor.params[cutoff_idx];
        let app = test_instrument_app(descriptor.clone());
        let seed_stored = cutoff.user_input_to_stored(520.0);
        app.state.pattern.instrument_slots[0]
            .defaults
            .set(cutoff_idx, seed_stored);
        let field = instrument_param_value_field(0, cutoff_idx, &cutoff.name);
        let fx_field = fx_instrument_param_value_field(cutoff_idx, &cutoff.name);
        let mut runtime = Runtime::new();
        let mut preview = None;

        set_learn_param_preview(
            &app,
            &mut runtime,
            &mut preview,
            0,
            0,
            &BTreeMap::from([("cutoff".to_string(), 1125.0)]),
        )
        .unwrap();
        assert!((reactive_number(&runtime, &field) - 1125.0).abs() < 0.01);
        assert!((reactive_number(&runtime, &fx_field) - 1125.0).abs() < 0.01);
        assert!((cutoff.stored_to_user(app.state.pattern.instrument_slots[0].defaults.get(cutoff_idx))
            - 520.0)
            .abs()
            < 0.01, "preview must not mutate the saved instrument default");

        assert!(clear_learn_param_preview(&app, &mut runtime, &mut preview, 0));
        assert!((reactive_number(&runtime, &field) - 520.0).abs() < 0.01);
        assert!((reactive_number(&runtime, &fx_field) - 520.0).abs() < 0.01);

        runtime.set_reactive("SEQ", &fx_field, Value::Number(777.0));
        set_learn_param_preview(
            &app,
            &mut runtime,
            &mut preview,
            0,
            1,
            &BTreeMap::from([("cutoff".to_string(), 1125.0)]),
        )
        .unwrap();
        assert_eq!(reactive_number(&runtime, &fx_field), 777.0);
        assert!(clear_learn_param_preview(&app, &mut runtime, &mut preview, 1));
        assert_eq!(reactive_number(&runtime, &fx_field), 777.0);
    }

    #[test]
    fn applying_learn_preview_is_one_undoable_instrument_value_edit() {
        let descriptor = sequencer::effects::EffectDescriptor::builtin_filter();
        let cutoff_idx = descriptor
            .params
            .iter()
            .position(|param| param.name == "cutoff")
            .expect("filter descriptor should include cutoff");
        let cutoff = descriptor.params[cutoff_idx].clone();
        let mut app = test_instrument_app(descriptor);
        app.state.pattern.instrument_slots[0]
            .defaults
            .set(cutoff_idx, cutoff.user_input_to_stored(520.0));
        let mut runtime = Runtime::new();
        let mut preview = None;
        set_learn_param_preview(
            &app,
            &mut runtime,
            &mut preview,
            0,
            0,
            &BTreeMap::from([("cutoff".to_string(), 1125.0)]),
        )
        .unwrap();

        apply_learn_param_preview(&mut app, &mut preview).unwrap();
        assert!(preview.is_none());
        assert_eq!(app.history.undo_len(), 1);
        assert!((cutoff.stored_to_user(
            app.state.pattern.instrument_slots[0].defaults.get(cutoff_idx),
        ) - 1125.0)
            .abs()
            < 0.01);

        assert!(matches!(
            sequencer::app::edit::undo(&mut app),
            sequencer::app::history::HistoryReplay::Applied(_)
        ));
        assert!((cutoff.stored_to_user(
            app.state.pattern.instrument_slots[0].defaults.get(cutoff_idx),
        ) - 520.0)
            .abs()
            < 0.01);
    }
}
