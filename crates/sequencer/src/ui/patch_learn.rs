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
    epochs: u64,
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
    let spec = sequencer::learn_job::LearnJobSpec {
        patch_path: session.path.clone(),
        patch_source: session.last_valid_source.clone(),
        target_path,
        seed,
        epochs,
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
    rt.set_reactive("SEQ", "learn-epoch-params", Value::List(vec![]));
    rt.set_reactive("SEQ", "learn-checkpoint-wav", Value::String(String::new()));
    rt.set_reactive("SEQ", "learn-improvement-pct", Value::Number(0.0));
    rt.set_reactive("SEQ", "learn-abs-distance", Value::Number(0.0));
    rt.set_reactive("SEQ", "learn-basin-check", Value::String(String::new()));
    rt.set_reactive("SEQ", "learn-result-deltas", Value::List(vec![]));
    rt.set_reactive("SEQ", "learn-final-wav", Value::String(String::new()));
    rt.set_reactive("SEQ", "learn-error", Value::String(String::new()));
}

pub(crate) fn poll_learn_job(sessions: &mut EditSessionState, editor: &mut Editor) {
    if sessions.instrument_edit_session.is_none() {
        if let Some(pending) = sessions.pending_learn_job.take() {
            let _ = pending.job.cancel();
        }
        return;
    }
    let mut finished = false;
    let mut dirty = false;
    let Some(pending) = sessions.pending_learn_job.as_mut() else {
        return;
    };
    loop {
        match pending.job.receiver.try_recv() {
            Ok(update) => {
                dirty = true;
                let rt = editor.runtime_mut();
                match update {
                    sequencer::learn_job::LearnJobUpdate::Event(event) => {
                        pending.saw_terminal |= event.is_terminal();
                        pending.saw_plan |= matches!(event, sequencer::learn_job::LearnEvent::Plan { .. });
                        publish_event(rt, pending, &event);
                    }
                    sequencer::learn_job::LearnJobUpdate::ProtocolError { error, .. }
                    | sequencer::learn_job::LearnJobUpdate::IoError(error) => {
                        pending.saw_host_error = true;
                        rt.set_reactive("SEQ", "learn-phase", Value::String("error".to_string()));
                        rt.set_reactive("SEQ", "learn-error", Value::String(error));
                    }
                    sequencer::learn_job::LearnJobUpdate::Exited { success, code } => {
                        if !pending.saw_terminal && !pending.saw_host_error {
                            if pending.kind == LearnLaunchKind::Plan && success && pending.saw_plan {
                                rt.set_reactive("SEQ", "learn-phase", Value::String("configure".to_string()));
                            } else if pending.cancel_requested {
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
            Err(std::sync::mpsc::TryRecvError::Empty) => break,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                if !finished {
                    let rt = editor.runtime_mut();
                    rt.set_reactive("SEQ", "learn-phase", Value::String("error".to_string()));
                    rt.set_reactive(
                        "SEQ",
                        "learn-error",
                        Value::String("Learning job event stream disconnected".to_string()),
                    );
                    dirty = true;
                }
                finished = true;
                break;
            }
        }
    }
    if finished {
        sessions.pending_learn_job = None;
    }
    if dirty {
        editor.runtime_mut().run_reactive_cycle();
        editor.refresh_runtime_side_effects();
        editor.mark_needs_redraw();
    }
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
        }
        LearnEvent::Epoch { epoch, total, loss, params, steps } => {
            pending.losses.push(*loss);
            if pending.losses.len() > 180 {
                pending.losses.remove(0);
            }
            rt.set_reactive("SEQ", "learn-phase", Value::String("training".to_string()));
            rt.set_reactive("SEQ", "learn-current-epoch", Value::Number(*epoch as f64));
            rt.set_reactive("SEQ", "learn-total-epochs", Value::Number(*total as f64));
            rt.set_reactive("SEQ", "learn-loss", Value::Number(*loss));
            rt.set_reactive(
                "SEQ",
                "learn-losses",
                Value::List(pending.losses.iter().map(|loss| Rc::new(RefCell::new(Value::Number(*loss)))).collect()),
            );
            rt.set_reactive("SEQ", "learn-epoch-params", epoch_params_value(params, steps));
        }
        LearnEvent::Checkpoint { wav, .. } => {
            rt.set_reactive("SEQ", "learn-checkpoint-wav", Value::String(wav.to_string_lossy().into_owned()));
        }
        LearnEvent::Result { improvement_pct, abs_distance, basin_check, deltas, final_wav, .. } => {
            rt.set_reactive("SEQ", "learn-phase", Value::String("result".to_string()));
            rt.set_reactive("SEQ", "learn-improvement-pct", Value::Number(*improvement_pct));
            rt.set_reactive("SEQ", "learn-abs-distance", Value::Number(*abs_distance));
            rt.set_reactive("SEQ", "learn-basin-check", Value::String(basin_check.clone()));
            rt.set_reactive("SEQ", "learn-result-deltas", deltas_value(deltas));
            rt.set_reactive("SEQ", "learn-final-wav", Value::String(final_wav.to_string_lossy().into_owned()));
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
    params: &std::collections::BTreeMap<String, f64>,
    steps: &std::collections::BTreeMap<String, f64>,
) -> Value {
    Value::List(params.iter().map(|(name, value)| Rc::new(RefCell::new(values::map_value([
        ("name", Value::String(name.clone())),
        ("value", Value::Number(*value)),
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
    use super::seed_echo_error;
    use std::collections::BTreeMap;

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
}
