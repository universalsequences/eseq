use super::*;

pub(crate) struct RuntimeInit {
    pub(crate) runtime: Runtime,
    pub(crate) accumulator_names: Arc<Mutex<Vec<String>>>,
    pub(crate) midi_fx_names: Arc<Mutex<Vec<String>>>,
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
                ("track-names", build_track_names(&track_names)),
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
                ("sync-labels", build_sync_labels()),
                ("track-volumes", build_track_volumes(&state)),
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
                ("editor-error", Value::String(String::new())),
                ("editor-mode", Value::String(String::new())),
                ("editor-buffer-name", Value::String(String::new())),
            ];
            for idx in 0..track_count {
                fields.push((
                    Box::leak(format!("track-peak-{idx}").into_boxed_str()),
                    Value::Number(0.0),
                ));
            }
            for idx in 0..MAX_STEPS {
                fields.push((
                    Box::leak(format!("playhead-active-{idx}").into_boxed_str()),
                    Value::Bool(idx == 0),
                ));
            }
            fields
        },
        false,
    );

    // ── Native functions ──

    // seq-toggle-step — toggle step on current track
    let st = state.clone();
    let ct = current_track.clone();
    let auto_follow_override = auto_follow_override_until.clone();
    let ui_ep = ui_epoch.clone();
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
        *auto_follow_override.lock().unwrap() = Some(Instant::now() + AUTO_FOLLOW_COOLDOWN);
        ui_ep.fetch_add(1, Ordering::Relaxed);
        Ok(Value::Bool(st.pattern.patterns[track].is_active(step)))
    });

    // seq-set-step-param — set param on current track
    let st = state.clone();
    let ct = current_track.clone();
    let auto_follow_override = auto_follow_override_until.clone();
    let ui_ep = ui_epoch.clone();
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
            "speed" => StepParam::Speed,
            other => return Err(format!("seq-set-step-param: unknown param :{other}").into()),
        };
        let track = ct.load(Ordering::Relaxed);
        let val = (*val as f32).clamp(param.min(), param.max());
        st.pattern.step_data[track].set(step, param, val);
        st.publish_scheduler_snapshot();
        *auto_follow_override.lock().unwrap() = Some(Instant::now() + AUTO_FOLLOW_COOLDOWN);
        ui_ep.fetch_add(1, Ordering::Relaxed);
        Ok(Value::Number(val as f64))
    });

    let st = state.clone();
    let ct = current_track.clone();
    let piano_sel = piano_roll_selection.clone();
    let piano_move = piano_roll_move_state.clone();
    let auto_follow_override = auto_follow_override_until.clone();
    let ui_ep = ui_epoch.clone();
    runtime.register_native("seq-piano-roll-action", move |args, ctx| {
        let Some(action) = args.first() else {
            return Err("seq-piano-roll-action: expected action map".into());
        };
        let track = ct.load(Ordering::Relaxed);
        let status = apply_piano_roll_action(&st, track, &piano_sel, &piano_move, action)?;
        if piano_roll_action_mutates_pattern(action) {
            st.publish_scheduler_snapshot();
            *auto_follow_override.lock().unwrap() = Some(Instant::now() + AUTO_FOLLOW_COOLDOWN);
        }
        ui_ep.fetch_add(1, Ordering::Relaxed);
        ctx.set_status(status.clone());
        Ok(Value::String(status))
    });

    // seq-set-track — switch current track
    let st = state.clone();
    let ct = current_track.clone();
    runtime.register_native("seq-set-track", move |args, _ctx| {
        let Some(Value::Number(track)) = args.first() else {
            return Err("seq-set-track: expected track number".into());
        };
        let track = *track as usize;
        if track >= st.active_track_count() {
            return Err(format!("seq-set-track: track {track} out of range").into());
        }
        ct.store(track, Ordering::Relaxed);
        Ok(Value::Number(track as f64))
    });

    // seq-set-track-volume — (seq-set-track-volume track-idx volume)
    let st = state.clone();
    let pan_ids = track_pan_ids.clone();
    let ui_ep = ui_epoch.clone();
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
        ui_ep.fetch_add(1, Ordering::Relaxed);
        // Push volume to audiograph's stereo panner node
        let pan_ids_lock = pan_ids.lock().unwrap();
        if let Some(&pan_id) = pan_ids_lock.get(track) {
            unsafe {
                sequencer::audiograph::params_push_wrapper(
                    lg_raw,
                    sequencer::audiograph::ParamMsg {
                        idx: sequencer::stereo_panner::STEREO_PANNER_PARAM_VOLUME,
                        logical_id: pan_id as u64,
                        fvalue: vol,
                    },
                );
            }
        }
        Ok(Value::Number(vol as f64))
    });

    // seq-toggle-track-mute — (seq-toggle-track-mute track-idx)
    let st = state.clone();
    let pan_ids = track_pan_ids.clone();
    let ui_ep = ui_epoch.clone();
    runtime.register_native("seq-toggle-track-mute", move |args, _ctx| {
        let Some(Value::Number(track)) = args.first() else {
            return Err("seq-toggle-track-mute: expected track".into());
        };
        let track = *track as usize;
        if track >= st.active_track_count() {
            return Err(format!("seq-toggle-track-mute: track {track} out of range").into());
        }
        let muted = st.pattern.track_params[track].toggle_mute();
        ui_ep.fetch_add(1, Ordering::Relaxed);
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
    let ui_ep = ui_epoch.clone();
    runtime.register_native("seq-toggle-track-solo", move |args, _ctx| {
        let Some(Value::Number(track)) = args.first() else {
            return Err("seq-toggle-track-solo: expected track".into());
        };
        let track = *track as usize;
        if track >= st.active_track_count() {
            return Err(format!("seq-toggle-track-solo: track {track} out of range").into());
        }
        let solo = st.pattern.track_params[track].toggle_solo();
        ui_ep.fetch_add(1, Ordering::Relaxed);
        let pan_ids_lock = pan_ids.lock().unwrap();
        push_solo_mutes(lg_raw, &st, &pan_ids_lock);
        Ok(Value::Bool(solo))
    });

    let bus_state = buses.clone();
    let bus_nodes = bus_node_ids.clone();
    let ui_ep = ui_epoch.clone();
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
                        fvalue: vol,
                    },
                );
            }
        }
        ui_ep.fetch_add(1, Ordering::Relaxed);
        Ok(Value::Number(vol as f64))
    });

    let bus_state = buses.clone();
    let bus_nodes = bus_node_ids.clone();
    let ui_ep = ui_epoch.clone();
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
                        fvalue: volume,
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
        ui_ep.fetch_add(1, Ordering::Relaxed);
        Ok(Value::Bool(muted))
    });

    let bus_state = buses.clone();
    let ui_ep = ui_epoch.clone();
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
        ui_ep.fetch_add(1, Ordering::Relaxed);
        Ok(Value::Bool(solo))
    });

    // seq-set-effect-param — (seq-set-effect-param slot-idx param-idx value)
    let st = state.clone();
    let ct = current_track.clone();
    let descs = effect_descriptors.clone();
    let ui_ep = ui_epoch.clone();
    let fx_ep = fx_epoch.clone();
    let auto_follow_override = auto_follow_override_until.clone();
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

        // Push to audiograph
        let node_id = slot_state.node_id.load(Ordering::Relaxed);
        if node_id != 0 {
            let idx = slot_state.resolve_node_idx(param_idx);
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
                            logical_id: node_id as u64,
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
        ui_ep.fetch_add(1, Ordering::Relaxed);
        fx_ep.fetch_add(1, Ordering::Relaxed);
        Ok(Value::Number(clamped as f64))
    });

    // ── Selection natives ──

    // seq-select-step — toggle step in/out of selection
    let sel = selected_steps.clone();
    let ui_ep = ui_epoch.clone();
    let fx_ep = fx_epoch.clone();
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
        ui_ep.fetch_add(1, Ordering::Relaxed);
        fx_ep.fetch_add(1, Ordering::Relaxed);
        Ok(Value::Bool(!was_selected))
    });

    // seq-select-step-range — replace selection with inclusive step range
    let st = state.clone();
    let ct = current_track.clone();
    let sel = selected_steps.clone();
    let ui_ep = ui_epoch.clone();
    let fx_ep = fx_epoch.clone();
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
        let mut set = sel.lock().unwrap();
        set.clear();
        set.extend(lo..=hi);
        ui_ep.fetch_add(1, Ordering::Relaxed);
        fx_ep.fetch_add(1, Ordering::Relaxed);
        Ok(Value::Number((hi - lo + 1) as f64))
    });

    // seq-clear-selection
    let sel = selected_steps.clone();
    let ui_ep = ui_epoch.clone();
    let fx_ep = fx_epoch.clone();
    runtime.register_native("seq-clear-selection", move |_args, _ctx| {
        sel.lock().unwrap().clear();
        ui_ep.fetch_add(1, Ordering::Relaxed);
        fx_ep.fetch_add(1, Ordering::Relaxed);
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
    let ui_ep = ui_epoch.clone();
    let fx_ep = fx_epoch.clone();
    runtime.register_native("seq-select-all-steps", move |_args, _ctx| {
        let track = ct.load(Ordering::Relaxed);
        let num_steps = st.pattern.track_params[track].get_num_steps();
        let mut set = sel.lock().unwrap();
        set.clear();
        set.extend(0..num_steps);
        ui_ep.fetch_add(1, Ordering::Relaxed);
        fx_ep.fetch_add(1, Ordering::Relaxed);
        Ok(Value::Number(num_steps as f64))
    });

    // seq-delete-selected-steps — clear all selected steps and clear selection
    let st = state.clone();
    let ct = current_track.clone();
    let sel = selected_steps.clone();
    let ui_ep = ui_epoch.clone();
    let fx_ep = fx_epoch.clone();
    let auto_follow_override = auto_follow_override_until.clone();
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
        ui_ep.fetch_add(1, Ordering::Relaxed);
        fx_ep.fetch_add(1, Ordering::Relaxed);
        Ok(Value::Number(steps.len() as f64))
    });

    // seq-move-step-drag — move clicked step, or selected steps if clicked step is selected.
    let st = state.clone();
    let ct = current_track.clone();
    let sel = selected_steps.clone();
    let ui_ep = ui_epoch.clone();
    let fx_ep = fx_epoch.clone();
    let auto_follow_override = auto_follow_override_until.clone();
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
        ui_ep.fetch_add(1, Ordering::Relaxed);
        fx_ep.fetch_add(1, Ordering::Relaxed);
        Ok(Value::Bool(true))
    });

    // seq-shift-selected-steps — rotate selected step payloads left/right in place
    let st = state.clone();
    let ct = current_track.clone();
    let sel = selected_steps.clone();
    let ui_ep = ui_epoch.clone();
    let fx_ep = fx_epoch.clone();
    let auto_follow_override = auto_follow_override_until.clone();
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
        ui_ep.fetch_add(1, Ordering::Relaxed);
        fx_ep.fetch_add(1, Ordering::Relaxed);
        Ok(Value::Bool(true))
    });

    // seq-set-effect-plock — apply p-lock to ALL selected steps
    let st = state.clone();
    let ct = current_track.clone();
    let sel = selected_steps.clone();
    let ui_ep = ui_epoch.clone();
    let fx_ep = fx_epoch.clone();
    let auto_follow_override = auto_follow_override_until.clone();
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
        }
        st.publish_scheduler_snapshot();
        *auto_follow_override.lock().unwrap() = Some(Instant::now() + AUTO_FOLLOW_COOLDOWN);
        ui_ep.fetch_add(1, Ordering::Relaxed);
        fx_ep.fetch_add(1, Ordering::Relaxed);
        Ok(Value::Number(val as f64))
    });

    // seq-set-step-param-plock — apply step param p-lock to selected steps
    let st = state.clone();
    let ct = current_track.clone();
    let sel = selected_steps.clone();
    let ui_ep = ui_epoch.clone();
    let auto_follow_override = auto_follow_override_until.clone();
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
            "speed" => StepParam::Speed,
            other => return Err(format!("unknown param :{other}").into()),
        };
        let track = ct.load(Ordering::Relaxed);
        let val = (*val as f32).clamp(param.min(), param.max());
        let steps = sel.lock().unwrap();
        for &step in steps.iter() {
            st.pattern.step_data[track].set(step, param, val);
        }
        st.publish_scheduler_snapshot();
        *auto_follow_override.lock().unwrap() = Some(Instant::now() + AUTO_FOLLOW_COOLDOWN);
        ui_ep.fetch_add(1, Ordering::Relaxed);
        Ok(Value::Number(val as f64))
    });

    // seq-toggle-play
    let st = state.clone();
    runtime.register_native("seq-toggle-play", move |_args, _ctx| {
        let playing = st.transport.playing.load(Ordering::Relaxed);
        if playing {
            st.transport.playing.store(false, Ordering::Relaxed);
            st.publish_scheduler_snapshot();
        } else {
            st.transport.playing.store(true, Ordering::Relaxed);
            st.transport.playhead.store(0, Ordering::Relaxed);
            st.publish_scheduler_snapshot();
        }
        Ok(Value::Bool(!playing))
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
    let ui_ep = ui_epoch.clone();
    let auto_follow_override = auto_follow_override_until.clone();
    runtime.register_native("seq-set-track-param", move |args, _ctx| {
        let (Some(Value::Keyword(param_name)), Some(Value::Number(val))) =
            (args.first(), args.get(1))
        else {
            return Err("seq-set-track-param: expected (:param value)".into());
        };
        let track = ct.load(Ordering::Relaxed);
        let tp = &st.pattern.track_params[track];
        match param_name.as_str() {
            "attack" => {
                let v = (*val as f32).clamp(0.0, 500.0);
                tp.set_attack_ms(v);
                Ok(Value::Number(v as f64))
            }
            "release" => {
                let v = (*val as f32).clamp(0.0, 2000.0);
                tp.set_release_ms(v);
                Ok(Value::Number(v as f64))
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
                Ok(Value::Number(v as f64))
            }
            "num-steps" => {
                let v = (*val as usize).clamp(1, MAX_STEPS);
                tp.set_num_steps(v);
                Ok(Value::Number(v as f64))
            }
            "send" => {
                let v = (*val as f32).clamp(0.0, 1.0);
                tp.set_send(v);
                Ok(Value::Number(v as f64))
            }
            "gate" => {
                let want_on = *val != 0.0;
                if want_on != tp.is_gate_on() {
                    tp.toggle_gate();
                }
                Ok(Value::Bool(tp.is_gate_on()))
            }
            "poly" => {
                let want_on = *val != 0.0;
                if want_on != tp.is_polyphonic() {
                    tp.toggle_polyphonic();
                }
                Ok(Value::Bool(tp.is_polyphonic()))
            }
            other => return Err(format!("seq-set-track-param: unknown param :{other}").into()),
        }
        .inspect(|_| {
            st.publish_scheduler_snapshot();
            *auto_follow_override.lock().unwrap() = Some(Instant::now() + AUTO_FOLLOW_COOLDOWN);
            ui_ep.fetch_add(1, Ordering::Relaxed);
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
                )
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
                )
            }
            _ => {
                return Err(
                    "seq-set-midi-fx-position: expected :pre-accumulator or :post-accumulator"
                        .into(),
                )
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

    // seq-search-samples — recursively search samples/ for .wav files matching a query
    // Pre-scan the sample tree once and cache it for fast filtering.
    let sample_index: Vec<(String, String, String)> = {
        let mut index = Vec::new();
        let samples_dir = std::path::Path::new("samples");
        if samples_dir.is_dir() {
            let mut stack = vec![samples_dir.to_path_buf()];
            while let Some(dir) = stack.pop() {
                let Ok(entries) = std::fs::read_dir(&dir) else {
                    continue;
                };
                for entry in entries.flatten() {
                    let path = entry.path();
                    if path.is_dir() {
                        stack.push(path);
                    } else if let Some(ext) = path.extension() {
                        if ext.eq_ignore_ascii_case("wav") {
                            let name = path
                                .file_name()
                                .unwrap_or_default()
                                .to_string_lossy()
                                .to_string();
                            let parent = path
                                .parent()
                                .and_then(|p| p.file_name())
                                .unwrap_or_default()
                                .to_string_lossy()
                                .to_string();
                            let full_path = path.to_string_lossy().to_string();
                            index.push((name, parent, full_path));
                        }
                    }
                }
            }
            index.sort_by(|a, b| a.0.to_lowercase().cmp(&b.0.to_lowercase()));
        }
        eprintln!("metal_seq: indexed {} samples", index.len());
        index
    };
    runtime.register_native("seq-search-samples", move |args, _ctx| {
        let query = match args.first() {
            Some(Value::String(s)) => s.to_lowercase(),
            _ => String::new(),
        };
        let results: Vec<Rc<RefCell<Value>>> = sample_index
            .iter()
            .filter(|(name, _, _)| query.is_empty() || name.to_lowercase().contains(&query))
            .take(100) // cap results for UI performance
            .map(|(name, parent, full_path)| {
                let mut map = std::collections::HashMap::new();
                map.insert(
                    "name".to_string(),
                    Rc::new(RefCell::new(Value::String(name.clone()))),
                );
                map.insert(
                    "parent".to_string(),
                    Rc::new(RefCell::new(Value::String(parent.clone()))),
                );
                map.insert(
                    "path".to_string(),
                    Rc::new(RefCell::new(Value::String(full_path.clone()))),
                );
                Rc::new(RefCell::new(Value::Map(map)))
            })
            .collect();
        Ok(Value::List(results))
    });

    let sample_tree_nodes = build_sample_tree_node(std::path::Path::new("samples"));
    let sample_tree = sample_tree_nodes_to_value(&sample_tree_nodes);
    eprintln!("metal_seq: sample tree built");
    runtime.register_native(
        "seq-sample-tree",
        move |_args, _ctx| Ok(sample_tree.clone()),
    );
    runtime.register_native("seq-filter-sample-tree", move |args, _ctx| {
        let query_lower = match args.first() {
            Some(Value::String(s)) => s.trim().to_lowercase(),
            _ => String::new(),
        };
        let filtered = filter_sample_tree_nodes(&sample_tree_nodes, &query_lower);
        Ok(sample_tree_nodes_to_value(&filtered))
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
    document_metal_seq_natives(&mut runtime);

    RuntimeInit {
        runtime,
        accumulator_names,
        midi_fx_names,
    }
}

fn document_metal_seq_natives(runtime: &mut Runtime) {
    runtime.document_symbols([
        (
            "seq-toggle-step",
            "(seq-toggle-step step)",
            "Toggle the current track's step on/off and clear that step's p-locks.",
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
            "Search indexed sample files by name.",
        ),
        (
            "seq-sample-tree",
            "(seq-sample-tree)",
            "Return the sample browser tree.",
        ),
        (
            "seq-filter-sample-tree",
            "(seq-filter-sample-tree query)",
            "Return a filtered sample browser tree.",
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
    ]);
}
