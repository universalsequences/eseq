use super::*;

pub(super) fn map_number(
    map: &std::collections::HashMap<String, Rc<RefCell<Value>>>,
    key: &str,
) -> Option<f64> {
    map.get(key).and_then(|cell| match &*cell.borrow() {
        Value::Number(value) => Some(*value),
        _ => None,
    })
}

pub(super) fn map_number_or_bool(
    map: &std::collections::HashMap<String, Rc<RefCell<Value>>>,
    key: &str,
) -> Option<f64> {
    map.get(key).and_then(|cell| match &*cell.borrow() {
        Value::Number(value) => Some(*value),
        Value::Bool(value) => Some(if *value { 1.0 } else { 0.0 }),
        _ => None,
    })
}

pub(super) fn map_string(
    map: &std::collections::HashMap<String, Rc<RefCell<Value>>>,
    key: &str,
) -> Option<String> {
    map.get(key).and_then(|cell| match &*cell.borrow() {
        Value::String(value) | Value::Symbol(value) | Value::Keyword(value) => {
            Some(value.trim_start_matches(':').to_string())
        }
        _ => None,
    })
}

pub(super) fn map_bool(map: &std::collections::HashMap<String, Rc<RefCell<Value>>>, key: &str) -> bool {
    map.get(key)
        .and_then(|cell| match &*cell.borrow() {
            Value::Bool(value) => Some(*value),
            _ => None,
        })
        .unwrap_or(false)
}

pub(super) fn map_usize(
    map: &std::collections::HashMap<String, Rc<RefCell<Value>>>,
    key: &str,
) -> Option<usize> {
    map_number(map, key).and_then(|value| {
        (value.is_finite() && value >= 0.0 && value <= usize::MAX as f64).then_some(value as usize)
    })
}

pub(super) fn map_param_updates(
    map: &std::collections::HashMap<String, Rc<RefCell<Value>>>,
) -> Option<Vec<(usize, f32)>> {
    let updates = map.get("updates")?;
    let updates = updates.borrow();
    let Value::List(items) = &*updates else {
        return None;
    };
    let parsed = items
        .iter()
        .map(|item| {
            let item = item.borrow();
            let Value::Map(update) = &*item else {
                return None;
            };
            Some((map_usize(update, "param-idx")?, map_number(update, "value")? as f32))
        })
        .collect::<Option<Vec<_>>>()?;
    (!parsed.is_empty()).then_some(parsed)
}

pub(super) fn apply_toggle_step_host_command(
    app: &mut app::App,
    payload: &Value,
) -> Result<(app::edit::EditOutcome, usize, usize), String> {
    let Value::Map(map) = payload else {
        return Err("step toggle payload was invalid".to_string());
    };
    let track = map_usize(map, "track")
        .ok_or_else(|| "step toggle track was invalid".to_string())?;
    let step =
        map_usize(map, "step").ok_or_else(|| "step toggle index was invalid".to_string())?;
    app::try_apply_command(app, app::AppCommand::ToggleStep { track, step })
        .map(|outcome| (outcome, track, step))
        .map_err(|error| format!("could not toggle step: {error:?}"))
}

pub(super) fn apply_selected_steps_delete(
    app: &mut app::App,
    track: usize,
    selected_steps: &Arc<Mutex<HashSet<usize>>>,
) -> Result<(app::edit::EditOutcome, Vec<usize>), String> {
    let mut steps = selected_steps
        .lock()
        .unwrap()
        .iter()
        .copied()
        .collect::<Vec<_>>();
    steps.sort_unstable();
    let outcome = app::try_apply_command(
        app,
        app::AppCommand::ClearSteps {
            track,
            steps: steps.clone(),
        },
    )
    .map_err(|error| format!("could not delete selected steps: {error:?}"))?;
    if matches!(outcome, app::edit::EditOutcome::Applied(_)) {
        selected_steps.lock().unwrap().clear();
    }
    Ok((outcome, steps))
}

pub(super) fn apply_step_paste_host_command(
    app: &mut app::App,
    clipboard: &Arc<Mutex<Option<(usize, Vec<(usize, sequencer::sequencer::StepSnapshot)>)>>>,
    payload: &Value,
) -> Result<(app::edit::EditOutcome, usize), String> {
    let Value::Map(map) = payload else {
        return Err("step paste payload was invalid".to_string());
    };
    let track = map_usize(map, "track")
        .ok_or_else(|| "step paste track was invalid".to_string())?;
    let dest_start = map_usize(map, "dest-start")
        .ok_or_else(|| "step paste destination was invalid".to_string())?;
    let Some((source_track, clipboard)) = clipboard.lock().unwrap().clone() else {
        return Ok((app::edit::EditOutcome::NoOp, track));
    };
    let num_steps = app
        .state
        .pattern
        .track_params
        .get(track)
        .ok_or_else(|| format!("step paste track {track} was out of range"))?
        .get_num_steps();
    app::try_apply_command(
        app,
        app::AppCommand::PasteSteps {
            track,
            source_track,
            clipboard,
            dest_start,
            num_steps,
        },
    )
    .map(|outcome| (outcome, track))
    .map_err(|error| format!("could not paste steps: {error:?}"))
}

pub(super) fn step_param_from_name(name: &str) -> Option<StepParam> {
    match name.trim_start_matches(':') {
        "velocity" | "vel" => Some(StepParam::Velocity),
        "duration" | "dur" => Some(StepParam::Duration),
        "aux-a" | "aux_a" | "auxa" | "axa" => Some(StepParam::AuxA),
        "aux-b" | "aux_b" | "auxb" => Some(StepParam::AuxB),
        "transpose" => Some(StepParam::Transpose),
        "pan" => Some(StepParam::Pan),
        "sync" | "syn" => Some(StepParam::Sync),
        "delay" | "dly" => Some(StepParam::Delay),
        "speed" => Some(StepParam::Speed),
        "chop" => Some(StepParam::Chop),
        _ => None,
    }
}

pub(super) fn apply_step_param_history_host_command(
    app: &mut app::App,
    payload: &Value,
) -> Result<(app::edit::EditOutcome, usize, Vec<usize>, StepParam), String> {
    let Value::Map(map) = payload else {
        return Err("step parameter payload was invalid".to_string());
    };
    let track = map_usize(map, "track")
        .ok_or_else(|| "step parameter track was invalid".to_string())?;
    let param = map_string(map, "param")
        .and_then(|name| step_param_from_name(&name))
        .ok_or_else(|| "step parameter name was invalid".to_string())?;
    let value = map_number(map, "value")
        .filter(|value| value.is_finite())
        .ok_or_else(|| "step parameter value was invalid".to_string())?
        as f32;
    let steps = map_usize_list(map, "steps")
        .ok_or_else(|| "step parameter targets were invalid".to_string())?;
    let outcome = app::edit::apply_recorded_step_mutation(
        app,
        track,
        &steps,
        "Set step parameter",
        |app| {
            for step in &steps {
                app.state.pattern.step_data[track].set(*step, param, value);
            }
            Ok(())
        },
    )
    .map_err(|error| format!("could not set step parameter: {error:?}"))?;
    Ok((outcome, track, steps, param))
}

pub(super) fn apply_move_step_history_host_command(
    app: &mut app::App,
    payload: &Value,
) -> Result<(app::edit::EditOutcome, usize, Vec<usize>, Vec<usize>, isize, bool), String> {
    let Value::Map(map) = payload else {
        return Err("step move payload was invalid".to_string());
    };
    let track = map_usize(map, "track")
        .ok_or_else(|| "step move track was invalid".to_string())?;
    let steps = map_usize_list(map, "steps")
        .ok_or_else(|| "step move targets were invalid".to_string())?;
    let delta = map_number(map, "delta")
        .filter(|value| value.is_finite())
        .ok_or_else(|| "step move delta was invalid".to_string())?
        .round() as isize;
    let move_selection = map_bool(map, "move-selection");
    let num_steps = app
        .state
        .pattern
        .track_params
        .get(track)
        .ok_or_else(|| format!("step move track {track} was out of range"))?
        .get_num_steps()
        .min(MAX_STEPS);
    if steps.is_empty()
        || delta == 0
        || steps.iter().any(|step| {
            *step >= num_steps
                || (*step as isize + delta) < 0
                || (*step as isize + delta) >= num_steps as isize
        })
    {
        return Err("step move range was invalid".to_string());
    }
    let sources = steps
        .iter()
        .map(|step| (*step, app.state.capture_step_snapshot(track, *step)))
        .collect::<Vec<_>>();
    let mut affected = Vec::new();
    for (step, snapshot) in &sources {
        let duration_cells = if snapshot.active {
            snapshot.params[StepParam::Duration as usize]
                .max(1.0)
                .ceil() as usize
        } else {
            1
        };
        let destination = (*step as isize + delta) as usize;
        for base in [*step, destination] {
            affected.extend(base..base.saturating_add(duration_cells).min(num_steps));
        }
    }
    affected.sort_unstable();
    affected.dedup();
    let outcome = app::edit::apply_recorded_step_mutation(
        app,
        track,
        &affected,
        "Move steps",
        |app| {
            for (step, _) in &sources {
                app.state.clear_step_payload_no_publish(track, *step);
            }
            for (step, snapshot) in &sources {
                app.state.restore_step_snapshot_no_publish(
                    track,
                    (*step as isize + delta) as usize,
                    snapshot,
                );
            }
            Ok(())
        },
    )
    .map_err(|error| format!("could not move steps: {error:?}"))?;
    Ok((outcome, track, steps, affected, delta, move_selection))
}

pub(super) fn apply_slice2_history_host_command(
    app: &mut app::App,
    payload: &Value,
) -> Result<(app::edit::EditOutcome, usize), String> {
    let Value::Map(map) = payload else {
        return Err("Slice 2 edit payload was invalid".to_string());
    };
    let op = map_string(map, "op")
        .ok_or_else(|| "Slice 2 edit operation was missing".to_string())?;
    let track = map_usize(map, "track")
        .ok_or_else(|| "Slice 2 edit track was invalid".to_string())?;
    let command = match op.as_str() {
        "duplicate" => app::AppCommand::DuplicateTrackPattern { track },
        "halve" => app::AppCommand::HalveTrackPattern { track },
        "set-length" => app::AppCommand::SetTrackNumSteps {
            track,
            n: map_usize(map, "value")
                .ok_or_else(|| "track pattern length was invalid".to_string())?,
        },
        "timebase-plock" => app::AppCommand::SetTimebasePlockMulti {
            track,
            steps: map_usize_list(map, "steps")
                .ok_or_else(|| "timebase p-lock steps were invalid".to_string())?,
            timebase: Timebase::from_index(
                map_usize(map, "value")
                    .ok_or_else(|| "timebase p-lock value was invalid".to_string())?
                    as u32,
            ),
        },
        "swing-plock" => app::AppCommand::SetTrackSwingPlockMulti {
            track,
            steps: map_usize_list(map, "steps")
                .ok_or_else(|| "swing p-lock steps were invalid".to_string())?,
            value: map_number(map, "value")
                .filter(|value| value.is_finite())
                .ok_or_else(|| "swing p-lock value was invalid".to_string())?
                as f32,
        },
        "swing-resolution-plock" => app::AppCommand::SetTrackSwingResolutionPlockMulti {
            track,
            steps: map_usize_list(map, "steps")
                .ok_or_else(|| "swing-resolution p-lock steps were invalid".to_string())?,
            resolution: SwingResolution::from_index(
                map_usize(map, "value")
                    .ok_or_else(|| "swing-resolution p-lock value was invalid".to_string())?
                    as u32,
            ),
        },
        _ => return Err(format!("unknown Slice 2 edit operation {op}")),
    };
    app::try_apply_command(app, command)
        .map(|outcome| (outcome, track))
        .map_err(|error| format!("could not apply Slice 2 edit: {error:?}"))
}

pub(super) fn apply_slice3_history_host_command(
    app: &mut app::App,
    payload: &Value,
) -> Result<(app::edit::EditOutcome, Option<usize>), String> {
    let Value::Map(map) = payload else {
        return Err("Slice 3 edit payload was invalid".to_string());
    };
    let op = map_string(map, "op")
        .ok_or_else(|| "Slice 3 edit operation was missing".to_string())?;
    let value = || {
        map_number(map, "value")
            .filter(|value| value.is_finite())
            .ok_or_else(|| "Slice 3 edit value was invalid".to_string())
    };
    let track = map_usize(map, "track");
    let track_required = || track.ok_or_else(|| "Slice 3 edit track was invalid".to_string());
    let command = match op.as_str() {
        "volume" => app::AppCommand::SetTrackVolume {
            track: track_required()?,
            value: value()? as f32,
        },
        "pan" => app::AppCommand::SetTrackPan {
            track: track_required()?,
            value: value()? as f32,
        },
        "send" => app::AppCommand::SetTrackSend {
            track: track_required()?,
            value: value()? as f32,
        },
        "attack" => app::AppCommand::SetTrackAttack {
            track: track_required()?,
            ms: value()? as f32,
        },
        "release" => app::AppCommand::SetTrackRelease {
            track: track_required()?,
            ms: value()? as f32,
        },
        "swing" => app::AppCommand::SetTrackSwing {
            track: track_required()?,
            value: value()? as f32,
        },
        "toggle-gate" => app::AppCommand::ToggleTrackGate {
            track: track_required()?,
        },
        "toggle-poly" => app::AppCommand::ToggleTrackPolyphonic {
            track: track_required()?,
        },
        "toggle-mute" => app::AppCommand::ToggleTrackMute {
            track: track_required()?,
        },
        "toggle-solo" => app::AppCommand::ToggleTrackSolo {
            track: track_required()?,
        },
        "max-polyphony" => app::AppCommand::SetTrackMaxPolyphony {
            track: track_required()?,
            value: value()?.round().max(1.0) as usize,
        },
        "swing-resolution" => app::AppCommand::SetTrackSwingResolution {
            track: track_required()?,
            resolution: SwingResolution::from_index(value()? as u32),
        },
        "timebase" => app::AppCommand::SetTrackTimebase {
            track: track_required()?,
            timebase: Timebase::from_index(value()? as u32),
        },
        "fts" => app::AppCommand::SetTrackFtsScale {
            track: track_required()?,
            scale_idx: value()? as usize,
        },
        "accumulator" => app::AppCommand::SetTrackAccumIdx {
            track: track_required()?,
            idx: value()? as usize,
            default_limit: map_number(map, "default-limit").map(|value| value as f32),
            script_name: map_string(map, "script-name"),
        },
        "accum-limit" => app::AppCommand::SetTrackAccumLimit {
            track: track_required()?,
            value: value()? as f32,
        },
        "accum-mode" => app::AppCommand::SetTrackAccumMode {
            track: track_required()?,
            mode: value()? as u32,
        },
        "mute-group" => app::AppCommand::SetTrackMuteGroup {
            track: track_required()?,
            group: value()?.round().clamp(0.0, 8.0) as u8,
        },
        "global-transpose" => app::AppCommand::SetTrackGlobalTranspose {
            track: track_required()?,
            enabled: value()? != 0.0,
        },
        "base-note" => app::AppCommand::SetInstrumentBaseNoteOffset {
            track: track_required()?,
            value: value()? as f32,
        },
        "master-volume" => app::AppCommand::SetMasterVolume {
            value: value()? as f32,
        },
        "bpm" => app::AppCommand::SetBpm {
            bpm: value()?.round() as u32,
        },
        _ => return Err(format!("unknown Slice 3 edit operation {op}")),
    };
    app::try_apply_command(app, command)
        .map(|outcome| (outcome, track))
        .map_err(|error| format!("could not apply Slice 3 edit: {error:?}"))
}

/// Mixer-strip ops stay on the targeted per-track invalidation (Mute/Solo
/// already fan the effective-mute/color fields out to every track);
/// everything else falls back to the full whole-track + ui-epoch resync.
pub(super) fn slice3_track_mixer_invalidation(payload: &Value) -> Option<TrackMixerInvalidation> {
    let Value::Map(map) = payload else {
        return None;
    };
    match map_string(map, "op")?.as_str() {
        "volume" => Some(TrackMixerInvalidation::Volume),
        "pan" => Some(TrackMixerInvalidation::Pan),
        "toggle-mute" => Some(TrackMixerInvalidation::Mute),
        "toggle-solo" => Some(TrackMixerInvalidation::Solo),
        _ => None,
    }
}

/// Bus-fader drags likewise skip the ui-epoch resync; discrete bus ops
/// (mute/solo toggles) keep it.
pub(super) fn bus_mixer_targeted_invalidation(payload: &Value) -> Option<BusMixerInvalidation> {
    let Value::Map(map) = payload else {
        return None;
    };
    match map_string(map, "op")?.as_str() {
        "volume" => Some(BusMixerInvalidation::Volume),
        _ => None,
    }
}

pub(super) fn apply_bus_mixer_history_host_command(
    app: &mut app::App,
    payload: &Value,
) -> Result<(app::edit::EditOutcome, usize), String> {
    let Value::Map(map) = payload else {
        return Err("Bus mixer edit payload was invalid".to_string());
    };
    let op = map_string(map, "op")
        .ok_or_else(|| "Bus mixer edit operation was missing".to_string())?;
    let requested_bus_idx = map_usize(map, "bus")
        .ok_or_else(|| "Bus mixer edit bus was invalid".to_string())?;
    let bus = map_string(map, "bus-id")
        .and_then(|value| value.parse::<u64>().ok())
        .map(sequencer::sequencer::BusId)
        .ok_or_else(|| "Bus mixer edit stable bus ID was invalid".to_string())?;
    let bus_idx = app
        .buses
        .iter()
        .position(|channel| channel.id == bus)
        .ok_or_else(|| {
            format!(
                "Bus mixer edit bus {requested_bus_idx} ({}) no longer exists",
                bus.0,
            )
        })?;
    let command = match op.as_str() {
        "volume" => app::AppCommand::SetBusVolume {
            bus,
            value: map_number(map, "value")
                .filter(|value| value.is_finite())
                .ok_or_else(|| "Bus mixer volume was invalid".to_string())?
                as f32,
        },
        "toggle-mute" => app::AppCommand::ToggleBusMute { bus },
        "toggle-solo" => app::AppCommand::ToggleBusSolo { bus },
        _ => return Err(format!("Unsupported bus mixer edit operation: {op}")),
    };
    app::try_apply_command(app, command)
        .map(|outcome| (outcome, bus_idx))
        .map_err(|error| format!("could not apply bus mixer edit: {error:?}"))
}

#[derive(Clone, Debug, PartialEq)]
pub(super) enum DrumLaneHistoryAction {
    Toggle {
        track: usize,
        pad_note: i32,
        step: usize,
    },
    Duration {
        track: usize,
        pad_note: i32,
        step: usize,
        duration: f32,
    },
    Move {
        track: usize,
        pad_note: i32,
        steps: Vec<usize>,
        delta: isize,
        move_selection: bool,
    },
    Clear {
        track: usize,
        pad_note: i32,
        steps: Vec<usize>,
    },
}

impl DrumLaneHistoryAction {
    pub(super) fn track(&self) -> usize {
        match self {
            Self::Toggle { track, .. }
            | Self::Duration { track, .. }
            | Self::Move { track, .. }
            | Self::Clear { track, .. } => *track,
        }
    }

    pub(super) fn affected_steps(&self) -> Vec<usize> {
        let mut affected = match self {
            Self::Toggle { step, .. } | Self::Duration { step, .. } => vec![*step],
            Self::Clear { steps, .. } => steps.clone(),
            Self::Move { steps, delta, .. } => {
                let mut affected = steps.clone();
                affected.extend(steps.iter().map(|step| (*step as isize + *delta) as usize));
                affected
            }
        };
        affected.sort_unstable();
        affected.dedup();
        affected
    }

    pub(super) fn label(&self) -> &'static str {
        match self {
            Self::Toggle { .. } => "Toggle drum-lane step",
            Self::Duration { .. } => "Set drum-lane duration",
            Self::Move { .. } => "Move drum-lane steps",
            Self::Clear { .. } => "Delete drum-lane steps",
        }
    }
}

pub(super) fn map_usize_list(
    map: &std::collections::HashMap<String, Rc<RefCell<Value>>>,
    key: &str,
) -> Option<Vec<usize>> {
    let cell = map.get(key)?;
    let Value::List(values) = &*cell.borrow() else {
        return None;
    };
    values
        .iter()
        .map(|value| match &*value.borrow() {
            Value::Number(value)
                if value.is_finite() && *value >= 0.0 && *value <= usize::MAX as f64 =>
            {
                Some(*value as usize)
            }
            _ => None,
        })
        .collect()
}

pub(super) fn parse_drum_lane_history_action(payload: &Value) -> Result<DrumLaneHistoryAction, String> {
    let Value::Map(map) = payload else {
        return Err("drum-lane edit payload was invalid".to_string());
    };
    let op = map_string(map, "op")
        .ok_or_else(|| "drum-lane edit operation was missing".to_string())?;
    let track = map_usize(map, "track")
        .ok_or_else(|| "drum-lane edit track was invalid".to_string())?;
    let pad_note = map_number(map, "pad-note")
        .filter(|value| value.is_finite())
        .ok_or_else(|| "drum-lane pad note was invalid".to_string())?
        .round() as i32;
    match op.as_str() {
        "toggle" => Ok(DrumLaneHistoryAction::Toggle {
            track,
            pad_note,
            step: map_usize(map, "step")
                .ok_or_else(|| "drum-lane step was invalid".to_string())?,
        }),
        "duration" => Ok(DrumLaneHistoryAction::Duration {
            track,
            pad_note,
            step: map_usize(map, "step")
                .ok_or_else(|| "drum-lane step was invalid".to_string())?,
            duration: map_number(map, "duration")
                .filter(|value| value.is_finite())
                .ok_or_else(|| "drum-lane duration was invalid".to_string())?
                as f32,
        }),
        "move" => Ok(DrumLaneHistoryAction::Move {
            track,
            pad_note,
            steps: map_usize_list(map, "steps")
                .ok_or_else(|| "drum-lane move steps were invalid".to_string())?,
            delta: map_number(map, "delta")
                .filter(|value| value.is_finite())
                .ok_or_else(|| "drum-lane move delta was invalid".to_string())?
                .round() as isize,
            move_selection: map_bool(map, "move-selection"),
        }),
        "clear" => Ok(DrumLaneHistoryAction::Clear {
            track,
            pad_note,
            steps: map_usize_list(map, "steps")
                .ok_or_else(|| "drum-lane clear steps were invalid".to_string())?,
        }),
        _ => Err(format!("unknown drum-lane history operation {op}")),
    }
}

pub(super) fn apply_drum_lane_history_host_command(
    app: &mut app::App,
    payload: &Value,
) -> Result<(app::edit::EditOutcome, DrumLaneHistoryAction), String> {
    let action = parse_drum_lane_history_action(payload)?;
    let track = action.track();
    if track >= app.state.active_track_count() {
        return Err(format!("drum-lane track {track} was out of range"));
    }
    let affected = action.affected_steps();
    if affected.is_empty() || affected.iter().any(|step| *step >= MAX_STEPS) {
        return Err("drum-lane affected steps were invalid".to_string());
    }
    let label = action.label();
    let mutation = action.clone();
    let outcome = app::edit::apply_recorded_step_mutation(
        app,
        track,
        &affected,
        label,
        |app| {
            match &mutation {
                DrumLaneHistoryAction::Toggle {
                    pad_note, step, ..
                } => {
                    app.state.toggle_drum_lane_step_no_publish(track, *step, *pad_note);
                }
                DrumLaneHistoryAction::Duration {
                    pad_note,
                    step,
                    duration,
                    ..
                } => {
                    app.state.set_drum_lane_step_duration_no_publish(
                        track,
                        *step,
                        *pad_note,
                        *duration,
                    );
                }
                DrumLaneHistoryAction::Move {
                    pad_note,
                    steps,
                    delta,
                    ..
                } => {
                    app.state.move_drum_lane_steps_no_publish(
                        track,
                        *pad_note,
                        steps,
                        *delta,
                    );
                }
                DrumLaneHistoryAction::Clear {
                    pad_note, steps, ..
                } => {
                    app.state.clear_drum_lane_steps_no_publish(track, *pad_note, steps);
                }
            }
            Ok(())
        },
    )
    .map_err(|error| format!("could not apply drum-lane edit: {error:?}"))?;
    Ok((outcome, action))
}

pub(super) fn apply_piano_roll_history_host_command(
    app: &mut app::App,
    selection: &Arc<Mutex<HashSet<u64>>>,
    move_state: &Arc<Mutex<Option<PianoRollMoveState>>>,
    clipboard: &PianoRollClipboard,
    payload: &Value,
) -> Result<(app::edit::EditOutcome, String, usize), String> {
    let Value::Map(map) = payload else {
        return Err("piano-roll edit payload was invalid".to_string());
    };
    let track = map_usize(map, "track")
        .ok_or_else(|| "piano-roll edit track was invalid".to_string())?;
    if track >= app.state.active_track_count() {
        return Err(format!("piano-roll edit track {track} was out of range"));
    }
    let action = map
        .get("action")
        .map(|value| value.borrow().clone())
        .ok_or_else(|| "piano-roll edit action was missing".to_string())?;
    let plan = piano_roll_history_plan(&app.state, track, &action, clipboard)?
        .ok_or_else(|| "piano-roll action is not recordable at this boundary".to_string())?;
    let mut status = None;
    let outcome = app::edit::apply_recorded_step_mutation(
        app,
        track,
        &plan.steps,
        plan.label,
        |app| {
            status = Some(
                apply_piano_roll_action_with_clipboard(
                    &app.state,
                    track,
                    selection,
                    move_state,
                    clipboard,
                    &action,
                )
                .map_err(app::edit::EditError::ReplayFailed)?,
            );
            Ok(())
        },
    )
    .map_err(|error| format!("could not apply piano-roll edit: {error:?}"))?;
    Ok((
        outcome,
        status.unwrap_or_else(|| "piano-roll edit made no change".to_string()),
        track,
    ))
}

pub(super) struct ActivePianoRollHistoryGesture {
    pub(super) kind: PianoRollDragKind,
    pub(super) track: usize,
    pub(super) transaction: app::edit::StepGestureTransaction,
}

pub(super) fn piano_roll_host_action(payload: &Value) -> Result<(usize, Value), String> {
    let Value::Map(map) = payload else {
        return Err("piano-roll gesture payload was invalid".to_string());
    };
    let track = map_usize(map, "track")
        .ok_or_else(|| "piano-roll gesture track was invalid".to_string())?;
    let action = map
        .get("action")
        .map(|value| value.borrow().clone())
        .ok_or_else(|| "piano-roll gesture action was missing".to_string())?;
    Ok((track, action))
}

pub(super) fn apply_piano_roll_gesture_update(
    app: &mut app::App,
    selection: &Arc<Mutex<HashSet<u64>>>,
    move_state: &Arc<Mutex<Option<PianoRollMoveState>>>,
    active: &mut Option<ActivePianoRollHistoryGesture>,
    payload: &Value,
) -> Result<(String, usize), String> {
    let (track, action) = piano_roll_host_action(payload)?;
    if track >= app.state.active_track_count() {
        return Err(format!("piano-roll gesture track {track} was out of range"));
    }
    let Some(PianoRollGestureCommand::Update(kind)) = piano_roll_gesture_command(&action) else {
        return Err("piano-roll gesture update action was invalid".to_string());
    };
    if active
        .as_ref()
        .is_some_and(|gesture| gesture.kind != kind || gesture.track != track)
    {
        let previous = active.take().expect("active gesture disappeared");
        previous
            .transaction
            .rollback(app)
            .map_err(|error| format!("could not roll back interrupted gesture: {error:?}"))?;
        *move_state.lock().unwrap() = None;
    }
    let touched = piano_roll_gesture_touched_steps(&app.state, track, move_state, &action)?;
    if active.is_none() {
        let label = match kind {
            PianoRollDragKind::Move => "Move piano-roll notes",
            PianoRollDragKind::Resize => "Resize piano-roll notes",
        };
        *active = Some(ActivePianoRollHistoryGesture {
            kind,
            track,
            transaction: app::edit::StepGestureTransaction::begin(app, track, &touched, label)
                .map_err(|error| format!("could not begin piano-roll gesture: {error:?}"))?,
        });
    } else if let Err(error) = active
        .as_mut()
        .expect("active gesture disappeared")
        .transaction
        .capture_additional_steps(app, &touched)
    {
        let gesture = active.take().expect("active gesture disappeared");
        let rollback = gesture.transaction.rollback(app);
        *move_state.lock().unwrap() = None;
        return match rollback {
            Ok(()) => Err(format!("could not extend piano-roll gesture: {error:?}")),
            Err(rollback_error) => Err(format!(
                "could not extend piano-roll gesture: {error:?}; rollback failed: {rollback_error:?}"
            )),
        };
    }
    let status = match apply_piano_roll_action(
        &app.state,
        track,
        selection,
        move_state,
        &action,
    ) {
        Ok(status) => status,
        Err(error) => {
            let gesture = active.take().expect("active gesture disappeared");
            let rollback = gesture.transaction.rollback(app);
            *move_state.lock().unwrap() = None;
            return match rollback {
                Ok(()) => Err(error),
                Err(rollback_error) => Err(format!(
                    "{error}; gesture rollback failed: {rollback_error:?}"
                )),
            };
        }
    };
    app.state.publish_scheduler_snapshot();
    Ok((status, track))
}

pub(super) fn finish_piano_roll_gesture(
    app: &mut app::App,
    move_state: &Arc<Mutex<Option<PianoRollMoveState>>>,
    active: &mut Option<ActivePianoRollHistoryGesture>,
    payload: &Value,
) -> Result<(app::edit::EditOutcome, usize), String> {
    let (track, action) = piano_roll_host_action(payload)?;
    let Some(PianoRollGestureCommand::Finish(kind)) = piano_roll_gesture_command(&action) else {
        return Err("piano-roll gesture finish action was invalid".to_string());
    };
    let Some(gesture) = active.take() else {
        *move_state.lock().unwrap() = None;
        return Ok((app::edit::EditOutcome::NoOp, track));
    };
    if gesture.kind != kind || gesture.track != track {
        gesture
            .transaction
            .rollback(app)
            .map_err(|error| format!("could not roll back mismatched gesture: {error:?}"))?;
        *move_state.lock().unwrap() = None;
        return Err("piano-roll gesture finished with a different edit kind".to_string());
    }
    *move_state.lock().unwrap() = None;
    gesture
        .transaction
        .commit(app)
        .map(|outcome| (outcome, track))
        .map_err(|error| format!("could not commit piano-roll gesture: {error:?}"))
}

pub(super) fn map_u32(map: &std::collections::HashMap<String, Rc<RefCell<Value>>>, key: &str) -> Option<u32> {
    map_number(map, key).and_then(|value| {
        (value.is_finite() && value >= 0.0 && value <= u32::MAX as f64).then_some(value as u32)
    })
}

pub(super) fn map_u8_list(
    map: &std::collections::HashMap<String, Rc<RefCell<Value>>>,
    key: &str,
) -> Option<Vec<u8>> {
    let cell = map.get(key)?;
    let Value::List(items) = &*cell.borrow() else {
        return None;
    };
    Some(
        items
            .iter()
            .filter_map(|item| match &*item.borrow() {
                Value::Number(value) if *value >= 0.0 && *value <= u8::MAX as f64 => {
                    Some(*value as u8)
                }
                _ => None,
            })
            .collect(),
    )
}

pub(super) fn rack_slot_snapshot_for_host(
    state: &Arc<SequencerState>,
    track: usize,
    slot_idx: usize,
) -> Option<sequencer::sequencer::RackSlotSnapshot> {
    state
        .pattern
        .rack_tracks
        .lock()
        .unwrap()
        .get(track)
        .and_then(|rack| rack.as_ref())
        .and_then(|rack| rack.slots.get(slot_idx))
        .cloned()
}

pub(super) fn rack_slot_effect_param_needs_panel_rebuild(
    state: &Arc<SequencerState>,
    track: usize,
    rack_slot: usize,
    effect_slot: usize,
    param_idx: usize,
) -> bool {
    rack_slot_snapshot_for_host(state, track, rack_slot)
        .and_then(|slot| slot.effect_descriptors.get(effect_slot).cloned())
        .and_then(|descriptor| descriptor.params.get(param_idx).cloned())
        .is_none_or(|param| param_change_needs_fx_rebuild(&param))
}

pub(super) fn param_change_needs_fx_rebuild(param: &sequencer::effects::ParamDescriptor) -> bool {
    matches!(param.kind, ParamKind::Boolean | ParamKind::Enum { .. })
}

pub(super) struct AgentDraftApplyResult {
    pub(super) track_index: usize,
    pub(super) created_track: bool,
}

pub(super) struct AgentFinalizeResult {
    pub(super) track_index: usize,
    pub(super) instrument_name: String,
}

pub(super) struct AgentEffectApplyResult {
    pub(super) track_index: usize,
    pub(super) slot_index: usize,
}

pub(super) struct AgentEffectFinalizeResult {
    pub(super) track_index: Option<usize>,
    pub(super) slot_index: Option<usize>,
    pub(super) effect_name: String,
}
