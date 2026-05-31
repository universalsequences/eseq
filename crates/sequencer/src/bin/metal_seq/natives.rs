use super::*;

pub(crate) struct RuntimeInit {
    pub(crate) runtime: Runtime,
    pub(crate) accumulator_names: Arc<Mutex<Vec<String>>>,
    pub(crate) midi_fx_names: Arc<Mutex<Vec<String>>>,
    pub(crate) sample_browser: Rc<RefCell<DebouncedSampleBrowser>>,
}

fn value_number_field(value: &Value, field: &str) -> Option<usize> {
    let Value::Map(map) = value else {
        return None;
    };
    map.get(field).and_then(|cell| match &*cell.borrow() {
        Value::Number(n) if *n >= 0.0 => Some(*n as usize),
        _ => None,
    })
}

fn value_string_field(value: &Value, field: &str) -> Option<String> {
    let Value::Map(map) = value else {
        return None;
    };
    map.get(field).and_then(|cell| match &*cell.borrow() {
        Value::String(s) => Some(s.clone()),
        Value::Keyword(s) => Some(s.clone()),
        _ => None,
    })
}

fn value_string_list(value: Option<&Value>) -> Vec<String> {
    let Some(Value::List(items)) = value else {
        return Vec::new();
    };
    items
        .iter()
        .filter_map(|item| match &*item.borrow() {
            Value::String(value) | Value::Keyword(value) => Some(value.trim().to_string()),
            _ => None,
        })
        .filter(|value| !value.is_empty())
        .collect()
}

fn delete_kind_name(value: &Value) -> Option<&str> {
    match value {
        Value::Keyword(kind) | Value::String(kind) => Some(kind.as_str()),
        _ => None,
    }
}

fn trace_ui_enabled() -> bool {
    std::env::var_os("ESEQLISP_TRACE_UI").is_some()
}

fn parse_fx_delete_chain(value: &Value) -> Option<FxDeleteChain> {
    match value_string_field(value, "chain")?.as_str() {
        "audio" | "fx" => Some(FxDeleteChain::Audio),
        "midi" | "midi-fx" => Some(FxDeleteChain::Midi),
        "bus" | "bus-fx" => Some(FxDeleteChain::Bus),
        _ => None,
    }
}

fn parse_delete_target(kind: &Value, payload: &Value) -> Result<ActiveDeleteTarget, String> {
    match delete_kind_name(kind) {
        Some("mixer-track") | Some("track") => {
            let track = value_number_field(payload, "track")
                .ok_or_else(|| "mixer-track delete target expects :track".to_string())?;
            Ok(ActiveDeleteTarget::MixerTrack { track })
        }
        Some("mod-route") | Some("route") | Some("cable") => {
            let source = value_number_field(payload, "source")
                .ok_or_else(|| "mod-route delete target expects :source".to_string())?;
            let dest = value_number_field(payload, "dest")
                .ok_or_else(|| "mod-route delete target expects :dest".to_string())?;
            let input = value_number_field(payload, "input")
                .ok_or_else(|| "mod-route delete target expects :input".to_string())?;
            Ok(ActiveDeleteTarget::ModRoute {
                source,
                dest,
                input,
            })
        }
        Some("fx-effect") | Some("effect") => {
            let chain = parse_fx_delete_chain(payload)
                .ok_or_else(|| "fx-effect delete target expects :chain".to_string())?;
            let slot = value_number_field(payload, "slot")
                .ok_or_else(|| "fx-effect delete target expects :slot".to_string())?;
            let bus = value_number_field(payload, "bus");
            if chain == FxDeleteChain::Bus && bus.is_none() {
                return Err("bus fx-effect delete target expects :bus".to_string());
            }
            Ok(ActiveDeleteTarget::FxEffect { chain, bus, slot })
        }
        Some(other) => Err(format!("unknown delete target kind :{other}")),
        None => Err("delete target kind must be a keyword or string".to_string()),
    }
}

fn effect_param_target(
    slot_state: &sequencer::effects::EffectSlotState,
    param_idx: usize,
) -> Option<(u64, u64)> {
    let idx = slot_state.resolve_node_idx(param_idx);
    if idx == u32::MAX as u64 {
        return None;
    }
    if idx as u32 >= sequencer::voice_modulator::MOD_PARAM_BASE {
        let modulator_node_id = slot_state.modulator_node_id.load(Ordering::Relaxed);
        (modulator_node_id != 0).then_some((
            modulator_node_id as u64,
            idx - sequencer::voice_modulator::MOD_PARAM_BASE as u64,
        ))
    } else {
        let node_id = slot_state.node_id.load(Ordering::Relaxed);
        (node_id != 0).then_some((node_id as u64, idx))
    }
}

fn active_delete_target_kind(target: Option<&ActiveDeleteTarget>) -> Value {
    match target {
        Some(ActiveDeleteTarget::MixerTrack { .. }) => Value::String("mixer-track".to_string()),
        Some(ActiveDeleteTarget::ModRoute { .. }) => Value::String("mod-route".to_string()),
        Some(ActiveDeleteTarget::FxEffect { .. }) => Value::String("fx-effect".to_string()),
        None => Value::Bool(false),
    }
}

fn bump_delete_target_version(
    active_delete_target_version: &Arc<AtomicUsize>,
    ui_epoch: &Arc<AtomicUsize>,
) {
    active_delete_target_version.fetch_add(1, Ordering::Relaxed);
    ui_epoch.fetch_add(1, Ordering::Relaxed);
}

#[cfg(test)]
mod delete_target_tests {
    use super::*;

    fn map_value(entries: impl IntoIterator<Item = (&'static str, Value)>) -> Value {
        Value::Map(
            entries
                .into_iter()
                .map(|(key, value)| (key.to_string(), Rc::new(RefCell::new(value))))
                .collect(),
        )
    }

    #[test]
    fn delete_target_parser_distinguishes_mixer_track_mod_route_and_fx_effects() {
        assert_eq!(
            parse_delete_target(
                &Value::Keyword("mixer-track".to_string()),
                &map_value([("track", Value::Number(2.0))]),
            )
            .expect("mixer target"),
            ActiveDeleteTarget::MixerTrack { track: 2 }
        );

        assert_eq!(
            parse_delete_target(
                &Value::Keyword("mod-route".to_string()),
                &map_value([
                    ("source", Value::Number(0.0)),
                    ("dest", Value::Number(3.0)),
                    ("input", Value::Number(1.0)),
                ]),
            )
            .expect("mod route target"),
            ActiveDeleteTarget::ModRoute {
                source: 0,
                dest: 3,
                input: 1,
            }
        );

        assert_eq!(
            parse_delete_target(
                &Value::Keyword("fx-effect".to_string()),
                &map_value([
                    ("chain", Value::String("bus".to_string())),
                    ("bus", Value::Number(1.0)),
                    ("slot", Value::Number(4.0)),
                ]),
            )
            .expect("bus fx target"),
            ActiveDeleteTarget::FxEffect {
                chain: FxDeleteChain::Bus,
                bus: Some(1),
                slot: 4,
            }
        );
    }

    #[test]
    fn delete_target_parser_rejects_incomplete_bus_fx_target() {
        let err = parse_delete_target(
            &Value::Keyword("fx-effect".to_string()),
            &map_value([
                ("chain", Value::String("bus".to_string())),
                ("slot", Value::Number(0.0)),
            ]),
        )
        .expect_err("bus fx target without bus should fail");
        assert!(err.contains(":bus"), "unexpected error: {err}");
    }
}

pub(crate) fn init_runtime(
    app: &ui::App,
    state: Arc<SequencerState>,
    track_names: &[String],
    track_pan_ids: Arc<Mutex<Vec<i32>>>,
    buses: Arc<Mutex<Vec<ui::BusChannelState>>>,
    bus_node_ids: Arc<Mutex<Vec<ui::BusNodeIds>>>,
    current_track: Arc<AtomicUsize>,
    selected_steps: Arc<Mutex<HashSet<usize>>>,
    piano_roll_selection: Arc<Mutex<HashSet<u64>>>,
    piano_roll_move_state: Arc<Mutex<Option<PianoRollMoveState>>>,
    recording: Arc<AtomicBool>,
    record_armed: Arc<Mutex<Vec<bool>>>,
    ui_epoch: Arc<AtomicUsize>,
    fx_epoch: Arc<AtomicUsize>,
    ui_invalidations: Arc<UiInvalidationQueue>,
    expanded_step_projection: Arc<ExpandedStepProjectionRegistry>,
    active_delete_target: Arc<Mutex<Option<ActiveDeleteTarget>>>,
    active_delete_target_version: Arc<AtomicUsize>,
    auto_follow_override_until: Arc<Mutex<Option<Instant>>>,
    lg_raw: *mut sequencer::audiograph::LiveGraph,
) -> RuntimeInit {
    let mut runtime = Runtime::new();
    let debug_accum = std::env::var_os("TINYSEQ_DEBUG_ACCUM").is_some();

    let track_count = track_names.len();
    let effect_descriptors = app.graph.effect_descriptors.clone();
    let accumulator_names = Arc::new(Mutex::new(build_accumulator_names(&app)));
    let midi_fx_names = Arc::new(Mutex::new(Vec::<String>::new()));

    // Register SEQ reactive namespace
    runtime.register_reactive(
        "SEQ",
        {
            let mut fields = vec![
                ("playing", Value::Bool(false)),
                ("bpm", Value::Number(120.0)),
                ("num-steps", Value::Number(PAGE_SIZE as f64)),
                ("num-tracks", Value::Number(track_count as f64)),
                ("current-track", Value::Number(0.0)),
                ("delete-target-version", Value::Number(0.0)),
                (
                    "current-pattern",
                    Value::Number(state.pattern.current_pattern.load(Ordering::Relaxed) as f64),
                ),
                (
                    "num-patterns",
                    Value::Number(state.pattern.num_patterns.load(Ordering::Relaxed) as f64),
                ),
                ("auto-follow", Value::Bool(true)),
                ("playhead", Value::Number(0.0)),
                ("transport-playhead", Value::Number(0.0)),
                ("sampler-playhead", Value::Number(0.0)),
                ("track-ids", build_track_ids(&app)),
                ("track-instrument-types", build_track_instrument_types(&app)),
                (
                    "track-mod-output-available",
                    build_track_mod_output_available(&app),
                ),
                (
                    "track-instrument-run-modes",
                    build_track_instrument_run_modes(&app),
                ),
                ("track-names", build_track_names(&track_names)),
                (
                    "track-num-steps",
                    build_all_track_num_steps_value(&state, app),
                ),
                (
                    "track-duration-spans",
                    build_all_track_duration_spans_value(&state, app),
                ),
                (
                    "steps",
                    if track_count == 0 {
                        Value::List(vec![])
                    } else {
                        build_steps_value(&state, 0)
                    },
                ),
                ("piano-roll-lanes", build_piano_roll_lanes_value()),
                (
                    "piano-roll-items",
                    if track_count == 0 {
                        Value::List(vec![])
                    } else {
                        build_piano_roll_items_value(&state, 0, &piano_roll_selection)
                    },
                ),
                (
                    "piano-roll-selection",
                    build_piano_roll_selection_value(&piano_roll_selection),
                ),
                (
                    "velocities",
                    if track_count == 0 {
                        Value::List(vec![])
                    } else {
                        build_param_list(&state, 0, StepParam::Velocity)
                    },
                ),
                (
                    "durations",
                    if track_count == 0 {
                        Value::List(vec![])
                    } else {
                        build_param_list(&state, 0, StepParam::Duration)
                    },
                ),
                (
                    "transposes",
                    if track_count == 0 {
                        Value::List(vec![])
                    } else {
                        build_param_list(&state, 0, StepParam::Transpose)
                    },
                ),
                (
                    "auxas",
                    if track_count == 0 {
                        Value::List(vec![])
                    } else {
                        build_param_list(&state, 0, StepParam::AuxA)
                    },
                ),
                (
                    "pans",
                    if track_count == 0 {
                        Value::List(vec![])
                    } else {
                        build_param_list(&state, 0, StepParam::Pan)
                    },
                ),
                (
                    "syncs",
                    if track_count == 0 {
                        Value::List(vec![])
                    } else {
                        build_param_list(&state, 0, StepParam::Sync)
                    },
                ),
                (
                    "delays",
                    if track_count == 0 {
                        Value::List(vec![])
                    } else {
                        build_param_list(&state, 0, StepParam::Delay)
                    },
                ),
                ("sync-labels", build_sync_labels()),
                ("track-volumes", build_track_volumes(&state)),
                (
                    "track-pans",
                    build_all_track_param_lists_value(&state, &app, StepParam::Pan),
                ),
                (
                    "track-delays",
                    build_all_track_param_lists_value(&state, &app, StepParam::Delay),
                ),
                ("track-mixer-pans", build_track_pans(&state)),
                ("track-outputs", build_track_outputs(&app, &state)),
                ("track-bus-sends", build_all_track_bus_sends(&app, &state)),
                ("mod-routes", build_mod_routes(&state)),
                ("track-mutes", build_track_mutes(&state)),
                ("track-solos", build_track_solos(&state)),
                ("track-muted-by-solo", build_track_muted_by_solo(&state)),
                (
                    "bus-names",
                    build_track_names(
                        &app.buses
                            .iter()
                            .map(|bus| bus.name.clone())
                            .collect::<Vec<_>>(),
                    ),
                ),
                (
                    "bus-volumes",
                    Value::List(
                        app.buses
                            .iter()
                            .map(|bus| Rc::new(RefCell::new(Value::Number(bus.volume as f64))))
                            .collect(),
                    ),
                ),
                (
                    "bus-mutes",
                    Value::List(
                        app.buses
                            .iter()
                            .map(|bus| Rc::new(RefCell::new(Value::Bool(bus.mute))))
                            .collect(),
                    ),
                ),
                (
                    "bus-solos",
                    Value::List(
                        app.buses
                            .iter()
                            .map(|bus| Rc::new(RefCell::new(Value::Bool(bus.solo))))
                            .collect(),
                    ),
                ),
                ("bus-effects", build_bus_effects_value(&app)),
                ("bus-steps", build_bus_steps_value(&app)),
                ("bus-velocities", build_bus_param_lists(&app, "velocity")),
                ("bus-durations", build_bus_param_lists(&app, "duration")),
                ("bus-syncs", build_bus_param_lists(&app, "sync")),
                ("bus-num-steps", build_bus_num_steps_value(&app)),
                ("bus-timebases", build_bus_timebase_value(&app)),
                ("bus-swings", build_bus_swing_value(&app)),
                (
                    "bus-swing-resolutions",
                    build_bus_swing_resolution_value(&app),
                ),
                ("bus-step-has-plocks", build_bus_step_has_plocks(&app)),
                ("bus-playheads", build_bus_playheads_value(&app)),
                (
                    "effects",
                    if track_count == 0 {
                        Value::List(vec![])
                    } else {
                        build_effects_value(&state, 0, &effect_descriptors, &selected_steps)
                    },
                ),
                (
                    "midi-effects",
                    if track_count == 0 {
                        Value::List(vec![])
                    } else {
                        build_midi_effects_value(&state, 0, &selected_steps)
                    },
                ),
                (
                    "instrument-panel",
                    if track_count == 0 {
                        Value::List(vec![])
                    } else {
                        build_instrument_panel_value(&app, 0, &selected_steps)
                    },
                ),
                ("track-params", build_track_params(&state, 0)),
                (
                    "tp-attack",
                    Value::Number(state.pattern.track_params[0].get_attack_ms() as f64),
                ),
                (
                    "tp-release",
                    Value::Number(state.pattern.track_params[0].get_release_ms() as f64),
                ),
                (
                    "tp-swing",
                    Value::Number(state.pattern.track_params[0].get_swing() as f64),
                ),
                (
                    "tp-send",
                    Value::Number(state.pattern.track_params[0].get_send() as f64),
                ),
                ("tp-output", {
                    let tp = &state.pattern.track_params[0];
                    let label = match tp.output() {
                        sequencer::sequencer::TrackOutput::Mix => "main".to_string(),
                        sequencer::sequencer::TrackOutput::None => "sends only".to_string(),
                        sequencer::sequencer::TrackOutput::Bus(id) => app
                            .buses
                            .iter()
                            .find(|bus| bus.id == id)
                            .map(|bus| bus.name.clone())
                            .unwrap_or_else(|| "main".to_string()),
                    };
                    Value::String(label)
                }),
                (
                    "track-output-options",
                    Value::List(
                        std::iter::once("main".to_string())
                            .chain(std::iter::once("sends only".to_string()))
                            .chain(
                                app.buses
                                    .iter()
                                    .filter(|bus| bus.id != sequencer::sequencer::BusId::MIX)
                                    .map(|bus| bus.name.clone()),
                            )
                            .map(|label| Rc::new(RefCell::new(Value::String(label))))
                            .collect(),
                    ),
                ),
                ("tp-bus-sends", {
                    use std::collections::HashMap;
                    let tp = &state.pattern.track_params[0];
                    let sends = tp.sends();
                    Value::List(
                        app.buses
                            .iter()
                            .enumerate()
                            .filter(|(_, bus)| bus.id != sequencer::sequencer::BusId::MIX)
                            .map(|(bus_idx, bus)| {
                                let amount = sends
                                    .iter()
                                    .find(|send| send.destination == bus.id)
                                    .map(|send| send.amount)
                                    .unwrap_or(0.0);
                                let mut map = HashMap::new();
                                map.insert(
                                    "bus-idx".to_string(),
                                    Rc::new(RefCell::new(Value::Number(bus_idx as f64))),
                                );
                                map.insert(
                                    "name".to_string(),
                                    Rc::new(RefCell::new(Value::String(bus.name.clone()))),
                                );
                                map.insert(
                                    "amount".to_string(),
                                    Rc::new(RefCell::new(Value::Number(amount as f64))),
                                );
                                Rc::new(RefCell::new(Value::Map(map)))
                            })
                            .collect(),
                    )
                }),
                (
                    "tp-num-steps",
                    Value::Number(state.pattern.track_params[0].get_num_steps() as f64),
                ),
                (
                    "tp-gate",
                    Value::Bool(state.pattern.track_params[0].is_gate_on()),
                ),
                (
                    "tp-poly",
                    Value::Bool(state.pattern.track_params[0].is_polyphonic()),
                ),
                (
                    "tp-timebase",
                    Value::String(
                        state.pattern.track_params[0]
                            .get_timebase()
                            .label()
                            .to_string(),
                    ),
                ),
                (
                    "tp-swing-resolution",
                    Value::String(
                        state.pattern.track_params[0]
                            .get_swing_resolution()
                            .label()
                            .to_string(),
                    ),
                ),
                (
                    "tp-fts",
                    Value::String(
                        FTS_SCALE_NAMES
                            .get(state.pattern.track_params[0].get_fts_scale())
                            .copied()
                            .unwrap_or("Off")
                            .to_string(),
                    ),
                ),
                (
                    "tp-accumulator",
                    Value::String(selected_accumulator_name(&app, 0)),
                ),
                (
                    "tp-accum-limit",
                    Value::Number(state.pattern.track_params[0].get_accum_limit() as f64),
                ),
                (
                    "tp-accum-mode",
                    Value::String(
                        accum_mode_label(state.pattern.track_params[0].get_accum_mode())
                            .to_string(),
                    ),
                ),
                ("accumulator-options", build_accumulator_options(&app)),
                ("fts-options", build_fts_options()),
                ("accum-mode-options", build_accum_mode_options()),
                (
                    "available-builtin-effects",
                    build_available_builtin_effects(),
                ),
                ("available-effects", build_available_effects()),
                ("available-midi-effects", build_available_midi_effects()),
                ("selected-steps", build_selection_value(&selected_steps)),
                (
                    "step-has-plocks",
                    if track_count == 0 {
                        Value::List(vec![])
                    } else {
                        build_step_has_plocks(&state, 0, &effect_descriptors)
                    },
                ),
                (
                    "track-plocks",
                    if track_count == 0 {
                        Value::List(vec![])
                    } else {
                        build_track_plocks_value(&app, &state, 0, &selected_steps)
                    },
                ),
                ("compiling", Value::Bool(false)),
                ("recording", Value::Bool(false)),
                ("cpu-load-pct", Value::Number(0.0)),
                ("master-peak-l", Value::Number(0.0)),
                ("master-peak-r", Value::Number(0.0)),
                (
                    "record-armed",
                    build_record_armed_value(&record_armed.lock().unwrap()),
                ),
                ("playhead-page", Value::Number(0.0)),
                ("sidebar-kind", Value::String("sampler".to_string())),
                ("sidebar-instrument-name", Value::String(String::new())),
                (
                    "sidebar-instrument-display-name",
                    Value::String(String::new()),
                ),
                ("sidebar-loaded-preset", Value::String(String::new())),
                ("sidebar-selected-sample", Value::String(String::new())),
                ("sidebar-track-index", Value::Number(0.0)),
                ("sidebar-presets", Value::List(vec![])),
                ("sidebar-preset-tree", Value::List(vec![])),
                ("current-project-name", Value::String(String::new())),
                // Editor mode state (for inline instrument/effect creation/editing)
                ("editor-active", Value::Bool(false)),
                ("editor-canceling", Value::Bool(false)),
                ("editor-error", Value::String(String::new())),
                ("editor-mode", Value::String(String::new())),
                ("editor-buffer-name", Value::String(String::new())),
                (
                    "editor-instrument-run-mode",
                    Value::String("instrument".to_string()),
                ),
            ];
            for idx in 0..track_count {
                fields.push((
                    Box::leak(track_selected_field(idx).into_boxed_str()),
                    Value::Bool(idx == 0),
                ));
                fields.push((
                    Box::leak(mixer_track_delete_target_field(idx).into_boxed_str()),
                    Value::Bool(false),
                ));
                fields.push((
                    Box::leak(format!("track-peak-{idx}").into_boxed_str()),
                    Value::Number(0.0),
                ));
                fields.push((
                    Box::leak(format!("modulator-phase-{idx}").into_boxed_str()),
                    Value::Number(0.0),
                ));
                fields.push((
                    Box::leak(format!("modulator-level-{idx}").into_boxed_str()),
                    Value::Number(1.0),
                ));
            }
            for idx in 0..app.buses.len() {
                fields.push((
                    Box::leak(format!("bus-peak-{idx}").into_boxed_str()),
                    Value::Number(0.0),
                ));
            }
            for track in 0..track_count {
                for (bus_idx, bus) in app.buses.iter().enumerate() {
                    if bus.id == sequencer::sequencer::BusId::MIX {
                        continue;
                    }
                    fields.push((
                        Box::leak(track_bus_send_field(track, bus_idx).into_boxed_str()),
                        Value::Number(
                            track_bus_send_amount(&app, &state, track, bus_idx).unwrap_or(0.0)
                                as f64,
                        ),
                    ));
                }
            }
            if track_count > 0 {
                for (bus_idx, bus) in app.buses.iter().enumerate() {
                    if bus.id == sequencer::sequencer::BusId::MIX {
                        continue;
                    }
                    fields.push((
                        Box::leak(current_track_bus_send_field(bus_idx).into_boxed_str()),
                        Value::Number(
                            track_bus_send_amount(&app, &state, 0, bus_idx).unwrap_or(0.0) as f64,
                        ),
                    ));
                }
            }
            for idx in 0..MAX_STEPS {
                fields.push((
                    Box::leak(format!("playhead-active-{idx}").into_boxed_str()),
                    Value::Bool(idx == 0),
                ));
            }
            for track in 0..track_count {
                fields.push((
                    Box::leak(track_playhead_page_field(track).into_boxed_str()),
                    Value::Number((track_active_playhead_step(&state, track) / PAGE_SIZE) as f64),
                ));
                for step in 0..MAX_STEPS {
                    fields.push((
                        Box::leak(track_step_active_field(track, step).into_boxed_str()),
                        Value::Bool(state.pattern.patterns[track].is_active(step)),
                    ));
                    fields.push((
                        Box::leak(track_step_duration_field(track, step).into_boxed_str()),
                        Value::Bool(track_step_duration_covered(&state, track, step)),
                    ));
                    fields.push((
                        Box::leak(track_step_plocked_field(track, step).into_boxed_str()),
                        Value::Bool(false),
                    ));
                    fields.push((
                        Box::leak(track_step_selected_field(track, step).into_boxed_str()),
                        Value::Bool(false),
                    ));
                    fields.push((
                        Box::leak(track_playhead_active_field(track, step).into_boxed_str()),
                        Value::Bool(step == track_active_playhead_step(&state, track)),
                    ));
                }
            }
            fields
        },
        false,
    );
    runtime.register_reactive("SEQV", vec![], true);
    runtime.register_reactive("AGENT", vec![("generation", Value::Number(0.0))], false);
    if track_count > 0 {
        sync_fx_param_binding_fields(&mut runtime, app, &state, 0, &selected_steps);
    }

    // ── Native functions ──

    let delete_target = active_delete_target.clone();
    let delete_target_version = active_delete_target_version.clone();
    let ui_ep = ui_epoch.clone();
    runtime.register_native("seq-set-delete-target", move |args, _ctx| {
        let (Some(kind), Some(payload)) = (args.first(), args.get(1)) else {
            return Err("seq-set-delete-target: expected (kind payload)".into());
        };
        let target = parse_delete_target(kind, payload)?;
        let mut guard = delete_target.lock().unwrap();
        if guard.as_ref() != Some(&target) {
            *guard = Some(target);
            bump_delete_target_version(&delete_target_version, &ui_ep);
        }
        Ok(Value::Bool(true))
    });

    let delete_target = active_delete_target.clone();
    let delete_target_version = active_delete_target_version.clone();
    let ui_ep = ui_epoch.clone();
    runtime.register_native("seq-clear-delete-target", move |_args, _ctx| {
        let mut guard = delete_target.lock().unwrap();
        if guard.take().is_some() {
            bump_delete_target_version(&delete_target_version, &ui_ep);
        }
        Ok(Value::Bool(true))
    });

    let delete_target = active_delete_target.clone();
    runtime.register_native("seq-delete-target?", move |args, _ctx| {
        let (Some(kind), Some(payload)) = (args.first(), args.get(1)) else {
            return Err("seq-delete-target?: expected (kind payload)".into());
        };
        let target = parse_delete_target(kind, payload)?;
        Ok(Value::Bool(
            delete_target.lock().unwrap().as_ref() == Some(&target),
        ))
    });

    let delete_target = active_delete_target.clone();
    runtime.register_native("seq-active-delete-target-kind", move |_args, _ctx| {
        let guard = delete_target.lock().unwrap();
        Ok(active_delete_target_kind(guard.as_ref()))
    });

    let projection = expanded_step_projection.clone();
    let ui_inv = ui_invalidations.clone();
    runtime.register_native("seqv-sync-expanded-step-slots", move |args, _ctx| {
        let (
            Some(Value::Number(track)),
            Some(Value::Number(track_id)),
            Some(Value::Number(page)),
            Some(Value::Number(mode)),
            Some(Value::Number(cursor_step)),
        ) = (
            args.first(),
            args.get(1),
            args.get(2),
            args.get(3),
            args.get(4),
        )
        else {
            return Err(
                "seqv-sync-expanded-step-slots: expected (track track-id page mode cursor-step)"
                    .into(),
            );
        };
        if *track < 0.0 || *track_id < 0.0 || *page < 0.0 || *mode < 0.0 || *cursor_step < 0.0 {
            return Err("seqv-sync-expanded-step-slots: numeric args must be non-negative".into());
        }
        let max_page = MAX_STEPS.saturating_sub(1) / PAGE_SIZE;
        let viewport = ExpandedStepViewport {
            track: (*track as usize).min(sequencer::sequencer::MAX_TRACKS.saturating_sub(1)),
            track_id: *track_id as usize,
            page: (*page as usize).min(max_page),
            mode: (*mode as usize).min(6),
            cursor_step: (*cursor_step as usize).min(MAX_STEPS.saturating_sub(1)),
        };
        if projection.set_viewport(viewport) {
            ui_inv.push(UiInvalidation::ExpandedStepViewport {
                track: viewport.track,
                track_id: viewport.track_id,
            });
        }
        Ok(Value::Bool(true))
    });

    let projection = expanded_step_projection.clone();
    let ui_inv = ui_invalidations.clone();
    runtime.register_native("seqv-clear-expanded-step-slots", move |args, _ctx| {
        let Some(Value::Number(track_id)) = args.first() else {
            return Err("seqv-clear-expanded-step-slots: expected track-id".into());
        };
        if *track_id < 0.0 {
            return Err("seqv-clear-expanded-step-slots: track-id must be non-negative".into());
        }
        let track_id = *track_id as usize;
        if let Some(viewport) = projection.viewport(track_id) {
            if projection.remove_viewport(track_id) {
                ui_inv.push(UiInvalidation::ExpandedStepViewport {
                    track: viewport.track,
                    track_id,
                });
            }
        }
        Ok(Value::Bool(true))
    });

    let st = state.clone();
    let ct = current_track.clone();
    let delete_target = active_delete_target.clone();
    let delete_target_version = active_delete_target_version.clone();
    let ui_ep = ui_epoch.clone();
    runtime.register_native("seq-delete-active-target", move |_args, ctx| {
        let target = delete_target.lock().unwrap().clone();
        let Some(target) = target else {
            return Ok(Value::Bool(false));
        };

        let current_buffer = ctx.current_buffer_name();
        match target {
            ActiveDeleteTarget::MixerTrack { track } => {
                if current_buffer != "*mixer*" {
                    return Ok(Value::Bool(false));
                }
                let track_count = st.active_track_count();
                if track_count <= 1 {
                    ctx.set_status("Cannot delete the last remaining track");
                    return Ok(Value::Bool(false));
                }
                if track >= track_count {
                    ctx.set_status(format!("Cannot delete missing track {}", track + 1));
                    let mut guard = delete_target.lock().unwrap();
                    if guard.take().is_some() {
                        bump_delete_target_version(&delete_target_version, &ui_ep);
                    }
                    return Ok(Value::Bool(false));
                }
                let mut map = std::collections::HashMap::new();
                map.insert(
                    "track".to_string(),
                    Rc::new(RefCell::new(Value::Number(track as f64))),
                );
                ctx.enqueue_command(HostCommand::Custom {
                    name: "delete-track".to_string(),
                    payload: Value::Map(map),
                });
            }
            ActiveDeleteTarget::ModRoute {
                source,
                dest,
                input,
            } => {
                if current_buffer != "*mixer*" {
                    return Ok(Value::Bool(false));
                }
                let current_pattern = st.pattern.current_pattern.load(Ordering::Relaxed) as usize;
                let route_exists = st
                    .pattern
                    .pattern_bank
                    .lock()
                    .unwrap()
                    .get(current_pattern)
                    .is_some_and(|pattern| {
                        pattern.mod_connections.iter().any(|route| {
                            route.source_track == source
                                && route.dest_track == dest
                                && route.dest_input == input
                        })
                    });
                if !route_exists {
                    let mut guard = delete_target.lock().unwrap();
                    if guard.take().is_some() {
                        bump_delete_target_version(&delete_target_version, &ui_ep);
                    }
                    return Ok(Value::Bool(false));
                }
                let mut map = std::collections::HashMap::new();
                map.insert(
                    "source".to_string(),
                    Rc::new(RefCell::new(Value::Number(source as f64))),
                );
                map.insert(
                    "dest".to_string(),
                    Rc::new(RefCell::new(Value::Number(dest as f64))),
                );
                map.insert(
                    "input".to_string(),
                    Rc::new(RefCell::new(Value::Number(input as f64))),
                );
                ctx.enqueue_command(HostCommand::Custom {
                    name: "delete-mod-route".to_string(),
                    payload: Value::Map(map),
                });
            }
            ActiveDeleteTarget::FxEffect { chain, bus, slot } => {
                if current_buffer != "*fx*" {
                    return Ok(Value::Bool(false));
                }
                let (name, payload) = match chain {
                    FxDeleteChain::Audio => {
                        let track = ct.load(Ordering::Relaxed);
                        let valid = track < st.active_track_count()
                            && slot < st.pattern.effect_chains[track].len();
                        if !valid {
                            ctx.set_status("Cannot delete missing audio effect");
                            let mut guard = delete_target.lock().unwrap();
                            if guard.take().is_some() {
                                bump_delete_target_version(&delete_target_version, &ui_ep);
                            }
                            return Ok(Value::Bool(false));
                        }
                        let mut map = std::collections::HashMap::new();
                        map.insert(
                            "slot".to_string(),
                            Rc::new(RefCell::new(Value::Number(slot as f64))),
                        );
                        ("delete-effect".to_string(), Value::Map(map))
                    }
                    FxDeleteChain::Midi => {
                        let track = ct.load(Ordering::Relaxed);
                        let valid = track < st.active_track_count()
                            && st
                                .pattern
                                .track_params
                                .get(track)
                                .is_some_and(|params| slot < params.midi_fx_chain().len());
                        if !valid {
                            ctx.set_status("Cannot delete missing MIDI effect");
                            let mut guard = delete_target.lock().unwrap();
                            if guard.take().is_some() {
                                bump_delete_target_version(&delete_target_version, &ui_ep);
                            }
                            return Ok(Value::Bool(false));
                        }
                        let mut map = std::collections::HashMap::new();
                        map.insert(
                            "slot".to_string(),
                            Rc::new(RefCell::new(Value::Number(slot as f64))),
                        );
                        ("delete-midi-fx".to_string(), Value::Map(map))
                    }
                    FxDeleteChain::Bus => {
                        let Some(bus) = bus else {
                            return Err("bus fx delete target is missing bus index".into());
                        };
                        let mut map = std::collections::HashMap::new();
                        map.insert(
                            "bus".to_string(),
                            Rc::new(RefCell::new(Value::Number(bus as f64))),
                        );
                        map.insert(
                            "slot".to_string(),
                            Rc::new(RefCell::new(Value::Number(slot as f64))),
                        );
                        ("delete-bus-effect".to_string(), Value::Map(map))
                    }
                };
                ctx.enqueue_command(HostCommand::Custom { name, payload });
            }
        }

        let mut guard = delete_target.lock().unwrap();
        if guard.take().is_some() {
            bump_delete_target_version(&delete_target_version, &ui_ep);
        }
        Ok(Value::Bool(true))
    });

    // seq-toggle-step — toggle step on current track
    let st = state.clone();
    let ct = current_track.clone();
    let sel = selected_steps.clone();
    let auto_follow_override = auto_follow_override_until.clone();
    let fx_ep = fx_epoch.clone();
    let ui_inv = ui_invalidations.clone();
    runtime.register_native("seq-toggle-step", move |args, _ctx| {
        let Some(Value::Number(step)) = args.first() else {
            return Err("seq-toggle-step: expected step number".into());
        };
        let step = *step as usize;
        if step >= MAX_STEPS {
            return Err(format!("seq-toggle-step: step {step} out of range").into());
        }
        let track = ct.load(Ordering::Relaxed);
        st.toggle_step_and_clear_plocks(track, step);
        {
            let mut set = sel.lock().unwrap();
            if !set.is_empty() {
                set.clear();
                fx_ep.fetch_add(1, Ordering::Relaxed);
            }
        }
        *auto_follow_override.lock().unwrap() = Some(Instant::now() + AUTO_FOLLOW_COOLDOWN);
        ui_inv.push(UiInvalidation::Step {
            track,
            step,
            change: StepInvalidation::Active,
        });
        ui_inv.push(UiInvalidation::Step {
            track,
            step,
            change: StepInvalidation::Payload,
        });
        ui_inv.push(UiInvalidation::Step {
            track,
            step,
            change: StepInvalidation::PlockPresence,
        });
        Ok(Value::Bool(st.pattern.patterns[track].is_active(step)))
    });

    // seq-toggle-track-step — toggle a step on a specific track (no track switch)
    let st = state.clone();
    let sel = selected_steps.clone();
    let auto_follow_override = auto_follow_override_until.clone();
    let fx_ep = fx_epoch.clone();
    let ui_inv = ui_invalidations.clone();
    runtime.register_native("seq-toggle-track-step", move |args, _ctx| {
        let (Some(Value::Number(track)), Some(Value::Number(step))) = (args.first(), args.get(1))
        else {
            return Err("seq-toggle-track-step: expected (track step)".into());
        };
        let track = *track as usize;
        let step = *step as usize;
        if track >= st.active_track_count() {
            return Err(format!("seq-toggle-track-step: track {track} out of range").into());
        }
        if step >= MAX_STEPS {
            return Err(format!("seq-toggle-track-step: step {step} out of range").into());
        }
        st.toggle_step_and_clear_plocks(track, step);
        {
            let mut set = sel.lock().unwrap();
            if !set.is_empty() {
                set.clear();
                fx_ep.fetch_add(1, Ordering::Relaxed);
            }
        }
        *auto_follow_override.lock().unwrap() = Some(Instant::now() + AUTO_FOLLOW_COOLDOWN);
        ui_inv.push(UiInvalidation::Step {
            track,
            step,
            change: StepInvalidation::Active,
        });
        ui_inv.push(UiInvalidation::Step {
            track,
            step,
            change: StepInvalidation::Payload,
        });
        ui_inv.push(UiInvalidation::Step {
            track,
            step,
            change: StepInvalidation::PlockPresence,
        });
        Ok(Value::Bool(st.pattern.patterns[track].is_active(step)))
    });

    let st = state.clone();
    runtime.register_native("seq-track-step-active?", move |args, _ctx| {
        let (Some(Value::Number(track)), Some(Value::Number(step))) = (args.first(), args.get(1))
        else {
            return Err("seq-track-step-active?: expected (track step)".into());
        };
        let track = *track as usize;
        let step = *step as usize;
        if track >= st.active_track_count() || step >= MAX_STEPS {
            return Ok(Value::Bool(false));
        }
        Ok(Value::Bool(st.pattern.patterns[track].is_active(step)))
    });

    // seq-set-step-param — set param on current track
    let st = state.clone();
    let ct = current_track.clone();
    let sel = selected_steps.clone();
    let auto_follow_override = auto_follow_override_until.clone();
    let fx_ep = fx_epoch.clone();
    let ui_inv = ui_invalidations.clone();
    runtime.register_native("seq-set-step-param", move |args, _ctx| {
        let (Some(Value::Number(step)), Some(Value::Keyword(param_name)), Some(Value::Number(val))) =
            (args.first(), args.get(1), args.get(2))
        else {
            return Err("seq-set-step-param: expected (step :param value)".into());
        };
        let step = *step as usize;
        if step >= MAX_STEPS {
            return Err(format!("seq-set-step-param: step {step} out of range").into());
        }
        let param = match param_name.as_str() {
            "velocity" | "vel" => StepParam::Velocity,
            "duration" | "dur" => StepParam::Duration,
            "aux-a" | "aux_a" | "auxa" | "axa" => StepParam::AuxA,
            "transpose" => StepParam::Transpose,
            "pan" => StepParam::Pan,
            "sync" | "syn" => StepParam::Sync,
            "delay" | "dly" => StepParam::Delay,
            "speed" => StepParam::Speed,
            other => return Err(format!("seq-set-step-param: unknown param :{other}").into()),
        };
        let track = ct.load(Ordering::Relaxed);
        let val = (*val as f32).clamp(param.min(), param.max());
        {
            let mut set = sel.lock().unwrap();
            if !set.is_empty() && !set.contains(&step) {
                set.clear();
                fx_ep.fetch_add(1, Ordering::Relaxed);
            }
        }
        st.pattern.step_data[track].set(step, param, val);
        st.publish_scheduler_snapshot();
        *auto_follow_override.lock().unwrap() = Some(Instant::now() + AUTO_FOLLOW_COOLDOWN);
        ui_inv.push(UiInvalidation::Step {
            track,
            step,
            change: StepInvalidation::Param(param.into()),
        });
        if param == StepParam::Duration {
            ui_inv.push(UiInvalidation::Step {
                track,
                step,
                change: StepInvalidation::DurationSpan,
            });
        }
        Ok(Value::Number(val as f64))
    });

    let st = state.clone();
    let ct = current_track.clone();
    let piano_sel = piano_roll_selection.clone();
    let piano_move = piano_roll_move_state.clone();
    let piano_clipboard = new_piano_roll_clipboard();
    let auto_follow_override = auto_follow_override_until.clone();
    let ui_inv = ui_invalidations.clone();
    runtime.register_native("seq-piano-roll-action", move |args, ctx| {
        let Some(action) = args.first() else {
            return Err("seq-piano-roll-action: expected action map".into());
        };
        let track = ct.load(Ordering::Relaxed);
        let status = apply_piano_roll_action_with_clipboard(
            &st,
            track,
            &piano_sel,
            &piano_move,
            &piano_clipboard,
            action,
        )?;
        if piano_roll_action_mutates_pattern(action) {
            st.publish_scheduler_snapshot();
            *auto_follow_override.lock().unwrap() = Some(Instant::now() + AUTO_FOLLOW_COOLDOWN);
        }
        ui_inv.push(UiInvalidation::PianoRoll {
            track,
            change: PianoRollInvalidation::Items,
        });
        ctx.set_status(status.clone());
        Ok(Value::String(status))
    });

    // seq-set-track — switch current track
    let st = state.clone();
    let ct = current_track.clone();
    let sel = selected_steps.clone();
    let piano_sel = piano_roll_selection.clone();
    let ui_ep = ui_epoch.clone();
    let fx_ep = fx_epoch.clone();
    let ui_inv = ui_invalidations.clone();
    runtime.register_native("seq-set-track", move |args, _ctx| {
        let Some(Value::Number(track)) = args.first() else {
            return Err("seq-set-track: expected track number".into());
        };
        let track = *track as usize;
        if track >= st.active_track_count() {
            return Err(format!("seq-set-track: track {track} out of range").into());
        }
        let previous = ct.load(Ordering::Relaxed);
        ct.store(track, Ordering::Relaxed);
        if previous != track {
            sel.lock().unwrap().clear();
            piano_sel.lock().unwrap().clear();
            ui_inv.push(UiInvalidation::CurrentTrack {
                previous,
                current: track,
            });
            let next_fx_epoch = fx_ep.fetch_add(1, Ordering::Relaxed) + 1;
            if trace_ui_enabled() {
                eprintln!(
                    "[ui-trace][native] seq-set-track previous={} next={} fx_epoch={}",
                    previous, track, next_fx_epoch
                );
            }
        } else if trace_ui_enabled() {
            eprintln!(
                "[ui-trace][native] seq-set-track unchanged track={} ui_epoch={}",
                track,
                ui_ep.load(Ordering::Relaxed)
            );
        }
        Ok(Value::Number(track as f64))
    });

    // seq-set-track-volume — (seq-set-track-volume track-idx volume)
    let st = state.clone();
    let pan_ids = track_pan_ids.clone();
    let ui_inv = ui_invalidations.clone();
    runtime.register_native("seq-set-track-volume", move |args, _ctx| {
        let (Some(Value::Number(track)), Some(Value::Number(vol))) = (args.first(), args.get(1))
        else {
            return Err("seq-set-track-volume: expected (track volume)".into());
        };
        let track = *track as usize;
        if track >= st.active_track_count() {
            return Err(format!("seq-set-track-volume: track {track} out of range").into());
        }
        let vol = (*vol as f32).clamp(0.0, 1.0);
        st.pattern.track_params[track].set_volume(vol);
        ui_inv.push(UiInvalidation::TrackMixer {
            track,
            change: TrackMixerInvalidation::Volume,
        });
        // Push volume to audiograph's stereo panner node
        let pan_ids_lock = pan_ids.lock().unwrap();
        if let Some(&pan_id) = pan_ids_lock.get(track) {
            unsafe {
                sequencer::audiograph::params_push_wrapper(
                    lg_raw,
                    sequencer::audiograph::ParamMsg {
                        idx: sequencer::stereo_panner::STEREO_PANNER_PARAM_VOLUME,
                        logical_id: pan_id as u64,
                        fvalue: sequencer::mixer_volume::fader_to_gain(vol),
                    },
                );
            }
        }
        Ok(Value::Number(vol as f64))
    });

    // seq-set-track-pan — (seq-set-track-pan track-idx pan)
    let st = state.clone();
    let pan_ids = track_pan_ids.clone();
    let ui_inv = ui_invalidations.clone();
    runtime.register_native("seq-set-track-pan", move |args, _ctx| {
        let (Some(Value::Number(track)), Some(Value::Number(pan))) = (args.first(), args.get(1))
        else {
            return Err("seq-set-track-pan: expected (track pan)".into());
        };
        let track = *track as usize;
        if track >= st.active_track_count() {
            return Err(format!("seq-set-track-pan: track {track} out of range").into());
        }
        let pan = (*pan as f32).clamp(-1.0, 1.0);
        st.pattern.track_params[track].set_pan(pan);
        ui_inv.push(UiInvalidation::TrackMixer {
            track,
            change: TrackMixerInvalidation::Pan,
        });
        let pan_ids_lock = pan_ids.lock().unwrap();
        if let Some(&pan_id) = pan_ids_lock.get(track) {
            unsafe {
                sequencer::audiograph::params_push_wrapper(
                    lg_raw,
                    sequencer::audiograph::ParamMsg {
                        idx: sequencer::stereo_panner::STEREO_PANNER_PARAM_PAN,
                        logical_id: pan_id as u64,
                        fvalue: pan,
                    },
                );
            }
        }
        Ok(Value::Number(pan as f64))
    });

    // seq-toggle-track-mute — (seq-toggle-track-mute track-idx)
    let st = state.clone();
    let pan_ids = track_pan_ids.clone();
    let ui_inv = ui_invalidations.clone();
    runtime.register_native("seq-toggle-track-mute", move |args, _ctx| {
        let Some(Value::Number(track)) = args.first() else {
            return Err("seq-toggle-track-mute: expected track".into());
        };
        let track = *track as usize;
        if track >= st.active_track_count() {
            return Err(format!("seq-toggle-track-mute: track {track} out of range").into());
        }
        let muted = st.pattern.track_params[track].toggle_mute();
        ui_inv.push(UiInvalidation::TrackMixer {
            track,
            change: TrackMixerInvalidation::Mute,
        });
        if trace_ui_enabled() {
            eprintln!(
                "[ui-trace][native] seq-toggle-track-mute track={} muted={}",
                track, muted
            );
        }
        let pan_ids_lock = pan_ids.lock().unwrap();
        if let Some(&pan_id) = pan_ids_lock.get(track) {
            push_panner_bool(
                lg_raw,
                pan_id,
                sequencer::stereo_panner::STEREO_PANNER_PARAM_MUTE,
                muted,
            );
        }
        Ok(Value::Bool(muted))
    });

    // seq-toggle-track-solo — (seq-toggle-track-solo track-idx)
    let st = state.clone();
    let pan_ids = track_pan_ids.clone();
    let ui_inv = ui_invalidations.clone();
    runtime.register_native("seq-toggle-track-solo", move |args, _ctx| {
        let Some(Value::Number(track)) = args.first() else {
            return Err("seq-toggle-track-solo: expected track".into());
        };
        let track = *track as usize;
        if track >= st.active_track_count() {
            return Err(format!("seq-toggle-track-solo: track {track} out of range").into());
        }
        let solo = st.pattern.track_params[track].toggle_solo();
        ui_inv.push(UiInvalidation::TrackMixer {
            track,
            change: TrackMixerInvalidation::Solo,
        });
        for affected_track in 0..st.active_track_count() {
            ui_inv.push(UiInvalidation::TrackMixer {
                track: affected_track,
                change: TrackMixerInvalidation::MutedBySolo,
            });
        }
        if trace_ui_enabled() {
            eprintln!(
                "[ui-trace][native] seq-toggle-track-solo track={} solo={}",
                track, solo
            );
        }
        let pan_ids_lock = pan_ids.lock().unwrap();
        push_solo_mutes(lg_raw, &st, &pan_ids_lock);
        Ok(Value::Bool(solo))
    });

    let bus_state = buses.clone();
    let bus_nodes = bus_node_ids.clone();
    let ui_inv = ui_invalidations.clone();
    runtime.register_native("seq-set-bus-volume", move |args, _ctx| {
        let (Some(Value::Number(bus_idx)), Some(Value::Number(vol))) = (args.first(), args.get(1))
        else {
            return Err("seq-set-bus-volume: expected (bus volume)".into());
        };
        let bus_idx = *bus_idx as usize;
        let vol = (*vol as f32).clamp(0.0, 1.0);
        {
            let mut buses = bus_state.lock().unwrap();
            let Some(bus) = buses.get_mut(bus_idx) else {
                return Err(format!("seq-set-bus-volume: bus {bus_idx} out of range").into());
            };
            bus.volume = vol;
        }
        if let Some(nodes) = bus_nodes.lock().unwrap().get(bus_idx).cloned() {
            unsafe {
                sequencer::audiograph::params_push_wrapper(
                    lg_raw,
                    sequencer::audiograph::ParamMsg {
                        idx: sequencer::stereo_panner::STEREO_PANNER_PARAM_VOLUME,
                        logical_id: nodes.volume_id as u64,
                        fvalue: sequencer::mixer_volume::fader_to_gain(vol),
                    },
                );
            }
        }
        ui_inv.push(UiInvalidation::BusMixer {
            bus: bus_idx,
            change: BusMixerInvalidation::Volume,
        });
        Ok(Value::Number(vol as f64))
    });

    let bus_state = buses.clone();
    let bus_nodes = bus_node_ids.clone();
    let ui_inv = ui_invalidations.clone();
    runtime.register_native("seq-toggle-bus-mute", move |args, _ctx| {
        let Some(Value::Number(bus_idx)) = args.first() else {
            return Err("seq-toggle-bus-mute: expected bus".into());
        };
        let bus_idx = *bus_idx as usize;
        let (muted, volume) = {
            let mut buses = bus_state.lock().unwrap();
            let Some(bus) = buses.get_mut(bus_idx) else {
                return Err(format!("seq-toggle-bus-mute: bus {bus_idx} out of range").into());
            };
            bus.mute = !bus.mute;
            (bus.mute, bus.volume)
        };
        if let Some(nodes) = bus_nodes.lock().unwrap().get(bus_idx).cloned() {
            unsafe {
                sequencer::audiograph::params_push_wrapper(
                    lg_raw,
                    sequencer::audiograph::ParamMsg {
                        idx: sequencer::stereo_panner::STEREO_PANNER_PARAM_VOLUME,
                        logical_id: nodes.volume_id as u64,
                        fvalue: sequencer::mixer_volume::fader_to_gain(volume),
                    },
                );
                sequencer::audiograph::params_push_wrapper(
                    lg_raw,
                    sequencer::audiograph::ParamMsg {
                        idx: sequencer::stereo_panner::STEREO_PANNER_PARAM_MUTE,
                        logical_id: nodes.volume_id as u64,
                        fvalue: if muted { 1.0 } else { 0.0 },
                    },
                );
            }
        }
        ui_inv.push(UiInvalidation::BusMixer {
            bus: bus_idx,
            change: BusMixerInvalidation::Mute,
        });
        Ok(Value::Bool(muted))
    });

    let bus_state = buses.clone();
    let ui_inv = ui_invalidations.clone();
    runtime.register_native("seq-toggle-bus-solo", move |args, _ctx| {
        let Some(Value::Number(bus_idx)) = args.first() else {
            return Err("seq-toggle-bus-solo: expected bus".into());
        };
        let bus_idx = *bus_idx as usize;
        let solo = {
            let mut buses = bus_state.lock().unwrap();
            let Some(bus) = buses.get_mut(bus_idx) else {
                return Err(format!("seq-toggle-bus-solo: bus {bus_idx} out of range").into());
            };
            bus.solo = !bus.solo;
            bus.solo
        };
        ui_inv.push(UiInvalidation::BusMixer {
            bus: bus_idx,
            change: BusMixerInvalidation::Solo,
        });
        Ok(Value::Bool(solo))
    });

    // seq-set-effect-param — (seq-set-effect-param slot-idx param-idx value)
    let st = state.clone();
    let ct = current_track.clone();
    let descs = effect_descriptors.clone();
    let auto_follow_override = auto_follow_override_until.clone();
    let ui_inv = ui_invalidations.clone();
    runtime.register_native("seq-set-effect-param", move |args, _ctx| {
        let (Some(Value::Number(slot)), Some(Value::Number(param)), Some(Value::Number(val))) =
            (args.first(), args.get(1), args.get(2))
        else {
            return Err("seq-set-effect-param: expected (slot param value)".into());
        };
        let track = ct.load(Ordering::Relaxed);
        let slot_idx = *slot as usize;
        let param_idx = *param as usize;
        let val = *val as f32;

        let chain = &st.pattern.effect_chains[track];
        let Some(slot_state) = chain.get(slot_idx) else {
            return Err(format!("seq-set-effect-param: slot {slot_idx} out of range").into());
        };

        // Clamp to descriptor range if available
        let clamped = descs
            .get(track)
            .and_then(|d| d.get(slot_idx))
            .and_then(|d| d.params.get(param_idx))
            .map(|p| val.clamp(p.min, p.max))
            .unwrap_or(val);

        slot_state.defaults.set(param_idx, clamped);

        if let Some((logical_id, idx)) = effect_param_target(slot_state, param_idx) {
            // Check for host_control — skip if present
            let skip = descs
                .get(track)
                .and_then(|d| d.get(slot_idx))
                .and_then(|d| d.params.get(param_idx))
                .and_then(|p| p.host_control.as_ref())
                .is_some();
            if !skip {
                unsafe {
                    sequencer::audiograph::params_push_wrapper(
                        lg_raw,
                        sequencer::audiograph::ParamMsg {
                            idx,
                            logical_id,
                            fvalue: clamped,
                        },
                    );
                }
            }
        }

        // Publish snapshot so the scheduler sees the new default
        // (otherwise it re-applies the old value on next step trigger)
        st.publish_scheduler_snapshot();
        *auto_follow_override.lock().unwrap() = Some(Instant::now() + AUTO_FOLLOW_COOLDOWN);
        ui_inv.push(UiInvalidation::TrackFx {
            track,
            change: TrackFxInvalidation::Param {
                slot: slot_idx,
                param: param_idx,
            },
        });
        Ok(Value::Number(clamped as f64))
    });

    // seq-set-effect-param-pair — (seq-set-effect-param-pair slot-idx param-a value-a param-b value-b)
    let st = state.clone();
    let ct = current_track.clone();
    let descs = effect_descriptors.clone();
    let auto_follow_override = auto_follow_override_until.clone();
    let ui_inv = ui_invalidations.clone();
    runtime.register_native("seq-set-effect-param-pair", move |args, _ctx| {
        let (
            Some(Value::Number(slot)),
            Some(Value::Number(param_a)),
            Some(Value::Number(val_a)),
            Some(Value::Number(param_b)),
            Some(Value::Number(val_b)),
        ) = (
            args.first(),
            args.get(1),
            args.get(2),
            args.get(3),
            args.get(4),
        )
        else {
            return Err(
                "seq-set-effect-param-pair: expected (slot param-a value-a param-b value-b)".into(),
            );
        };
        let track = ct.load(Ordering::Relaxed);
        let slot_idx = *slot as usize;
        let updates = [
            (*param_a as usize, *val_a as f32),
            (*param_b as usize, *val_b as f32),
        ];

        let chain = &st.pattern.effect_chains[track];
        let Some(slot_state) = chain.get(slot_idx) else {
            return Err(format!("seq-set-effect-param-pair: slot {slot_idx} out of range").into());
        };

        let mut clamped_values = Vec::with_capacity(updates.len());
        for (param_idx, val) in updates {
            let clamped = descs
                .get(track)
                .and_then(|d| d.get(slot_idx))
                .and_then(|d| d.params.get(param_idx))
                .map(|p| val.clamp(p.min, p.max))
                .unwrap_or(val);

            slot_state.defaults.set(param_idx, clamped);

            if let Some((logical_id, idx)) = effect_param_target(slot_state, param_idx) {
                let skip = descs
                    .get(track)
                    .and_then(|d| d.get(slot_idx))
                    .and_then(|d| d.params.get(param_idx))
                    .and_then(|p| p.host_control.as_ref())
                    .is_some();
                if !skip {
                    unsafe {
                        sequencer::audiograph::params_push_wrapper(
                            lg_raw,
                            sequencer::audiograph::ParamMsg {
                                idx,
                                logical_id,
                                fvalue: clamped,
                            },
                        );
                    }
                }
            }
            if let Some(name) = descs
                .get(track)
                .and_then(|d| d.get(slot_idx))
                .and_then(|d| d.params.get(param_idx))
                .map(|p| p.name.as_str())
            {
                eseqlisp::reactive::write_float_slot(
                    "SEQ",
                    &track_effect_param_value_field(track, slot_idx, param_idx, name),
                    clamped as f64,
                );
            }
            clamped_values.push(Value::Number(clamped as f64));
            ui_inv.push(UiInvalidation::TrackFx {
                track,
                change: TrackFxInvalidation::Param {
                    slot: slot_idx,
                    param: param_idx,
                },
            });
        }

        st.publish_scheduler_snapshot();
        *auto_follow_override.lock().unwrap() = Some(Instant::now() + AUTO_FOLLOW_COOLDOWN);
        Ok(Value::List(
            clamped_values
                .into_iter()
                .map(|value| Rc::new(RefCell::new(value)))
                .collect(),
        ))
    });

    // seq-set-effect-param-pair-live — push two effect params during a drag without
    // forcing the expensive FX reactive rebuild. The final commit should use
    // seq-set-effect-param-pair so the scheduler/UI snapshot catches up.
    let st = state.clone();
    let ct = current_track.clone();
    let descs = effect_descriptors.clone();
    runtime.register_native("seq-set-effect-param-pair-live", move |args, _ctx| {
        let (
            Some(Value::Number(slot)),
            Some(Value::Number(param_a)),
            Some(Value::Number(val_a)),
            Some(Value::Number(param_b)),
            Some(Value::Number(val_b)),
        ) = (
            args.first(),
            args.get(1),
            args.get(2),
            args.get(3),
            args.get(4),
        )
        else {
            return Err(
                "seq-set-effect-param-pair-live: expected (slot param-a value-a param-b value-b)"
                    .into(),
            );
        };
        let track = ct.load(Ordering::Relaxed);
        let slot_idx = *slot as usize;
        let updates = [
            (*param_a as usize, *val_a as f32),
            (*param_b as usize, *val_b as f32),
        ];

        let chain = &st.pattern.effect_chains[track];
        let Some(slot_state) = chain.get(slot_idx) else {
            return Err(
                format!("seq-set-effect-param-pair-live: slot {slot_idx} out of range").into(),
            );
        };

        let mut clamped_values = Vec::with_capacity(updates.len());
        for (param_idx, val) in updates {
            let clamped = descs
                .get(track)
                .and_then(|d| d.get(slot_idx))
                .and_then(|d| d.params.get(param_idx))
                .map(|p| val.clamp(p.min, p.max))
                .unwrap_or(val);

            slot_state.defaults.set(param_idx, clamped);

            if let Some((logical_id, idx)) = effect_param_target(slot_state, param_idx) {
                let skip = descs
                    .get(track)
                    .and_then(|d| d.get(slot_idx))
                    .and_then(|d| d.params.get(param_idx))
                    .and_then(|p| p.host_control.as_ref())
                    .is_some();
                if !skip {
                    unsafe {
                        sequencer::audiograph::params_push_wrapper(
                            lg_raw,
                            sequencer::audiograph::ParamMsg {
                                idx,
                                logical_id,
                                fvalue: clamped,
                            },
                        );
                    }
                }
            }
            if let Some(name) = descs
                .get(track)
                .and_then(|d| d.get(slot_idx))
                .and_then(|d| d.params.get(param_idx))
                .map(|p| p.name.as_str())
            {
                eseqlisp::reactive::write_float_slot(
                    "SEQ",
                    &track_effect_param_value_field(track, slot_idx, param_idx, name),
                    clamped as f64,
                );
            }
            clamped_values.push(Value::Number(clamped as f64));
        }

        Ok(Value::List(
            clamped_values
                .into_iter()
                .map(|value| Rc::new(RefCell::new(value)))
                .collect(),
        ))
    });

    // ── Selection natives ──

    // seq-select-step — toggle step in/out of selection
    let sel = selected_steps.clone();
    let ct = current_track.clone();
    let ui_inv = ui_invalidations.clone();
    runtime.register_native("seq-select-step", move |args, _ctx| {
        let Some(Value::Number(step)) = args.first() else {
            return Err("seq-select-step: expected step number".into());
        };
        let step = *step as usize;
        let mut set = sel.lock().unwrap();
        let was_selected = !set.insert(step);
        if was_selected {
            set.remove(&step);
        }
        ui_inv.push(UiInvalidation::StepSelection {
            track: ct.load(Ordering::Relaxed),
        });
        Ok(Value::Bool(!was_selected))
    });

    // seq-select-step-range — replace selection with inclusive step range
    let st = state.clone();
    let ct = current_track.clone();
    let sel = selected_steps.clone();
    let ui_inv = ui_invalidations.clone();
    runtime.register_native("seq-select-step-range", move |args, _ctx| {
        let (Some(Value::Number(a)), Some(Value::Number(b))) = (args.first(), args.get(1)) else {
            return Err("seq-select-step-range: expected start and end steps".into());
        };
        let track = ct.load(Ordering::Relaxed);
        let num_steps = st.pattern.track_params[track].get_num_steps();
        if num_steps == 0 {
            return Ok(Value::Number(0.0));
        }
        let a = (*a as usize).min(num_steps - 1);
        let b = (*b as usize).min(num_steps - 1);
        let lo = a.min(b);
        let hi = a.max(b);
        let len = hi - lo + 1;
        let mut set = sel.lock().unwrap();
        if set.len() == len && (lo..=hi).all(|step| set.contains(&step)) {
            return Ok(Value::Number(len as f64));
        }
        set.clear();
        set.extend(lo..=hi);
        ui_inv.push(UiInvalidation::StepSelection { track });
        Ok(Value::Number(len as f64))
    });

    // seq-clear-selection
    let sel = selected_steps.clone();
    let ct = current_track.clone();
    let ui_inv = ui_invalidations.clone();
    runtime.register_native("seq-clear-selection", move |_args, _ctx| {
        let mut selected = sel.lock().unwrap();
        if selected.is_empty() {
            return Ok(Value::Nil);
        }
        selected.clear();
        drop(selected);
        ui_inv.push(UiInvalidation::StepSelection {
            track: ct.load(Ordering::Relaxed),
        });
        Ok(Value::Nil)
    });

    // seq-has-selection?
    let sel = selected_steps.clone();
    runtime.register_native("seq-has-selection?", move |_args, _ctx| {
        Ok(Value::Bool(!sel.lock().unwrap().is_empty()))
    });

    // seq-select-all-steps — select every step in the current track pattern
    let st = state.clone();
    let ct = current_track.clone();
    let sel = selected_steps.clone();
    let ui_inv = ui_invalidations.clone();
    runtime.register_native("seq-select-all-steps", move |_args, _ctx| {
        let track = ct.load(Ordering::Relaxed);
        let num_steps = st.pattern.track_params[track].get_num_steps();
        let mut set = sel.lock().unwrap();
        set.clear();
        set.extend(0..num_steps);
        ui_inv.push(UiInvalidation::StepSelection { track });
        Ok(Value::Number(num_steps as f64))
    });

    // seq-delete-selected-steps — clear all selected steps and clear selection
    let st = state.clone();
    let ct = current_track.clone();
    let sel = selected_steps.clone();
    let auto_follow_override = auto_follow_override_until.clone();
    let ui_inv = ui_invalidations.clone();
    runtime.register_native("seq-delete-selected-steps", move |_args, _ctx| {
        let track = ct.load(Ordering::Relaxed);
        let steps: Vec<usize> = {
            let mut set = sel.lock().unwrap();
            let mut steps: Vec<usize> = set.iter().copied().collect();
            steps.sort_unstable();
            set.clear();
            steps
        };
        for step in &steps {
            st.clear_step_payload(track, *step);
        }
        st.publish_scheduler_snapshot();
        *auto_follow_override.lock().unwrap() = Some(Instant::now() + AUTO_FOLLOW_COOLDOWN);
        ui_inv.push(UiInvalidation::Pattern(PatternInvalidation::WholeTrack {
            track,
        }));
        ui_inv.push(UiInvalidation::StepSelection { track });
        Ok(Value::Number(steps.len() as f64))
    });

    // seq-move-step-drag — move clicked step, or selected steps if clicked step is selected.
    let st = state.clone();
    let ct = current_track.clone();
    let sel = selected_steps.clone();
    let auto_follow_override = auto_follow_override_until.clone();
    let ui_inv = ui_invalidations.clone();
    runtime.register_native("seq-move-step-drag", move |args, _ctx| {
        let (Some(Value::Number(start)), Some(Value::Number(target))) = (args.first(), args.get(1))
        else {
            return Err("seq-move-step-drag: expected start and target steps".into());
        };
        let start = *start as usize;
        let target = *target as usize;
        if start == target {
            return Ok(Value::Bool(false));
        }
        let track = ct.load(Ordering::Relaxed);
        let num_steps = st.pattern.track_params[track].get_num_steps();
        if start >= num_steps || target >= num_steps {
            return Ok(Value::Bool(false));
        }
        let delta = target as isize - start as isize;
        let mut move_selection = false;
        let steps: Vec<usize> = {
            let set = sel.lock().unwrap();
            if set.contains(&start) {
                move_selection = true;
                let mut steps: Vec<usize> = set.iter().copied().collect();
                steps.sort_unstable();
                steps
            } else {
                vec![start]
            }
        };
        if steps.is_empty() {
            return Ok(Value::Bool(false));
        }
        let Some(&first) = steps.first() else {
            return Ok(Value::Bool(false));
        };
        let Some(&last) = steps.last() else {
            return Ok(Value::Bool(false));
        };
        let new_first = first as isize + delta;
        let new_last = last as isize + delta;
        if new_first < 0 || new_last >= num_steps as isize {
            return Ok(Value::Bool(false));
        }
        let snapshots: Vec<(usize, sequencer::sequencer::StepSnapshot)> = steps
            .iter()
            .map(|&step| (step, st.capture_step_snapshot(track, step)))
            .collect();
        for &(step, _) in &snapshots {
            st.clear_step_payload(track, step);
        }
        let moved_steps: Vec<usize> = snapshots
            .iter()
            .map(|(step, _)| (*step as isize + delta) as usize)
            .collect();
        for ((_, snapshot), dst_step) in snapshots.iter().zip(moved_steps.iter().copied()) {
            st.restore_step_snapshot(track, dst_step, snapshot);
        }
        if move_selection {
            let mut set = sel.lock().unwrap();
            set.clear();
            set.extend(moved_steps.iter().copied());
        }
        st.publish_scheduler_snapshot();
        *auto_follow_override.lock().unwrap() = Some(Instant::now() + AUTO_FOLLOW_COOLDOWN);
        ui_inv.push(UiInvalidation::Pattern(PatternInvalidation::WholeTrack {
            track,
        }));
        ui_inv.push(UiInvalidation::StepSelection { track });
        Ok(Value::Bool(true))
    });

    // seq-shift-selected-steps — rotate selected step payloads left/right in place
    let st = state.clone();
    let ct = current_track.clone();
    let sel = selected_steps.clone();
    let auto_follow_override = auto_follow_override_until.clone();
    let ui_inv = ui_invalidations.clone();
    runtime.register_native("seq-shift-selected-steps", move |args, _ctx| {
        let Some(Value::Number(direction)) = args.first() else {
            return Err("seq-shift-selected-steps: expected direction".into());
        };
        let direction = (*direction).round() as isize;
        if direction == 0 {
            return Ok(Value::Nil);
        }
        let track = ct.load(Ordering::Relaxed);
        let steps: Vec<usize> = {
            let set = sel.lock().unwrap();
            let mut steps: Vec<usize> = set.iter().copied().collect();
            steps.sort_unstable();
            steps
        };
        if steps.is_empty() {
            return Ok(Value::Bool(false));
        }
        let num_steps = st.pattern.track_params[track].get_num_steps();
        let delta = direction.signum();
        let can_shift = if delta < 0 {
            steps[0] > 0
        } else {
            steps[steps.len() - 1] + 1 < num_steps
        };
        if !can_shift {
            return Ok(Value::Bool(false));
        }

        let snapshots: Vec<(usize, sequencer::sequencer::StepSnapshot)> = steps
            .iter()
            .map(|&step| (step, st.capture_step_snapshot(track, step)))
            .collect();
        for &(step, _) in &snapshots {
            st.clear_step_payload(track, step);
        }
        let shifted_steps: Vec<usize> = snapshots
            .iter()
            .map(|(step, _)| (*step as isize + delta) as usize)
            .collect();
        for ((_, snapshot), dst_step) in snapshots.iter().zip(shifted_steps.iter().copied()) {
            st.restore_step_snapshot(track, dst_step, snapshot);
        }
        {
            let mut set = sel.lock().unwrap();
            set.clear();
            set.extend(shifted_steps);
        }
        st.publish_scheduler_snapshot();
        *auto_follow_override.lock().unwrap() = Some(Instant::now() + AUTO_FOLLOW_COOLDOWN);
        ui_inv.push(UiInvalidation::Pattern(PatternInvalidation::WholeTrack {
            track,
        }));
        ui_inv.push(UiInvalidation::StepSelection { track });
        Ok(Value::Bool(true))
    });

    // seq-set-effect-plock — apply p-lock to ALL selected steps
    let st = state.clone();
    let ct = current_track.clone();
    let sel = selected_steps.clone();
    let auto_follow_override = auto_follow_override_until.clone();
    let ui_inv = ui_invalidations.clone();
    runtime.register_native("seq-set-effect-plock", move |args, _ctx| {
        let (Some(Value::Number(slot)), Some(Value::Number(param)), Some(Value::Number(val))) =
            (args.first(), args.get(1), args.get(2))
        else {
            return Err("seq-set-effect-plock: expected (slot param value)".into());
        };
        let track = ct.load(Ordering::Relaxed);
        let slot_idx = *slot as usize;
        let param_idx = *param as usize;
        let val = *val as f32;
        let chain = &st.pattern.effect_chains[track];
        let Some(slot_state) = chain.get(slot_idx) else {
            return Err(format!("slot {slot_idx} out of range").into());
        };
        let steps = sel.lock().unwrap();
        for &step in steps.iter() {
            slot_state.plocks.set(step, param_idx, val);
            ui_inv.push(UiInvalidation::Step {
                track,
                step,
                change: StepInvalidation::PlockPresence,
            });
        }
        st.publish_scheduler_snapshot();
        *auto_follow_override.lock().unwrap() = Some(Instant::now() + AUTO_FOLLOW_COOLDOWN);
        ui_inv.push(UiInvalidation::TrackFx {
            track,
            change: TrackFxInvalidation::Plock {
                slot: slot_idx,
                param: param_idx,
            },
        });
        Ok(Value::Number(val as f64))
    });

    // seq-set-effect-plock-pair — apply two effect p-locks to ALL selected steps
    let st = state.clone();
    let ct = current_track.clone();
    let sel = selected_steps.clone();
    let auto_follow_override = auto_follow_override_until.clone();
    let ui_inv = ui_invalidations.clone();
    runtime.register_native("seq-set-effect-plock-pair", move |args, _ctx| {
        let (
            Some(Value::Number(slot)),
            Some(Value::Number(param_a)),
            Some(Value::Number(val_a)),
            Some(Value::Number(param_b)),
            Some(Value::Number(val_b)),
        ) = (
            args.first(),
            args.get(1),
            args.get(2),
            args.get(3),
            args.get(4),
        )
        else {
            return Err(
                "seq-set-effect-plock-pair: expected (slot param-a value-a param-b value-b)".into(),
            );
        };
        let track = ct.load(Ordering::Relaxed);
        let slot_idx = *slot as usize;
        let chain = &st.pattern.effect_chains[track];
        let Some(slot_state) = chain.get(slot_idx) else {
            return Err(format!("slot {slot_idx} out of range").into());
        };
        let updates = [
            (*param_a as usize, *val_a as f32),
            (*param_b as usize, *val_b as f32),
        ];
        let steps = sel.lock().unwrap();
        for &step in steps.iter() {
            for (param_idx, val) in updates {
                slot_state.plocks.set(step, param_idx, val);
                ui_inv.push(UiInvalidation::TrackFx {
                    track,
                    change: TrackFxInvalidation::Plock {
                        slot: slot_idx,
                        param: param_idx,
                    },
                });
            }
            ui_inv.push(UiInvalidation::Step {
                track,
                step,
                change: StepInvalidation::PlockPresence,
            });
        }
        st.publish_scheduler_snapshot();
        *auto_follow_override.lock().unwrap() = Some(Instant::now() + AUTO_FOLLOW_COOLDOWN);
        Ok(Value::Bool(true))
    });

    // seq-set-step-param-plock — apply step param p-lock to selected steps
    let st = state.clone();
    let ct = current_track.clone();
    let sel = selected_steps.clone();
    let auto_follow_override = auto_follow_override_until.clone();
    let ui_inv = ui_invalidations.clone();
    runtime.register_native("seq-set-step-param-plock", move |args, _ctx| {
        let (Some(Value::Keyword(param_name)), Some(Value::Number(val))) =
            (args.first(), args.get(1))
        else {
            return Err("seq-set-step-param-plock: expected (:param value)".into());
        };
        let param = match param_name.as_str() {
            "velocity" | "vel" => StepParam::Velocity,
            "duration" | "dur" => StepParam::Duration,
            "aux-a" | "aux_a" | "auxa" | "axa" => StepParam::AuxA,
            "transpose" => StepParam::Transpose,
            "pan" => StepParam::Pan,
            "sync" | "syn" => StepParam::Sync,
            "delay" | "dly" => StepParam::Delay,
            "speed" => StepParam::Speed,
            other => return Err(format!("unknown param :{other}").into()),
        };
        let track = ct.load(Ordering::Relaxed);
        let val = (*val as f32).clamp(param.min(), param.max());
        let steps = sel.lock().unwrap();
        for &step in steps.iter() {
            st.pattern.step_data[track].set(step, param, val);
            ui_inv.push(UiInvalidation::Step {
                track,
                step,
                change: StepInvalidation::Param(param.into()),
            });
            if param == StepParam::Duration {
                ui_inv.push(UiInvalidation::Step {
                    track,
                    step,
                    change: StepInvalidation::DurationSpan,
                });
            }
        }
        st.publish_scheduler_snapshot();
        *auto_follow_override.lock().unwrap() = Some(Instant::now() + AUTO_FOLLOW_COOLDOWN);
        Ok(Value::Number(val as f64))
    });

    // seq-toggle-play
    let st = state.clone();
    runtime.register_native("seq-toggle-play", move |_args, _ctx| {
        Ok(Value::Bool(st.toggle_play()))
    });

    let st = state.clone();
    runtime.register_native("seq-set-bpm", move |args, _ctx| {
        let Some(Value::Number(bpm)) = args.first() else {
            return Err("seq-set-bpm: expected bpm number".into());
        };
        let bpm = (*bpm as u32).clamp(20, 300);
        st.transport.bpm.store(bpm, Ordering::Relaxed);
        st.publish_scheduler_snapshot();
        Ok(Value::Number(bpm as f64))
    });

    // seq-set-track-param — set a track parameter on the current track
    let st = state.clone();
    let ct = current_track.clone();
    let sel = selected_steps.clone();
    let auto_follow_override = auto_follow_override_until.clone();
    let ui_inv = ui_invalidations.clone();
    runtime.register_native("seq-set-track-param", move |args, _ctx| {
        let (Some(Value::Keyword(param_name)), Some(Value::Number(val))) =
            (args.first(), args.get(1))
        else {
            return Err("seq-set-track-param: expected (:param value)".into());
        };
        let track = ct.load(Ordering::Relaxed);
        let tp = &st.pattern.track_params[track];
        let invalidation = match param_name.as_str() {
            "attack" => {
                let v = (*val as f32).clamp(0.0, 500.0);
                tp.set_attack_ms(v);
                (TrackParamInvalidation::Attack, Ok(Value::Number(v as f64)))
            }
            "release" => {
                let v = (*val as f32).clamp(0.0, 2000.0);
                tp.set_release_ms(v);
                (TrackParamInvalidation::Release, Ok(Value::Number(v as f64)))
            }
            "swing" => {
                let v = (*val as f32).clamp(50.0, 75.0);
                let steps = sel.lock().unwrap();
                if steps.is_empty() {
                    tp.set_swing(v);
                } else {
                    for &step in steps.iter() {
                        st.pattern.swing_plocks[track].set(step, v);
                    }
                }
                (TrackParamInvalidation::Swing, Ok(Value::Number(v as f64)))
            }
            "num-steps" => {
                let v = (*val as usize).clamp(1, MAX_STEPS);
                tp.set_num_steps(v);
                (
                    TrackParamInvalidation::NumSteps,
                    Ok(Value::Number(v as f64)),
                )
            }
            "send" => {
                let v = (*val as f32).clamp(0.0, 1.0);
                tp.set_send(v);
                (TrackParamInvalidation::Send, Ok(Value::Number(v as f64)))
            }
            "gate" => {
                let want_on = *val != 0.0;
                if want_on != tp.is_gate_on() {
                    tp.toggle_gate();
                }
                (
                    TrackParamInvalidation::Gate,
                    Ok(Value::Bool(tp.is_gate_on())),
                )
            }
            "poly" => {
                let want_on = *val != 0.0;
                if want_on != tp.is_polyphonic() {
                    tp.toggle_polyphonic();
                }
                (
                    TrackParamInvalidation::Poly,
                    Ok(Value::Bool(tp.is_polyphonic())),
                )
            }
            "max-poly" | "max-polyphony" | "voices" => {
                tp.set_max_polyphony((*val).round().max(1.0) as usize);
                (
                    TrackParamInvalidation::MaxPolyphony,
                    Ok(Value::Number(tp.get_max_polyphony() as f64)),
                )
            }
            other => return Err(format!("seq-set-track-param: unknown param :{other}").into()),
        };
        let result = invalidation.1;
        result.inspect(|_| {
            st.publish_scheduler_snapshot();
            *auto_follow_override.lock().unwrap() = Some(Instant::now() + AUTO_FOLLOW_COOLDOWN);
            ui_inv.push(UiInvalidation::TrackParam {
                track,
                change: invalidation.0,
            });
        })
    });

    let st = state.clone();
    let ct = current_track.clone();
    let ui_ep = ui_epoch.clone();
    let accumulator_names_for_native = accumulator_names.clone();
    let auto_follow_override = auto_follow_override_until.clone();
    runtime.register_native("seq-set-accumulator", move |args, _ctx| {
        let label = match args.first() {
            Some(Value::String(s)) => s.as_str(),
            _ => return Err("seq-set-accumulator: expected string label".into()),
        };
        let names = accumulator_names_for_native.lock().unwrap();
        let idx = names
            .iter()
            .position(|name| name.eq_ignore_ascii_case(label))
            .ok_or_else(|| format!("seq-set-accumulator: unknown accumulator '{label}'"))?;
        let track = ct.load(Ordering::Relaxed);
        let tp = &st.pattern.track_params[track];
        tp.set_accumulator_idx(idx);
        if idx < BUILTIN_ACCUMULATOR_NAMES.len() {
            tp.set_script_accumulator_name(None);
            tp.set_accum_limit(builtin_accumulator_default_limit(idx));
        } else {
            tp.set_script_accumulator_name(Some(names[idx].clone()));
        }
        st.publish_scheduler_snapshot();
        *auto_follow_override.lock().unwrap() = Some(Instant::now() + AUTO_FOLLOW_COOLDOWN);
        ui_ep.fetch_add(1, Ordering::Relaxed);
        Ok(Value::String(names[idx].clone()))
    });

    let accumulator_names_for_native = accumulator_names.clone();
    let debug_accum_preview = debug_accum;
    runtime.register_native("__register-accumulator-preview", move |args, _ctx| {
        let label = match args.first() {
            Some(Value::String(s)) => s.clone(),
            _ => return Err("__register-accumulator-preview: expected string label".into()),
        };
        let mut names = accumulator_names_for_native.lock().unwrap();
        if !names.iter().any(|name| name.eq_ignore_ascii_case(&label)) {
            names.push(label.clone());
        }
        if debug_accum_preview {
            eprintln!("[accum-ui] preview register label={label} names={names:?}");
        }
        Ok(Value::String(label))
    });
    let _ = runtime.eval_str(
        r#"
        (defmacro def-accumulator (name body)
          `(__register-accumulator-preview ,name))
        "#,
    );

    let midi_fx_names_for_native = midi_fx_names.clone();
    let debug_midi_fx_preview = debug_accum;
    runtime.register_native("__register-midi-fx-preview", move |args, _ctx| {
        let label = match args.first() {
            Some(Value::String(s)) => s.clone(),
            _ => return Err("__register-midi-fx-preview: expected string label".into()),
        };
        let mut names = midi_fx_names_for_native.lock().unwrap();
        if !names.iter().any(|name| name.eq_ignore_ascii_case(&label)) {
            names.push(label.clone());
        }
        if debug_midi_fx_preview {
            eprintln!("[midi-fx-ui] preview register label={label} names={names:?}");
        }
        Ok(Value::String(label))
    });
    let _ = runtime.eval_str(
        r#"
        (defmacro def-midi-fx (name body)
          `(__register-midi-fx-preview ,name))
        "#,
    );

    let st = state.clone();
    let ct = current_track.clone();
    let ui_ep = ui_epoch.clone();
    let accumulator_names_for_native = accumulator_names.clone();
    let auto_follow_override = auto_follow_override_until.clone();
    let debug_accum_use = debug_accum;
    runtime.register_native("seq-use-accumulator", move |args, _ctx| {
        let (track, label) = match args.as_slice() {
            [Value::String(label)] => (ct.load(Ordering::Relaxed), label.clone()),
            [Value::Number(track), Value::String(label)] if *track >= 0.0 => {
                (*track as usize, label.clone())
            }
            _ => {
                return Err(
                    "seq-use-accumulator: expected string label or track/string label".into(),
                );
            }
        };
        if track >= st.active_track_count() {
            return Err("seq-use-accumulator: track out of range".into());
        }

        let mut names = accumulator_names_for_native.lock().unwrap();
        if !names.iter().any(|name| name.eq_ignore_ascii_case(&label)) {
            names.push(label.clone());
        }
        let idx = names
            .iter()
            .position(|name| name.eq_ignore_ascii_case(&label))
            .ok_or_else(|| format!("seq-use-accumulator: unknown accumulator '{label}'"))?;

        let tp = &st.pattern.track_params[track];
        tp.set_accumulator_idx(idx);
        if idx < BUILTIN_ACCUMULATOR_NAMES.len() {
            tp.set_script_accumulator_name(None);
            tp.set_accum_limit(builtin_accumulator_default_limit(idx));
        } else {
            tp.set_script_accumulator_name(Some(names[idx].clone()));
        }
        st.request_accumulator_reset(track);
        st.publish_scheduler_snapshot();
        *auto_follow_override.lock().unwrap() = Some(Instant::now() + AUTO_FOLLOW_COOLDOWN);
        ui_ep.fetch_add(1, Ordering::Relaxed);
        if debug_accum_use {
            eprintln!(
                "[accum-ui] seq-use track={} label={} idx={} script={:?} names={:?}",
                track,
                label,
                idx,
                tp.script_accumulator_name(),
                *names
            );
        }
        Ok(Value::String(names[idx].clone()))
    });

    let st = state.clone();
    let ct = current_track.clone();
    let ui_ep = ui_epoch.clone();
    let midi_fx_names_for_native = midi_fx_names.clone();
    let auto_follow_override = auto_follow_override_until.clone();
    let debug_midi_fx_use = debug_accum;
    runtime.register_native("seq-use-midi-fx", move |args, _ctx| {
        if args.is_empty() {
            return Err("seq-use-midi-fx: expected one or more MIDI FX names".into());
        }
        let (track, start_idx) = match args.first() {
            Some(Value::Number(track)) if *track >= 0.0 => (*track as usize, 1),
            _ => (ct.load(Ordering::Relaxed), 0),
        };
        if track >= st.active_track_count() {
            return Err("seq-use-midi-fx: track out of range".into());
        }
        let mut chain = Vec::new();
        for arg in args.iter().skip(start_idx) {
            match arg {
                Value::String(label) => chain.push(label.clone()),
                _ => return Err("seq-use-midi-fx: expected string MIDI FX names".into()),
            }
        }
        if chain.is_empty() {
            return Err("seq-use-midi-fx: expected at least one MIDI FX name".into());
        }
        let mut names = midi_fx_names_for_native.lock().unwrap();
        for label in &chain {
            if !names.iter().any(|name| name.eq_ignore_ascii_case(label)) {
                names.push(label.clone());
            }
        }
        st.pattern.track_params[track].set_midi_fx_chain(chain.clone());
        st.publish_scheduler_snapshot();
        *auto_follow_override.lock().unwrap() = Some(Instant::now() + AUTO_FOLLOW_COOLDOWN);
        ui_ep.fetch_add(1, Ordering::Relaxed);
        if debug_midi_fx_use {
            eprintln!("[midi-fx-ui] seq-use track={} chain={:?}", track, chain);
        }
        Ok(Value::List(
            chain
                .into_iter()
                .map(|name| Rc::new(RefCell::new(Value::String(name))))
                .collect(),
        ))
    });

    let st = state.clone();
    let ct = current_track.clone();
    let ui_ep = ui_epoch.clone();
    runtime.register_native("seq-clear-midi-fx", move |args, _ctx| {
        let track = match args.first() {
            Some(Value::Number(track)) if *track >= 0.0 => *track as usize,
            None => ct.load(Ordering::Relaxed),
            _ => return Err("seq-clear-midi-fx: expected no args or track".into()),
        };
        if track >= st.active_track_count() {
            return Err("seq-clear-midi-fx: track out of range".into());
        }
        st.pattern.track_params[track].set_midi_fx_chain(Vec::new());
        st.publish_scheduler_snapshot();
        ui_ep.fetch_add(1, Ordering::Relaxed);
        Ok(Value::Bool(true))
    });

    let st = state.clone();
    let ct = current_track.clone();
    let ui_ep = ui_epoch.clone();
    runtime.register_native("seq-set-midi-fx-position", move |args, _ctx| {
        if args.is_empty() {
            return Err("seq-set-midi-fx-position: expected position".into());
        }
        let (track, pos_idx) = match args.first() {
            Some(Value::Number(track)) if *track >= 0.0 => (*track as usize, 1),
            _ => (ct.load(Ordering::Relaxed), 0),
        };
        if track >= st.active_track_count() {
            return Err("seq-set-midi-fx-position: track out of range".into());
        }
        let position = match args.get(pos_idx) {
            Some(Value::Keyword(name)) | Some(Value::String(name))
                if name == "post-accumulator" || name == "post" =>
            {
                MidiFxPosition::PostAccumulator
            }
            Some(Value::Keyword(name)) | Some(Value::String(name))
                if name == "pre-accumulator" || name == "pre" =>
            {
                return Err(
                    "seq-set-midi-fx-position: pre-accumulator is not implemented yet".into(),
                );
            }
            _ => {
                return Err(
                    "seq-set-midi-fx-position: expected :pre-accumulator or :post-accumulator"
                        .into(),
                );
            }
        };
        st.pattern.track_params[track].set_midi_fx_position(position);
        st.publish_scheduler_snapshot();
        ui_ep.fetch_add(1, Ordering::Relaxed);
        Ok(Value::Bool(true))
    });

    let st = state.clone();
    let ct = current_track.clone();
    let ui_ep = ui_epoch.clone();
    let auto_follow_override = auto_follow_override_until.clone();
    runtime.register_native("seq-set-accum-mode", move |args, _ctx| {
        let label = match args.first() {
            Some(Value::String(s)) => s.as_str(),
            _ => return Err("seq-set-accum-mode: expected string label".into()),
        };
        let mode = ACCUM_MODE_LABELS
            .iter()
            .position(|entry: &&str| entry.eq_ignore_ascii_case(label))
            .map(|idx| idx as u32)
            .ok_or_else(|| format!("seq-set-accum-mode: unknown mode '{label}'"))?;
        let track = ct.load(Ordering::Relaxed);
        st.pattern.track_params[track].set_accum_mode(mode);
        st.publish_scheduler_snapshot();
        *auto_follow_override.lock().unwrap() = Some(Instant::now() + AUTO_FOLLOW_COOLDOWN);
        ui_ep.fetch_add(1, Ordering::Relaxed);
        Ok(Value::String(accum_mode_label(mode).to_string()))
    });

    let st = state.clone();
    let ct = current_track.clone();
    let ui_ep = ui_epoch.clone();
    let auto_follow_override = auto_follow_override_until.clone();
    runtime.register_native("seq-set-accum-limit", move |args, _ctx| {
        let Some(Value::Number(limit)) = args.first() else {
            return Err("seq-set-accum-limit: expected number".into());
        };
        let limit = (*limit as f32).clamp(0.0, 127.0);
        let track = ct.load(Ordering::Relaxed);
        st.pattern.track_params[track].set_accum_limit(limit);
        st.publish_scheduler_snapshot();
        *auto_follow_override.lock().unwrap() = Some(Instant::now() + AUTO_FOLLOW_COOLDOWN);
        ui_ep.fetch_add(1, Ordering::Relaxed);
        Ok(Value::Number(limit as f64))
    });

    // seq-double-track-pattern — duplicate current track pattern to double its length
    let st = state.clone();
    let ct = current_track.clone();
    let ui_ep = ui_epoch.clone();
    runtime.register_native("seq-double-track-pattern", move |_args, _ctx| {
        let track = ct.load(Ordering::Relaxed);
        let new_len = st.duplicate_track_pattern(track);
        ui_ep.fetch_add(1, Ordering::Relaxed);
        Ok(Value::Number(new_len as f64))
    });

    // seq-halve-track-pattern — halve current track pattern length
    let st = state.clone();
    let ct = current_track.clone();
    let ui_ep = ui_epoch.clone();
    runtime.register_native("seq-halve-track-pattern", move |_args, _ctx| {
        let track = ct.load(Ordering::Relaxed);
        let new_len = st.halve_track_pattern(track);
        ui_ep.fetch_add(1, Ordering::Relaxed);
        Ok(Value::Number(new_len as f64))
    });

    let ct = current_track.clone();
    runtime.register_native(
        "seq-propagate-current-track-to-all-patterns",
        move |_args, ctx| {
            let track = ct.load(Ordering::Relaxed);
            ctx.enqueue_command(HostCommand::Custom {
                name: "propagate-current-track-to-all-patterns".to_string(),
                payload: Value::Number(track as f64),
            });
            Ok(Value::Bool(true))
        },
    );

    // seq-set-timebase — set the default timebase for the current track (by label string)
    let st = state.clone();
    let ct = current_track.clone();
    let ui_ep = ui_epoch.clone();
    let auto_follow_override = auto_follow_override_until.clone();
    runtime.register_native("seq-set-timebase", move |args, _ctx| {
        let label = match args.first() {
            Some(Value::String(s)) => s.as_str(),
            _ => return Err("seq-set-timebase: expected string label".into()),
        };
        let normalized = label.to_ascii_lowercase();
        let tb = Timebase::LABELS
            .iter()
            .position(|l| l.to_ascii_lowercase() == normalized)
            .map(|i| Timebase::ALL[i])
            .ok_or_else(|| format!("seq-set-timebase: unknown timebase '{label}'"))?;
        let track = ct.load(Ordering::Relaxed);
        st.pattern.track_params[track].set_timebase(tb);
        st.publish_scheduler_snapshot();
        *auto_follow_override.lock().unwrap() = Some(Instant::now() + AUTO_FOLLOW_COOLDOWN);
        ui_ep.fetch_add(1, Ordering::Relaxed);
        Ok(Value::String(tb.label().to_string()))
    });

    let st = state.clone();
    let ct = current_track.clone();
    let ui_ep = ui_epoch.clone();
    let auto_follow_override = auto_follow_override_until.clone();
    runtime.register_native("seq-set-fts", move |args, _ctx| {
        let label = match args.first() {
            Some(Value::String(s)) => s.as_str(),
            _ => return Err("seq-set-fts: expected string label".into()),
        };
        let normalized = label.to_ascii_lowercase();
        let scale_idx = FTS_SCALE_NAMES
            .iter()
            .position(|scale| scale.to_ascii_lowercase() == normalized)
            .ok_or_else(|| format!("seq-set-fts: unknown scale '{label}'"))?;
        let track = ct.load(Ordering::Relaxed);
        st.pattern.track_params[track].set_fts_scale(scale_idx);
        st.publish_scheduler_snapshot();
        *auto_follow_override.lock().unwrap() = Some(Instant::now() + AUTO_FOLLOW_COOLDOWN);
        ui_ep.fetch_add(1, Ordering::Relaxed);
        Ok(Value::String(FTS_SCALE_NAMES[scale_idx].to_string()))
    });

    // seq-plock-timebase — set a timebase p-lock on selected steps
    let st = state.clone();
    let ct = current_track.clone();
    let sel = selected_steps.clone();
    let ui_ep = ui_epoch.clone();
    let auto_follow_override = auto_follow_override_until.clone();
    runtime.register_native("seq-plock-timebase", move |args, _ctx| {
        let label = match args.first() {
            Some(Value::String(s)) => s.as_str(),
            _ => return Err("seq-plock-timebase: expected string label".into()),
        };
        let normalized = label.to_ascii_lowercase();
        let tb = Timebase::LABELS
            .iter()
            .position(|l| l.to_ascii_lowercase() == normalized)
            .map(|i| Timebase::ALL[i])
            .ok_or_else(|| format!("seq-plock-timebase: unknown timebase '{label}'"))?;
        let track = ct.load(Ordering::Relaxed);
        let steps = sel.lock().unwrap();
        for &step in steps.iter() {
            st.pattern.timebase_plocks[track].set(step, tb);
        }
        st.publish_scheduler_snapshot();
        *auto_follow_override.lock().unwrap() = Some(Instant::now() + AUTO_FOLLOW_COOLDOWN);
        ui_ep.fetch_add(1, Ordering::Relaxed);
        Ok(Value::String(tb.label().to_string()))
    });

    // seq-set-swing-resolution — set the default swing resolution for the current track (by label string)
    let st = state.clone();
    let ct = current_track.clone();
    let sel = selected_steps.clone();
    let ui_ep = ui_epoch.clone();
    let auto_follow_override = auto_follow_override_until.clone();
    runtime.register_native("seq-set-swing-resolution", move |args, _ctx| {
        let label = match args.first() {
            Some(Value::String(s)) => s.as_str(),
            _ => return Err("seq-set-swing-resolution: expected string label".into()),
        };
        let normalized = label.to_ascii_lowercase();
        let resolution = SwingResolution::LABELS
            .iter()
            .position(|l| l.to_ascii_lowercase() == normalized)
            .map(|i| SwingResolution::ALL[i])
            .ok_or_else(|| format!("seq-set-swing-resolution: unknown resolution '{label}'"))?;
        let track = ct.load(Ordering::Relaxed);
        let steps = sel.lock().unwrap();
        if steps.is_empty() {
            st.pattern.track_params[track].set_swing_resolution(resolution);
        } else {
            for &step in steps.iter() {
                st.pattern.swing_resolution_plocks[track].set(step, resolution);
            }
        }
        st.publish_scheduler_snapshot();
        *auto_follow_override.lock().unwrap() = Some(Instant::now() + AUTO_FOLLOW_COOLDOWN);
        ui_ep.fetch_add(1, Ordering::Relaxed);
        Ok(Value::String(resolution.label().to_string()))
    });

    let ui_ep = ui_epoch.clone();
    let auto_follow_override = auto_follow_override_until.clone();
    runtime.register_native("seq-pause-auto-follow", move |_args, _ctx| {
        *auto_follow_override.lock().unwrap() = Some(Instant::now() + AUTO_FOLLOW_COOLDOWN);
        ui_ep.fetch_add(1, Ordering::Relaxed);
        Ok(Value::Bool(false))
    });

    // seq-toggle-record — toggle recording mode (requires at least one armed track)
    let rec = recording.clone();
    let ra = record_armed.clone();
    let ui_ep = ui_epoch.clone();
    runtime.register_native("seq-toggle-record", move |_args, _ctx| {
        let any_armed = ra.lock().unwrap().iter().any(|a| *a);
        if any_armed {
            let was = rec.load(Ordering::Relaxed);
            rec.store(!was, Ordering::Relaxed);
            ui_ep.fetch_add(1, Ordering::Relaxed);
            Ok(Value::Bool(!was))
        } else {
            Ok(Value::Bool(false))
        }
    });

    // seq-toggle-record-arm — toggle record arm for a given track index
    let ra = record_armed.clone();
    let rec = recording.clone();
    let ui_ep = ui_epoch.clone();
    runtime.register_native("seq-toggle-record-arm", move |args, _ctx| {
        let Some(Value::Number(track_idx)) = args.first() else {
            return Err("seq-toggle-record-arm: expected track index".into());
        };
        let track = *track_idx as usize;
        let mut armed = ra.lock().unwrap();
        if track < armed.len() {
            armed[track] = !armed[track];
            // If no tracks armed, turn off recording
            if !armed.iter().any(|a| *a) {
                rec.store(false, Ordering::Relaxed);
            }
            ui_ep.fetch_add(1, Ordering::Relaxed);
            Ok(Value::Bool(armed[track]))
        } else {
            Ok(Value::Bool(false))
        }
    });

    let sample_db = Rc::new(
        sequencer::sample_db::SampleDb::open(std::path::Path::new("samples.db"))
            .expect("metal_seq requires crates/sequencer/samples.db for sample browsing"),
    );
    eprintln!("metal_seq: sample db opened");

    let sample_db_for_search = sample_db.clone();
    runtime.register_native("seq-search-samples", move |args, _ctx| {
        let query = match args.first() {
            Some(Value::String(s)) => s.trim(),
            _ => "",
        };
        let rows = sample_db_for_search
            .query_samples_for_browser_limited(&[], (!query.is_empty()).then_some(query), 100)
            .map_err(|error| format!("failed to search samples.db: {error}"))?;
        Ok(Value::List(
            rows.into_iter()
                .map(|row| {
                    let name = row
                        .title
                        .as_deref()
                        .map(str::trim)
                        .filter(|title| !title.is_empty())
                        .unwrap_or(&row.hash)
                        .to_string();
                    Rc::new(RefCell::new(map_value([
                        ("name", Value::String(name)),
                        ("parent", Value::String(row.tags.join(", "))),
                        ("path", Value::String(format!("samples/{}.wav", row.hash))),
                    ])))
                })
                .collect(),
        ))
    });

    let sample_browser = Rc::new(RefCell::new(DebouncedSampleBrowser::new(
        sample_db.clone(),
        Duration::from_millis(150),
    )));
    let sample_browser_for_native = sample_browser.clone();
    runtime.register_native("seq-sample-browser", move |args, _ctx| {
        let query = match args.first() {
            Some(Value::String(s)) => s.as_str(),
            _ => "",
        };
        let selected_tags = value_string_list(args.get(1));
        let selected_tag_refs: Vec<&str> = selected_tags.iter().map(String::as_str).collect();
        sample_browser_for_native
            .borrow_mut()
            .query(query, &selected_tag_refs)
            .map_err(|error| format!("failed to query samples.db browser state: {error}"))
    });

    let sample_db_for_tree = sample_db.clone();
    runtime.register_native("seq-sample-tree", move |_args, _ctx| {
        build_sample_tree_value_from_db(&sample_db_for_tree, "", &[], &[])
            .map_err(|error| format!("failed to query samples.db sample tree: {error}"))
    });
    let sample_db_for_filter = sample_db.clone();
    runtime.register_native("seq-filter-sample-tree", move |args, _ctx| {
        let query = match args.first() {
            Some(Value::String(s)) => s.trim(),
            _ => "",
        };
        build_sample_tree_value_from_db(&sample_db_for_filter, query, &[], &[])
            .map_err(|error| format!("failed to filter samples.db sample tree: {error}"))
    });
    let sample_db_for_tags = sample_db.clone();
    runtime.register_native("seq-sample-tags-for-path", move |args, _ctx| {
        let path = match args.first() {
            Some(Value::String(path)) => std::path::Path::new(path),
            _ => return Ok(Value::List(vec![])),
        };
        let Some(hash) = path.file_stem().and_then(|stem| stem.to_str()) else {
            return Ok(Value::List(vec![]));
        };
        let tags = sample_db_for_tags
            .tags_for(hash)
            .map_err(|error| format!("failed to query sample tags for {hash}: {error}"))?;
        Ok(build_string_list(&tags))
    });
    runtime.register_native("seq-project-tree", move |args, _ctx| {
        let query = match args.first() {
            Some(Value::String(s)) => s.clone(),
            _ => String::new(),
        };
        Ok(build_project_tree(&query))
    });
    runtime.register_native("seq-preset-tree", move |args, _ctx| {
        let query = match args.get(1) {
            Some(Value::String(s)) => s.as_str(),
            _ => "",
        };
        Ok(build_preset_tree_from_list(args.first(), query))
    });
    runtime.register_native("seq-saved-instruments", move |_args, _ctx| {
        Ok(Value::List(
            sequencer::lisp_effect::list_saved_instruments()
                .into_iter()
                .map(|name| Rc::new(RefCell::new(Value::String(name))))
                .collect(),
        ))
    });
    runtime.register_native("seq-saved-instrument-tree", move |args, _ctx| {
        let query = match args.first() {
            Some(Value::String(s)) => s.as_str(),
            _ => "",
        };
        Ok(build_instrument_tree_value(query))
    });
    runtime.register_native("seq-audio-effect-tree", move |args, _ctx| {
        let query = match args.first() {
            Some(Value::String(s)) => s.as_str(),
            _ => "",
        };
        Ok(build_audio_effect_tree(query))
    });
    runtime.register_native("seq-midi-effect-tree", move |args, _ctx| {
        let query = match args.first() {
            Some(Value::String(s)) => s.as_str(),
            _ => "",
        };
        Ok(build_midi_effect_tree(query))
    });
    register_agent_mode_natives(&mut runtime, app.agent_store.clone());
    document_metal_seq_natives(&mut runtime);

    RuntimeInit {
        runtime,
        accumulator_names,
        midi_fx_names,
        sample_browser,
    }
}

fn register_agent_mode_natives(
    runtime: &mut Runtime,
    store: sequencer::agent::store::ConversationStore,
) {
    let s = store.clone();
    runtime.register_native("agent/new", move |args, _ctx| {
        let kind = parse_agent_kind(&args).unwrap_or(sequencer::agent::store::AgentKind::General);
        let id = s.new_conversation(kind);
        eprintln!("[agent-ui] agent/new kind={kind:?} conv={id}");
        Ok(Value::Number(id as f64))
    });

    let s = store.clone();
    runtime.register_native("agent/list", move |_args, _ctx| {
        Ok(Value::List(
            s.list()
                .into_iter()
                .map(|id| Rc::new(RefCell::new(Value::Number(id as f64))))
                .collect(),
        ))
    });

    let s = store.clone();
    runtime.register_native("agent/send", move |args, ctx| {
        let id = conv_id_arg(args.first())?;
        let prompt = match args.get(1) {
            Some(Value::String(value)) => value.clone(),
            _ => return Err("agent/send: expected conv-id and prompt string".to_string()),
        };
        eprintln!(
            "[agent-ui] agent/send conv={id} prompt_len={} prompt={:?}",
            prompt.len(),
            prompt
        );
        if s.snapshot(id).is_none() {
            return Err(format!("unknown agent conversation {id}"));
        }
        let mut payload = std::collections::HashMap::new();
        payload.insert(
            "id".to_string(),
            Rc::new(RefCell::new(Value::Number(id as f64))),
        );
        payload.insert(
            "prompt".to_string(),
            Rc::new(RefCell::new(Value::String(prompt))),
        );
        ctx.enqueue_command(HostCommand::Custom {
            name: "agent-send".to_string(),
            payload: Value::Map(payload),
        });
        Ok(Value::Nil)
    });

    let s = store.clone();
    runtime.register_native("agent/cancel", move |args, _ctx| {
        let id = conv_id_arg(args.first())?;
        eprintln!("[agent-ui] agent/cancel conv={id}");
        s.cancel(id)?;
        Ok(Value::Nil)
    });

    let s = store.clone();
    runtime.register_native("agent/discard", move |args, _ctx| {
        let id = conv_id_arg(args.first())?;
        eprintln!("[agent-ui] agent/discard conv={id}");
        s.discard(id)?;
        Ok(Value::Nil)
    });

    let s = store.clone();
    runtime.register_native("agent/close", move |args, _ctx| {
        let id = conv_id_arg(args.first())?;
        eprintln!("[agent-ui] agent/close conv={id}");
        s.close(id);
        Ok(Value::Nil)
    });

    let s = store.clone();
    runtime.register_native("agent/accept", move |args, ctx| {
        let id = conv_id_arg(args.first())?;
        eprintln!("[agent-ui] agent/accept conv={id}");
        ctx.enqueue_command(HostCommand::Custom {
            name: "agent-accept".to_string(),
            payload: Value::Number(id as f64),
        });
        // The host command performs graph mutation and returns status asynchronously.
        let _ = &s;
        Ok(Value::Number(id as f64))
    });

    runtime.register_native("agent/finalize", move |args, ctx| {
        let id = conv_id_arg(args.first())?;
        let name = match args.get(1) {
            Some(Value::String(value)) if !value.trim().is_empty() => value.clone(),
            _ => return Err("agent/finalize: expected conv-id and name string".to_string()),
        };
        eprintln!("[agent-ui] agent/finalize conv={id} name={name:?}");
        let mut payload = std::collections::HashMap::new();
        payload.insert(
            "id".to_string(),
            Rc::new(RefCell::new(Value::Number(id as f64))),
        );
        payload.insert(
            "name".to_string(),
            Rc::new(RefCell::new(Value::String(name))),
        );
        ctx.enqueue_command(HostCommand::Custom {
            name: "agent-finalize".to_string(),
            payload: Value::Map(payload),
        });
        Ok(Value::Number(id as f64))
    });

    let s = store.clone();
    runtime.register_native("agent/artifact", move |args, _ctx| {
        let state = snapshot_state(&s, args.first())?;
        Ok(agent_artifact_value(state))
    });

    let s = store.clone();
    runtime.register_native("agent/kind", move |args, _ctx| {
        let state = snapshot_state(&s, args.first())?;
        Ok(Value::Symbol(agent_kind_symbol(state.kind).to_string()))
    });

    let s = store.clone();
    runtime.register_native("agent/status", move |args, _ctx| {
        let state = snapshot_state(&s, args.first())?;
        Ok(Value::Symbol(agent_status_symbol(state.status).to_string()))
    });

    let s = store.clone();
    runtime.register_native("agent/messages", move |args, _ctx| {
        let state = snapshot_state(&s, args.first())?;
        Ok(Value::List(
            state
                .messages
                .into_iter()
                .map(agent_message_value)
                .map(|value| Rc::new(RefCell::new(value)))
                .collect(),
        ))
    });

    let s = store.clone();
    runtime.register_native("agent/draft-source", move |args, _ctx| {
        let state = snapshot_state(&s, args.first())?;
        Ok(state
            .draft
            .map(|draft| Value::String(draft.dsp_source))
            .or_else(|| {
                state
                    .effect_draft
                    .map(|draft| Value::String(draft.dsp_source))
            })
            .unwrap_or(Value::Nil))
    });

    let s = store.clone();
    runtime.register_native("agent/draft-ui-source", move |args, _ctx| {
        let state = snapshot_state(&s, args.first())?;
        Ok(state
            .draft
            .map(|draft| Value::String(draft.ui_source))
            .or_else(|| {
                state
                    .effect_draft
                    .map(|draft| Value::String(draft.ui_source))
            })
            .unwrap_or(Value::Nil))
    });

    let s = store.clone();
    runtime.register_native("agent/draft-handle", move |args, _ctx| {
        let state = snapshot_state(&s, args.first())?;
        Ok(if state.draft_handle.is_some() {
            Value::Number(state.id as f64)
        } else {
            Value::Nil
        })
    });

    let s = store.clone();
    runtime.register_native("agent/last-error", move |args, _ctx| {
        let state = snapshot_state(&s, args.first())?;
        Ok(state
            .last_compile_error
            .map(Value::String)
            .unwrap_or(Value::Nil))
    });

    let s = store.clone();
    runtime.register_native("agent/last-audition", move |args, _ctx| {
        let state = snapshot_state(&s, args.first())?;
        Ok(state
            .last_audition
            .map(agent_audition_value)
            .unwrap_or(Value::Nil))
    });

    let s = store.clone();
    runtime.register_native("agent/generation", move |args, _ctx| {
        let state = snapshot_state(&s, args.first())?;
        Ok(Value::Number(state.generation as f64))
    });

    runtime.register_native("agent/models", move |_args, _ctx| {
        Ok(Value::List(
            sequencer::agent::providers::default_model_presets()
                .into_iter()
                .map(|model| Rc::new(RefCell::new(Value::String(model.id))))
                .collect(),
        ))
    });

    let s = store.clone();
    runtime.register_native("agent/model", move |args, _ctx| {
        let state = snapshot_state(&s, args.first())?;
        Ok(Value::String(state.model))
    });

    let s = store;
    runtime.register_native("agent/set-model", move |args, _ctx| {
        let id = conv_id_arg(args.first())?;
        let model = match args.get(1) {
            Some(Value::String(value)) => value.clone(),
            _ => return Err("agent/set-model: expected conv-id and model string".to_string()),
        };
        let provider = sequencer::agent::providers::default_model_presets()
            .into_iter()
            .find(|preset| preset.id == model)
            .map(|preset| preset.provider)
            .unwrap_or(sequencer::agent::providers::AgentProviderKind::OpenAi);
        s.set_model(id, provider, model)?;
        Ok(Value::Nil)
    });
}

fn conv_id_arg(value: Option<&Value>) -> Result<sequencer::agent::store::ConvId, String> {
    match value {
        Some(Value::Number(id)) if *id >= 1.0 => Ok(*id as sequencer::agent::store::ConvId),
        _ => Err("expected agent conversation id".to_string()),
    }
}

fn snapshot_state(
    store: &sequencer::agent::store::ConversationStore,
    value: Option<&Value>,
) -> Result<sequencer::agent::store::ConversationState, String> {
    let id = conv_id_arg(value)?;
    store
        .snapshot(id)
        .map(|snapshot| snapshot.state)
        .ok_or_else(|| format!("unknown agent conversation {id}"))
}

fn parse_agent_kind(args: &[Value]) -> Option<sequencer::agent::store::AgentKind> {
    let candidate = args.iter().rev().find_map(|value| match value {
        Value::String(value) | Value::Symbol(value) | Value::Keyword(value) => Some(value.as_str()),
        _ => None,
    })?;
    match candidate.trim_start_matches(':') {
        "instrument" => Some(sequencer::agent::store::AgentKind::Instrument),
        "effect" => Some(sequencer::agent::store::AgentKind::Effect),
        "general" => Some(sequencer::agent::store::AgentKind::General),
        _ => None,
    }
}

fn agent_kind_symbol(kind: sequencer::agent::store::AgentKind) -> &'static str {
    match kind {
        sequencer::agent::store::AgentKind::General => "general",
        sequencer::agent::store::AgentKind::Instrument => "instrument",
        sequencer::agent::store::AgentKind::Effect => "effect",
    }
}

fn agent_status_symbol(status: sequencer::agent::store::AgentStatus) -> &'static str {
    match status {
        sequencer::agent::store::AgentStatus::Idle => "idle",
        sequencer::agent::store::AgentStatus::Streaming => "streaming",
        sequencer::agent::store::AgentStatus::Compiling => "compiling",
        sequencer::agent::store::AgentStatus::Auditioning => "auditioning",
        sequencer::agent::store::AgentStatus::Error => "error",
        sequencer::agent::store::AgentStatus::Cancelled => "cancelled",
    }
}

fn agent_role_symbol(role: sequencer::agent::store::Role) -> &'static str {
    match role {
        sequencer::agent::store::Role::User => "user",
        sequencer::agent::store::Role::Assistant => "assistant",
        sequencer::agent::store::Role::System => "system",
        sequencer::agent::store::Role::Tool => "tool",
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct AgentMessageDisplay {
    text: String,
    code_block_count: usize,
}

fn agent_message_display_text(text: &str) -> AgentMessageDisplay {
    let mut visible_lines = Vec::new();
    let mut in_fenced_block = false;
    let mut code_block_count = 0;

    for line in text.lines() {
        if line.trim_start().starts_with("```") {
            if !in_fenced_block {
                code_block_count += 1;
            }
            in_fenced_block = !in_fenced_block;
            continue;
        }

        if !in_fenced_block {
            visible_lines.push(line);
        }
    }

    let mut display = visible_lines.join("\n");
    display = display.trim().to_string();

    if code_block_count > 0 {
        if display.is_empty() {
            display = "Generated instrument source.".to_string();
        }
    }

    AgentMessageDisplay {
        text: display,
        code_block_count,
    }
}

fn agent_message_value(message: sequencer::agent::store::Message) -> Value {
    let display = agent_message_display_text(&message.text);
    let mut map = std::collections::HashMap::new();
    map.insert(
        "role".to_string(),
        Rc::new(RefCell::new(Value::Symbol(
            agent_role_symbol(message.role).to_string(),
        ))),
    );
    map.insert(
        "text".to_string(),
        Rc::new(RefCell::new(Value::String(message.text))),
    );
    map.insert(
        "display-text".to_string(),
        Rc::new(RefCell::new(Value::String(display.text))),
    );
    map.insert(
        "has-code-blocks".to_string(),
        Rc::new(RefCell::new(Value::Bool(display.code_block_count > 0))),
    );
    map.insert(
        "code-block-count".to_string(),
        Rc::new(RefCell::new(Value::Number(display.code_block_count as f64))),
    );
    let ts = message
        .ts
        .duration_since(std::time::SystemTime::UNIX_EPOCH)
        .map(|duration| duration.as_secs_f64())
        .unwrap_or(0.0);
    map.insert("ts".to_string(), Rc::new(RefCell::new(Value::Number(ts))));
    Value::Map(map)
}

fn agent_artifact_value(state: sequencer::agent::store::ConversationState) -> Value {
    fn display_name(name: &str) -> String {
        let trimmed = name.trim_end_matches('/');
        std::path::Path::new(trimmed)
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or(trimmed)
            .trim_end_matches(".lisp")
            .to_string()
    }

    let target_name = state
        .accepted_instrument_target
        .as_ref()
        .map(|target| target.instrument_name.clone());
    let stub_name = state
        .stub_instrument_target
        .as_ref()
        .map(|target| target.instrument_name.clone());
    let instrument_has_unapplied_draft =
        state.draft.is_some() && state.finalized_instrument_name.is_none();
    let draft_name = state
        .draft
        .as_ref()
        .map(|_| format!("agent-draft-{}/", state.id));
    let instrument_name = state
        .finalized_instrument_name
        .clone()
        .or(target_name)
        .or(draft_name)
        .or(stub_name);
    let effect_name = state
        .finalized_effect_name
        .clone()
        .or_else(|| state.effect_draft.as_ref().map(|draft| draft.name.clone()));
    let artifact_name = match state.kind {
        sequencer::agent::store::AgentKind::Instrument => instrument_name.clone(),
        sequencer::agent::store::AgentKind::Effect => effect_name.clone(),
        sequencer::agent::store::AgentKind::General => {
            instrument_name.clone().or_else(|| effect_name.clone())
        }
    };
    let unapplied_effect_draft = state.effect_draft.is_some()
        && !state.effect_draft_applied
        && state.finalized_effect_name.is_none();

    let mut map = std::collections::HashMap::new();
    let exists = artifact_name.is_some();
    map.insert(
        "exists".to_string(),
        Rc::new(RefCell::new(Value::Bool(exists))),
    );
    map.insert(
        "name".to_string(),
        Rc::new(RefCell::new(
            artifact_name
                .clone()
                .map(Value::String)
                .unwrap_or(Value::Nil),
        )),
    );
    map.insert(
        "display-name".to_string(),
        Rc::new(RefCell::new(
            artifact_name
                .as_ref()
                .map(|name| Value::String(display_name(name)))
                .unwrap_or(Value::Nil),
        )),
    );
    let status = match state.kind {
        sequencer::agent::store::AgentKind::Instrument => {
            if state.finalized_instrument_name.is_some() {
                "saved"
            } else if instrument_has_unapplied_draft && state.accepted_instrument_target.is_some() {
                "updated"
            } else if instrument_has_unapplied_draft {
                "ready"
            } else if state.accepted_instrument_target.is_some() {
                "applied"
            } else if state.stub_instrument_target.is_some() {
                "working"
            } else {
                "none"
            }
        }
        sequencer::agent::store::AgentKind::Effect => {
            if state.finalized_effect_name.is_some() {
                "saved"
            } else if unapplied_effect_draft && state.accepted_effect_target.is_some() {
                "updated"
            } else if unapplied_effect_draft {
                "ready"
            } else if state.accepted_effect_target.is_some() {
                "applied"
            } else {
                "none"
            }
        }
        sequencer::agent::store::AgentKind::General => {
            if state.finalized_instrument_name.is_some() || state.finalized_effect_name.is_some() {
                "saved"
            } else if instrument_has_unapplied_draft && state.accepted_instrument_target.is_some() {
                "updated"
            } else if unapplied_effect_draft && state.accepted_effect_target.is_some() {
                "updated"
            } else if instrument_has_unapplied_draft || unapplied_effect_draft {
                "ready"
            } else if state.accepted_instrument_target.is_some()
                || state.accepted_effect_target.is_some()
            {
                "applied"
            } else if state.stub_instrument_target.is_some() {
                "working"
            } else {
                "none"
            }
        }
    };
    map.insert(
        "status".to_string(),
        Rc::new(RefCell::new(Value::Symbol(status.to_string()))),
    );
    map.insert(
        "track".to_string(),
        Rc::new(RefCell::new(match state.kind {
            sequencer::agent::store::AgentKind::Instrument => state
                .accepted_instrument_target
                .as_ref()
                .or(state.stub_instrument_target.as_ref())
                .map(|target| Value::Number((target.track_index + 1) as f64))
                .unwrap_or(Value::Nil),
            sequencer::agent::store::AgentKind::Effect => state
                .accepted_effect_target
                .as_ref()
                .map(|target| Value::Number((target.track_index + 1) as f64))
                .unwrap_or(Value::Nil),
            sequencer::agent::store::AgentKind::General => state
                .accepted_instrument_target
                .as_ref()
                .or(state.stub_instrument_target.as_ref())
                .map(|target| Value::Number((target.track_index + 1) as f64))
                .or_else(|| {
                    state
                        .accepted_effect_target
                        .as_ref()
                        .map(|target| Value::Number((target.track_index + 1) as f64))
                })
                .unwrap_or(Value::Nil),
        })),
    );
    map.insert(
        "has-draft".to_string(),
        Rc::new(RefCell::new(Value::Bool(
            state.draft.is_some() || state.effect_draft.is_some(),
        ))),
    );
    map.insert(
        "can-finalize".to_string(),
        Rc::new(RefCell::new(Value::Bool(match state.kind {
            sequencer::agent::store::AgentKind::Instrument => {
                state.accepted_instrument_target.is_some()
                    && state.finalized_instrument_name.is_none()
            }
            sequencer::agent::store::AgentKind::Effect => {
                (state.effect_draft.is_some() || state.accepted_effect_target.is_some())
                    && state.finalized_effect_name.is_none()
            }
            sequencer::agent::store::AgentKind::General => {
                (state.accepted_instrument_target.is_some()
                    && state.finalized_instrument_name.is_none())
                    || ((state.effect_draft.is_some() || state.accepted_effect_target.is_some())
                        && state.finalized_effect_name.is_none())
            }
        }))),
    );
    map.insert(
        "can-apply".to_string(),
        Rc::new(RefCell::new(Value::Bool(match state.kind {
            sequencer::agent::store::AgentKind::Instrument => instrument_has_unapplied_draft,
            sequencer::agent::store::AgentKind::Effect => unapplied_effect_draft,
            sequencer::agent::store::AgentKind::General => {
                instrument_has_unapplied_draft || unapplied_effect_draft
            }
        }))),
    );
    map.insert(
        "apply-label".to_string(),
        Rc::new(RefCell::new(Value::String(
            if (instrument_has_unapplied_draft && state.accepted_instrument_target.is_some())
                || (unapplied_effect_draft && state.accepted_effect_target.is_some())
            {
                "Update artifact"
            } else {
                "Apply artifact"
            }
            .to_string(),
        ))),
    );
    Value::Map(map)
}

fn agent_audition_value(audition: sequencer::agent::store::AuditionResult) -> Value {
    let mut map = std::collections::HashMap::new();
    map.insert(
        "ran".to_string(),
        Rc::new(RefCell::new(Value::Bool(audition.ran))),
    );
    map.insert(
        "peak-db".to_string(),
        Rc::new(RefCell::new(Value::Number(audition.peak_db as f64))),
    );
    map.insert(
        "rms-db".to_string(),
        Rc::new(RefCell::new(Value::Number(audition.rms_db as f64))),
    );
    map.insert(
        "clipped".to_string(),
        Rc::new(RefCell::new(Value::Bool(audition.clipped))),
    );
    map.insert(
        "duration-ms".to_string(),
        Rc::new(RefCell::new(Value::Number(audition.duration_ms as f64))),
    );
    map.insert(
        "silent".to_string(),
        Rc::new(RefCell::new(Value::Bool(audition.silent))),
    );
    map.insert(
        "differs-from-input".to_string(),
        Rc::new(RefCell::new(
            audition
                .differs_from_input
                .map(Value::Bool)
                .unwrap_or(Value::Nil),
        )),
    );
    map.insert(
        "diff-rms-db".to_string(),
        Rc::new(RefCell::new(
            audition
                .diff_rms_db
                .map(|value| Value::Number(value as f64))
                .unwrap_or(Value::Nil),
        )),
    );
    Value::Map(map)
}

fn document_metal_seq_natives(runtime: &mut Runtime) {
    runtime.document_symbols([
        (
            "seq-toggle-step",
            "(seq-toggle-step step)",
            "Toggle the current track's step on/off and clear that step's p-locks.",
        ),
        (
            "seq-toggle-track-step",
            "(seq-toggle-track-step track step)",
            "Toggle a step on a specific track without changing the current track.",
        ),
        (
            "seq-set-step-param",
            "(seq-set-step-param step :param value)",
            "Set a per-step parameter on the current track.",
        ),
        (
            "seq-piano-roll-action",
            "(seq-piano-roll-action action-map)",
            "Apply a piano-roll edit action to the current track.",
        ),
        (
            "seq-set-track",
            "(seq-set-track track)",
            "Select the current track by 0-based index.",
        ),
        (
            "seq-set-delete-target",
            "(seq-set-delete-target kind payload)",
            "Set the active destructive keyboard target, such as :mixer-track, :mod-route, or :fx-effect.",
        ),
        (
            "seq-clear-delete-target",
            "(seq-clear-delete-target)",
            "Clear the active destructive keyboard target.",
        ),
        (
            "seq-delete-target?",
            "(seq-delete-target? kind payload)",
            "Return true when the active destructive keyboard target matches kind and payload.",
        ),
        (
            "seq-active-delete-target-kind",
            "(seq-active-delete-target-kind)",
            "Return the active destructive keyboard target kind, or false when none is active.",
        ),
        (
            "seq-delete-active-target",
            "(seq-delete-active-target)",
            "Delete the active destructive keyboard target when valid for the current buffer.",
        ),
        (
            "seq-set-track-volume",
            "(seq-set-track-volume track volume)",
            "Set a track's mixer volume and update its audio panner.",
        ),
        (
            "seq-toggle-track-mute",
            "(seq-toggle-track-mute track)",
            "Toggle a track's mute state.",
        ),
        (
            "seq-toggle-track-solo",
            "(seq-toggle-track-solo track)",
            "Toggle a track's solo state and update solo mute routing.",
        ),
        (
            "seq-set-bus-volume",
            "(seq-set-bus-volume bus volume)",
            "Set a bus mixer volume and update its bus gain nodes.",
        ),
        (
            "seq-toggle-bus-mute",
            "(seq-toggle-bus-mute bus)",
            "Toggle a bus mute state.",
        ),
        (
            "seq-toggle-bus-solo",
            "(seq-toggle-bus-solo bus)",
            "Toggle a bus solo state.",
        ),
        (
            "seq-set-effect-param",
            "(seq-set-effect-param slot param value)",
            "Set an effect parameter default on the current track.",
        ),
        (
            "seq-set-effect-param-pair",
            "(seq-set-effect-param-pair slot param-a value-a param-b value-b)",
            "Set two effect parameter defaults on the current track with one UI/FX invalidation.",
        ),
        (
            "seq-set-effect-param-pair-live",
            "(seq-set-effect-param-pair-live slot param-a value-a param-b value-b)",
            "Push two effect parameter defaults during a drag without rebuilding reactive FX state.",
        ),
        (
            "seq-select-step",
            "(seq-select-step step)",
            "Toggle a step in the current selection.",
        ),
        (
            "seq-select-step-range",
            "(seq-select-step-range start end)",
            "Replace the selection with an inclusive step range.",
        ),
        (
            "seq-clear-selection",
            "(seq-clear-selection)",
            "Clear the selected steps.",
        ),
        (
            "seq-has-selection?",
            "(seq-has-selection?)",
            "Return true when one or more steps are selected.",
        ),
        (
            "seq-select-all-steps",
            "(seq-select-all-steps)",
            "Select all steps in the current track pattern.",
        ),
        (
            "seq-delete-selected-steps",
            "(seq-delete-selected-steps)",
            "Clear all selected step payloads and clear the selection.",
        ),
        (
            "seq-move-step-drag",
            "(seq-move-step-drag start target)",
            "Move a step payload or the selected step payloads by drag delta.",
        ),
        (
            "seq-shift-selected-steps",
            "(seq-shift-selected-steps direction)",
            "Shift selected step payloads left or right by one step.",
        ),
        (
            "seq-set-effect-plock",
            "(seq-set-effect-plock slot param value)",
            "Set an effect parameter p-lock on each selected step.",
        ),
        (
            "seq-set-effect-plock-pair",
            "(seq-set-effect-plock-pair slot param-a value-a param-b value-b)",
            "Set two effect parameter p-locks on each selected step with one UI/FX invalidation.",
        ),
        (
            "seq-set-step-param-plock",
            "(seq-set-step-param-plock :param value)",
            "Set a step parameter p-lock on each selected step.",
        ),
        (
            "seq-toggle-play",
            "(seq-toggle-play)",
            "Toggle sequencer playback.",
        ),
        (
            "seq-set-bpm",
            "(seq-set-bpm bpm)",
            "Set the project tempo in beats per minute.",
        ),
        (
            "seq-set-track-param",
            "(seq-set-track-param track :param value)",
            "Set a track-level parameter such as length, scale, transpose, or pan.",
        ),
        (
            "seq-set-accumulator",
            "(seq-set-accumulator value)",
            "Set the accumulator value for the current track.",
        ),
        (
            "__register-accumulator-preview",
            "(__register-accumulator-preview label)",
            "Internal helper that registers an accumulator preview label for UI selection.",
        ),
        (
            "__register-midi-fx-preview",
            "(__register-midi-fx-preview label)",
            "Internal helper that registers a MIDI FX preview label for UI selection.",
        ),
        (
            "seq-use-accumulator",
            "(seq-use-accumulator [track] name)",
            "Assign a script accumulator to a track.",
        ),
        (
            "seq-use-midi-fx",
            "(seq-use-midi-fx [track] name ...)",
            "Set the MIDI FX chain for a track.",
        ),
        (
            "seq-clear-midi-fx",
            "(seq-clear-midi-fx [track])",
            "Clear a track's MIDI FX chain.",
        ),
        (
            "seq-set-midi-fx-position",
            "(seq-set-midi-fx-position [track] position)",
            "Set where the MIDI FX chain runs relative to the accumulator.",
        ),
        (
            "seq-set-accum-mode",
            "(seq-set-accum-mode label)",
            "Set the current track's accumulator mode by label.",
        ),
        (
            "seq-set-accum-limit",
            "(seq-set-accum-limit limit)",
            "Set the current track's accumulator limit.",
        ),
        (
            "seq-double-track-pattern",
            "(seq-double-track-pattern)",
            "Duplicate the current track pattern to double its length.",
        ),
        (
            "seq-halve-track-pattern",
            "(seq-halve-track-pattern)",
            "Halve the current track pattern length.",
        ),
        (
            "seq-propagate-current-track-to-all-patterns",
            "(seq-propagate-current-track-to-all-patterns)",
            "Request host propagation of the current track to every pattern.",
        ),
        (
            "seq-set-timebase",
            "(seq-set-timebase label)",
            "Set the current track's default timebase by label.",
        ),
        (
            "seq-set-fts",
            "(seq-set-fts label)",
            "Set the current track's force-to-scale mode by label.",
        ),
        (
            "seq-plock-timebase",
            "(seq-plock-timebase label)",
            "Set a timebase p-lock on selected steps.",
        ),
        (
            "seq-set-swing-resolution",
            "(seq-set-swing-resolution label)",
            "Set default swing resolution or p-lock it on selected steps.",
        ),
        (
            "seq-pause-auto-follow",
            "(seq-pause-auto-follow)",
            "Temporarily pause automatic playhead following.",
        ),
        (
            "seq-toggle-record",
            "(seq-toggle-record)",
            "Toggle recording when at least one track is armed.",
        ),
        (
            "seq-toggle-record-arm",
            "(seq-toggle-record-arm track)",
            "Toggle record-arm state for a track.",
        ),
        (
            "seq-search-samples",
            "(seq-search-samples query)",
            "Search samples.db by sample title, hash, or tag.",
        ),
        (
            "seq-sample-browser",
            "(seq-sample-browser query selected-tags)",
            "Return DB-backed sample tag facets and a flat sample list.",
        ),
        (
            "seq-sample-tags-for-path",
            "(seq-sample-tags-for-path path)",
            "Return tags for a DB-backed sample path.",
        ),
        (
            "seq-sample-tree",
            "(seq-sample-tree)",
            "Return the DB-backed sample browser tree.",
        ),
        (
            "seq-filter-sample-tree",
            "(seq-filter-sample-tree query)",
            "Return a filtered DB-backed sample browser tree.",
        ),
        (
            "seq-project-tree",
            "(seq-project-tree query)",
            "Return the project browser tree filtered by query.",
        ),
        (
            "seq-preset-tree",
            "(seq-preset-tree presets query)",
            "Return a preset browser tree for a preset list and query.",
        ),
        (
            "seq-saved-instruments",
            "(seq-saved-instruments)",
            "Return saved custom instrument names.",
        ),
        (
            "seq-saved-instrument-tree",
            "(seq-saved-instrument-tree query)",
            "Return the saved instrument browser tree filtered by query.",
        ),
        (
            "seq-audio-effect-tree",
            "(seq-audio-effect-tree query)",
            "Return the audio effect browser tree filtered by query.",
        ),
        (
            "seq-midi-effect-tree",
            "(seq-midi-effect-tree query)",
            "Return the MIDI effect browser tree filtered by query.",
        ),
    ]);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn map_bool(value: &Value, key: &str) -> bool {
        let Value::Map(map) = value else {
            panic!("expected map value");
        };
        match map.get(key).map(|cell| cell.borrow().clone()) {
            Some(Value::Bool(value)) => value,
            other => panic!("expected bool field {key}, got {other:?}"),
        }
    }

    fn map_symbol(value: &Value, key: &str) -> String {
        let Value::Map(map) = value else {
            panic!("expected map value");
        };
        match map.get(key).map(|cell| cell.borrow().clone()) {
            Some(Value::Symbol(value)) => value,
            other => panic!("expected symbol field {key}, got {other:?}"),
        }
    }

    fn map_string(value: &Value, key: &str) -> String {
        let Value::Map(map) = value else {
            panic!("expected map value");
        };
        match map.get(key).map(|cell| cell.borrow().clone()) {
            Some(Value::String(value)) => value,
            other => panic!("expected string field {key}, got {other:?}"),
        }
    }

    #[test]
    fn applied_general_effect_artifact_can_finalize_but_not_apply_again() {
        let store = sequencer::agent::store::ConversationStore::new(44_100);
        let id = store.new_conversation(sequencer::agent::store::AgentKind::General);
        let mut state = store.snapshot(id).unwrap().state;
        state.effect_draft = Some(sequencer::agent::store::EffectDraft {
            name: "simple-tape-delay".to_string(),
            dsp_source: "(def in_l (in 1 @name left))".to_string(),
            ui_source: "(defeffect-ui (label \"ok\"))".to_string(),
        });
        state.accepted_effect_target = Some(sequencer::agent::store::AcceptedEffectTarget {
            track_index: 0,
            slot_index: 0,
            effect_name: "agent-effect-draft-1/".to_string(),
        });
        state.effect_draft_applied = true;

        let artifact = agent_artifact_value(state);

        assert_eq!(map_symbol(&artifact, "status"), "applied");
        assert!(!map_bool(&artifact, "can-apply"));
        assert!(map_bool(&artifact, "can-finalize"));
        assert!(map_bool(&artifact, "has-draft"));
    }

    #[test]
    fn updated_applied_effect_artifact_can_apply_again() {
        let store = sequencer::agent::store::ConversationStore::new(44_100);
        let id = store.new_conversation(sequencer::agent::store::AgentKind::Effect);
        let mut state = store.snapshot(id).unwrap().state;
        state.effect_draft = Some(sequencer::agent::store::EffectDraft {
            name: "simple-tape-delay".to_string(),
            dsp_source: "(def in_l (in 1 @name left))".to_string(),
            ui_source: "(defeffect-ui (label \"ok\"))".to_string(),
        });
        state.accepted_effect_target = Some(sequencer::agent::store::AcceptedEffectTarget {
            track_index: 0,
            slot_index: 0,
            effect_name: "agent-effect-draft-1/".to_string(),
        });
        state.effect_draft_applied = false;

        let artifact = agent_artifact_value(state);

        assert_eq!(map_symbol(&artifact, "status"), "updated");
        assert!(map_bool(&artifact, "can-apply"));
        assert_eq!(map_string(&artifact, "apply-label"), "Update artifact");
        assert!(map_bool(&artifact, "can-finalize"));
    }

    #[test]
    fn updated_applied_instrument_artifact_can_apply_again() {
        let store = sequencer::agent::store::ConversationStore::new(44_100);
        let id = store.new_conversation(sequencer::agent::store::AgentKind::Instrument);
        let mut state = store.snapshot(id).unwrap().state;
        state.draft = Some(sequencer::agent::store::InstrumentDraft {
            dsp_source: "(out 0 1 @name audio)".to_string(),
            ui_source: "(defsynth-ui (label \"updated\"))".to_string(),
        });
        state.accepted_instrument_target =
            Some(sequencer::agent::store::AcceptedInstrumentTarget {
                track_index: 0,
                instrument_name: "agent-draft-1/".to_string(),
            });

        let artifact = agent_artifact_value(state);

        assert_eq!(map_symbol(&artifact, "status"), "updated");
        assert!(map_bool(&artifact, "can-apply"));
        assert_eq!(map_string(&artifact, "apply-label"), "Update artifact");
        assert!(map_bool(&artifact, "can-finalize"));
    }

    #[test]
    fn agent_display_text_omits_fenced_source_blocks() {
        let display = agent_message_display_text(
            "Built it.\n```dgenlisp\n(def x 1)\n(out x 1 @name audio)\n```\n```eseqlisp\n(defsynth-ui)\n```",
        );

        assert_eq!(display.code_block_count, 2);
        assert_eq!(display.text, "Built it.");
    }

    #[test]
    fn agent_display_text_uses_placeholder_for_source_only_messages() {
        let display = agent_message_display_text("```dgenlisp\n(def x 1)\n```");

        assert_eq!(display.code_block_count, 1);
        assert_eq!(display.text, "Generated instrument source.");
    }

    #[test]
    fn agent_display_text_preserves_plain_messages() {
        let display = agent_message_display_text("No code here.\nStill normal.");

        assert_eq!(display.code_block_count, 0);
        assert_eq!(display.text, "No code here.\nStill normal.");
    }
}
