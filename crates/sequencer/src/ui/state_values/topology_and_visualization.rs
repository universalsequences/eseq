use super::*;

pub(crate) fn sync_track_topology_state(
    rt: &mut Runtime,
    app: &app::App,
    state: &Arc<SequencerState>,
    track_names: &mut Vec<String>,
    current_track_idx: usize,
    selected_steps: &Arc<Mutex<HashSet<usize>>>,
    piano_roll_selection: &Arc<Mutex<HashSet<u64>>>,
    accumulator_names: &Arc<Mutex<Vec<String>>>,
    record_armed: &Arc<Mutex<Vec<bool>>>,
    track_peak_levels: &[f64],
) {
    sync_macro_state(rt, app);
    sync_track_name_state(rt, track_names, app);
    sync_bus_mixer_state(rt, app);
    sync_pattern_state(rt, state);
    set_current_track_reactive(rt, app.tracks.len(), current_track_idx);
    rt.set_reactive(
        "SEQ",
        "record-armed",
        build_record_armed_value(&record_armed.lock().unwrap()),
    );
    let (selected_step, selected_step_count) = {
        let selected = selected_steps.lock().unwrap();
        (selected.iter().copied().min(), selected.len())
    };
    rt.set_reactive(
        "SEQ",
        "fx-step-selection-count",
        Value::Number(selected_step_count as f64),
    );
    rt.set_reactive("SEQ", "fx-step-cursor-number", Value::Number(1.0));
    rt.set_reactive("SEQ", "fx-step-parameter-step", Value::Number(0.0));

    if app.tracks.is_empty() {
        sync_playhead_fields(rt, 0, 1);
        rt.set_reactive("SEQ", "steps", Value::List(vec![]));
        rt.set_reactive("SEQ", "piano-roll-items", Value::List(vec![]));
        rt.set_reactive("SEQ", "piano-roll-selection", Value::List(vec![]));
        rt.set_reactive("SEQ", "velocities", Value::List(vec![]));
        rt.set_reactive("SEQ", "durations", Value::List(vec![]));
        rt.set_reactive("SEQ", "transposes", Value::List(vec![]));
        rt.set_reactive("SEQ", "auxas", Value::List(vec![]));
        rt.set_reactive("SEQ", "pans", Value::List(vec![]));
        rt.set_reactive("SEQ", "syncs", Value::List(vec![]));
        rt.set_reactive("SEQ", "delays", Value::List(vec![]));
        sync_track_mixer_state(rt, app, state);
        sync_bus_mixer_state(rt, app);
        rt.set_reactive("SEQ", "effects", Value::List(vec![]));
        rt.set_reactive("SEQ", "midi-effects", Value::List(vec![]));
        rt.set_reactive("SEQ", "instrument-panel", Value::List(vec![]));
        rt.set_reactive("SEQ", "step-has-plocks", Value::List(vec![]));
        rt.set_reactive("SEQ", "step-plock-kinds", Value::List(vec![]));
        rt.set_reactive("SEQ", "step-variant-r", Value::List(vec![]));
        rt.set_reactive("SEQ", "step-variant-g", Value::List(vec![]));
        rt.set_reactive("SEQ", "step-variant-b", Value::List(vec![]));
        rt.set_reactive("SEQ", "track-steps", Value::List(vec![]));
        rt.set_reactive("SEQ", "track-num-steps", Value::List(vec![]));
        rt.set_reactive("SEQ", "track-timebases", Value::List(vec![]));
        rt.set_reactive("SEQ", "track-duration-spans", Value::List(vec![]));
        rt.set_reactive("SEQ", "track-playheads", Value::List(vec![]));
        rt.set_reactive("SEQ", "track-step-has-plocks", Value::List(vec![]));
        rt.set_reactive("SEQ", "track-step-plock-kinds", Value::List(vec![]));
        rt.set_reactive("SEQ", "track-step-variant-r", Value::List(vec![]));
        rt.set_reactive("SEQ", "track-step-variant-g", Value::List(vec![]));
        rt.set_reactive("SEQ", "track-step-variant-b", Value::List(vec![]));
        rt.set_reactive("SEQ", "track-velocities", Value::List(vec![]));
        rt.set_reactive("SEQ", "track-durations", Value::List(vec![]));
        rt.set_reactive("SEQ", "track-auxas", Value::List(vec![]));
        rt.set_reactive("SEQ", "track-transposes", Value::List(vec![]));
        rt.set_reactive("SEQ", "track-drum-racks", Value::List(vec![]));
        rt.set_reactive("SEQ", "track-drum-sounds", Value::List(vec![]));
        rt.set_reactive("SEQ", "track-pans", Value::List(vec![]));
        rt.set_reactive("SEQ", "track-syncs", Value::List(vec![]));
        rt.set_reactive("SEQ", "track-delays", Value::List(vec![]));
        rt.set_reactive("SEQ", "track-process-lanes", Value::List(vec![]));
        rt.set_reactive("SEQ", "process-lanes", Value::List(vec![]));
        rt.set_reactive("SEQ", "process-slots", Value::List(vec![]));
        rt.set_reactive("SEQ", "track-process-slots", Value::List(vec![]));
        rt.set_reactive("SEQ", "process-library", Value::List(vec![]));
        rt.set_reactive("SEQ", "track-ids", Value::List(vec![]));
        rt.set_reactive("SEQ", "track-plocks", Value::List(vec![]));
        rt.set_reactive("SEQ", "track-plock-variants", Value::List(vec![]));
        for param in [StepParam::Velocity, StepParam::Duration, StepParam::Transpose] {
            rt.set_reactive(
                "SEQ",
                fx_step_param_value_field(param)
                    .expect("step parameter strip field should exist"),
                Value::Number(0.0),
            );
        }
        return;
    }

    sync_all_track_sequencer_state(rt, state, app, current_track_idx, selected_steps);
    let cursor_step = fx_step_cursor_from_runtime(rt);
    sync_fx_step_cursor_binding_fields(
        rt,
        state,
        current_track_idx,
        cursor_step,
        selected_step,
        selected_step_count,
    );

    sync_playhead_fields(
        rt,
        state.transport.track_playheads[current_track_idx].load(Ordering::Relaxed) as usize,
        state.pattern.track_params[current_track_idx].get_num_steps(),
    );
    rt.set_reactive("SEQ", "steps", build_steps_value(state, current_track_idx));
    sync_piano_roll_state(rt, app, state, current_track_idx, piano_roll_selection);
    sync_step_param_lists(rt, state, current_track_idx);
    sync_track_mixer_state(rt, app, state);
    sync_bus_mixer_state(rt, app);
    sync_track_peak_fields(rt, track_peak_levels);
    rt.set_reactive(
        "SEQ",
        "effects",
        build_effects_value(
            state,
            current_track_idx,
            &app.graph.effect_descriptors,
            selected_steps,
        ),
    );
    rt.set_reactive(
        "SEQ",
        "midi-effects",
        build_midi_effects_value(state, current_track_idx, selected_steps),
    );
    rt.set_reactive(
        "SEQ",
        "instrument-panel",
        build_instrument_panel_value(app, current_track_idx, selected_steps),
    );
    rt.set_reactive(
        "SEQ",
        "fx-step-display-step",
        displayed_plock_step(state, current_track_idx, selected_plock_step(selected_steps))
            .map(|step| Value::Number(step as f64))
            .unwrap_or(Value::Number(-1.0)),
    );
    sync_fx_param_binding_fields(rt, app, state, current_track_idx, selected_steps);
    *accumulator_names.lock().unwrap() = build_accumulator_names(app);
    sync_track_params(rt, app, state, current_track_idx, selected_steps);
    rt.set_reactive(
        "SEQ",
        "step-has-plocks",
        build_step_has_plocks(state, current_track_idx, &app.graph.effect_descriptors),
    );
    rt.set_reactive(
        "SEQ",
        "step-plock-kinds",
        build_step_plock_kinds(state, current_track_idx),
    );
    rt.set_reactive(
        "SEQ",
        "step-variant-r",
        build_step_variant_color_channel(state, current_track_idx, 0),
    );
    rt.set_reactive(
        "SEQ",
        "step-variant-g",
        build_step_variant_color_channel(state, current_track_idx, 1),
    );
    rt.set_reactive(
        "SEQ",
        "step-variant-b",
        build_step_variant_color_channel(state, current_track_idx, 2),
    );
    sync_sidebar_browser(rt, app, current_track_idx);
}

pub(crate) fn sync_pattern_state(rt: &mut Runtime, state: &Arc<SequencerState>) {
    rt.set_reactive(
        "SEQ",
        "current-pattern",
        Value::Number(state.current_scene_index() as f64),
    );
    rt.set_reactive(
        "SEQ",
        "num-patterns",
        Value::Number(state.scene_count() as f64),
    );
    rt.set_reactive(
        "SEQ",
        "track-pattern-cells",
        build_track_pattern_cells_value(state, state.active_track_count()),
    );
    sync_track_pattern_cell_state_fields(rt, state, state.active_track_count());
    rt.set_reactive("SEQ", "neural-networks", build_neural_networks_value(state));
    rt.set_reactive(
        "SEQ",
        "neural-energy-matrix",
        build_neural_energy_matrix_value(state),
    );
    rt.set_reactive(
        "SEQ",
        "neural-trigger-matrix",
        build_neural_trigger_matrix_value(state),
    );
    rt.set_reactive(
        "SEQ",
        "neural-dampening-matrix",
        build_neural_dampening_matrix_value(state),
    );
    rt.set_reactive(
        "SEQ",
        "graph-visualizations",
        build_graph_visualizations_value(state),
    );
    rt.set_reactive(
        "SEQ",
        "track-events",
        build_track_output_events_value(state),
    );
    rt.set_reactive(
        "SEQ",
        "track-event-current-beat",
        build_track_output_current_beat_value(state),
    );
}

pub(crate) fn build_neural_networks_value(state: &Arc<SequencerState>) -> Value {
    Value::List(
        state
            .current_neural_networks()
            .iter()
            .map(neural_network_value)
            .map(|network| Rc::new(RefCell::new(network)))
            .collect(),
    )
}

pub(crate) fn build_neural_dampening_matrix_value(state: &Arc<SequencerState>) -> Value {
    let snapshot = state.neural_visualization();
    let size = snapshot.num_neurons.min(sequencer::neural::NUM_NEURONS);
    Value::List(
        (0..size)
            .map(|row| {
                Rc::new(RefCell::new(Value::List(
                    (0..size)
                        .map(|col| {
                            Rc::new(RefCell::new(Value::Number(neural_dampening_display_value(
                                snapshot.dampening[row][col],
                            ))))
                        })
                        .collect(),
                )))
            })
            .collect(),
    )
}

pub(crate) fn build_neural_energy_matrix_value(state: &Arc<SequencerState>) -> Value {
    let snapshot = state.neural_visualization();
    let size = snapshot.num_neurons.min(sequencer::neural::NUM_NEURONS);
    neural_column_matrix_value(
        (0..size).map(|idx| neural_energy_display_value(snapshot.energy[idx])),
    )
}

pub(crate) fn build_neural_trigger_matrix_value(state: &Arc<SequencerState>) -> Value {
    let snapshot = state.neural_visualization();
    let size = snapshot.num_neurons.min(sequencer::neural::NUM_NEURONS);
    neural_column_matrix_value(
        (0..size).map(|idx| neural_trigger_display_value(snapshot.trigger_activity[idx])),
    )
}

pub(crate) fn build_graph_visualizations_value(state: &Arc<SequencerState>) -> Value {
    Value::List(
        state
            .graph_visualizations()
            .iter()
            .map(|snapshot| Rc::new(RefCell::new(graph_visualization_value(snapshot))))
            .collect(),
    )
}

pub(crate) fn build_track_output_events_value(state: &Arc<SequencerState>) -> Value {
    Value::List(
        state
            .track_output_events()
            .into_iter()
            .map(|event| Rc::new(RefCell::new(track_output_event_value(event))))
            .collect(),
    )
}

pub(crate) fn build_track_output_current_beat_value(state: &Arc<SequencerState>) -> Value {
    Value::Number(state.track_output_current_beat())
}

pub(crate) fn build_active_notes_value(notes: &[u8]) -> Value {
    Value::List(
        notes
            .iter()
            .map(|note| Rc::new(RefCell::new(Value::Number(*note as f64))))
            .collect(),
    )
}

pub(super) fn track_output_event_value(event: sequencer::sequencer::TrackOutputEvent) -> Value {
    map_value([
        ("node", Value::Nil),
        ("track", Value::Number(event.track as f64)),
        ("sample", Value::Number(event.sample_time as f64)),
        ("beat", Value::Number(event.beat)),
        ("transpose", Value::Number(event.transpose as f64)),
        ("velocity", Value::Number(event.velocity as f64)),
    ])
}

pub(super) fn graph_visualization_value(snapshot: &sequencer::graph::GraphVisualizationSnapshot) -> Value {
    let mut map = HashMap::new();
    map.insert(
        "id".to_string(),
        value_cell(Value::Number(snapshot.id as f64)),
    );
    map.insert(
        "name".to_string(),
        value_cell(Value::String(snapshot.name.clone())),
    );
    map.insert(
        "active".to_string(),
        value_cell(Value::Bool(snapshot.active)),
    );
    map.insert(
        "current-beat".to_string(),
        value_cell(Value::Number(snapshot.current_beat)),
    );
    map.insert(
        "num-nodes".to_string(),
        value_cell(Value::Number(snapshot.num_nodes as f64)),
    );
    map.insert(
        "energy-matrix".to_string(),
        value_cell(neural_column_matrix_value(
            snapshot
                .energy
                .iter()
                .take(snapshot.num_nodes)
                .map(|value| graph_energy_display_value(*value)),
        )),
    );
    map.insert(
        "trigger-matrix".to_string(),
        value_cell(neural_column_matrix_value(
            snapshot
                .trigger_activity
                .iter()
                .take(snapshot.num_nodes)
                .map(|value| neural_trigger_display_value(*value)),
        )),
    );
    map.insert(
        "weight-matrix".to_string(),
        value_cell(graph_dense_edge_matrix_value(snapshot, |edge| {
            graph_weight_display_value(edge.weight)
        })),
    );
    map.insert(
        "dampening-matrix".to_string(),
        value_cell(graph_dense_edge_matrix_value(snapshot, |edge| {
            neural_dampening_display_value(edge.dampening as f32)
        })),
    );
    map.insert(
        "delay-matrix".to_string(),
        value_cell(graph_dense_edge_matrix_value(snapshot, |edge| {
            edge.delay_steps as f64
        })),
    );
    map.insert(
        "edges".to_string(),
        value_cell(Value::List(
            snapshot
                .edges
                .iter()
                .map(|edge| Rc::new(RefCell::new(graph_edge_value(*edge))))
                .collect(),
        )),
    );
    map.insert(
        "node-events".to_string(),
        value_cell(Value::List(
            snapshot
                .node_events
                .iter()
                .take(snapshot.num_nodes)
                .map(|event| Rc::new(RefCell::new(graph_optional_event_value(*event))))
                .collect(),
        )),
    );
    map.insert(
        "events".to_string(),
        value_cell(Value::List(
            snapshot
                .node_events
                .iter()
                .take(snapshot.num_nodes)
                .flatten()
                .copied()
                .map(|event| Rc::new(RefCell::new(graph_event_value(event))))
                .collect(),
        )),
    );
    map.insert(
        "event-history".to_string(),
        value_cell(Value::List(
            snapshot
                .event_history
                .iter()
                .copied()
                .map(|event| Rc::new(RefCell::new(graph_raw_event_value(event))))
                .collect(),
        )),
    );
    Value::Map(map)
}

pub(super) fn value_cell(value: Value) -> Rc<RefCell<Value>> {
    Rc::new(RefCell::new(value))
}

pub(super) fn graph_dense_edge_matrix_value(
    snapshot: &sequencer::graph::GraphVisualizationSnapshot,
    value: impl Fn(sequencer::graph::GraphVisualizationEdge) -> f64,
) -> Value {
    let mut matrix = vec![vec![0.0; snapshot.num_nodes]; snapshot.num_nodes];
    for edge in &snapshot.edges {
        if edge.from < snapshot.num_nodes && edge.to < snapshot.num_nodes {
            matrix[edge.from][edge.to] = value(*edge);
        }
    }
    Value::List(
        matrix
            .into_iter()
            .map(|row| {
                Rc::new(RefCell::new(Value::List(
                    row.into_iter()
                        .map(|cell| Rc::new(RefCell::new(Value::Number(cell))))
                        .collect(),
                )))
            })
            .collect(),
    )
}

pub(super) fn graph_edge_value(edge: sequencer::graph::GraphVisualizationEdge) -> Value {
    let mut map = HashMap::new();
    map.insert(
        "from".to_string(),
        value_cell(Value::Number(edge.from as f64)),
    );
    map.insert("to".to_string(), value_cell(Value::Number(edge.to as f64)));
    map.insert(
        "weight".to_string(),
        value_cell(Value::Number(graph_weight_display_value(edge.weight))),
    );
    map.insert(
        "dampening".to_string(),
        value_cell(Value::Number(neural_dampening_display_value(
            edge.dampening as f32,
        ))),
    );
    map.insert(
        "delay".to_string(),
        value_cell(Value::Number(edge.delay_steps as f64)),
    );
    map.insert(
        "distribution".to_string(),
        value_cell(Value::String(match edge.distribution {
            sequencer::graph::EdgeDistribution::BroadcastWeighted => {
                "broadcast-weighted".to_string()
            }
            sequencer::graph::EdgeDistribution::WeightedChoice => "weighted-choice".to_string(),
        })),
    );
    Value::Map(map)
}

pub(super) fn graph_optional_event_value(event: Option<sequencer::graph::GraphVisualizationEvent>) -> Value {
    event.map(graph_event_value).unwrap_or(Value::Nil)
}

pub(super) fn graph_event_value(event: sequencer::graph::GraphVisualizationEvent) -> Value {
    let mut map = HashMap::new();
    map.insert(
        "node".to_string(),
        value_cell(Value::Number(event.node_index as f64)),
    );
    map.insert(
        "track".to_string(),
        value_cell(
            event
                .track
                .map(|track| Value::Number(track as f64))
                .unwrap_or(Value::Nil),
        ),
    );
    map.insert(
        "sample".to_string(),
        value_cell(Value::Number(event.sample_time as f64)),
    );
    map.insert("beat".to_string(), value_cell(Value::Number(event.beat)));
    map.insert(
        "transpose".to_string(),
        value_cell(Value::Number(graph_weight_display_value(
            event.transpose as f64,
        ))),
    );
    map.insert(
        "velocity".to_string(),
        value_cell(Value::Number(neural_trigger_display_value(event.velocity))),
    );
    Value::Map(map)
}

pub(super) fn graph_raw_event_value(event: sequencer::graph::GraphVisualizationEvent) -> Value {
    let mut map = HashMap::new();
    map.insert(
        "node".to_string(),
        value_cell(Value::Number(event.node_index as f64)),
    );
    map.insert(
        "track".to_string(),
        value_cell(
            event
                .track
                .map(|track| Value::Number(track as f64))
                .unwrap_or(Value::Nil),
        ),
    );
    map.insert(
        "sample".to_string(),
        value_cell(Value::Number(event.sample_time as f64)),
    );
    map.insert("beat".to_string(), value_cell(Value::Number(event.beat)));
    map.insert(
        "transpose".to_string(),
        value_cell(Value::Number(event.transpose as f64)),
    );
    map.insert(
        "velocity".to_string(),
        value_cell(Value::Number(event.velocity as f64)),
    );
    Value::Map(map)
}

pub(super) fn neural_column_matrix_value(values: impl Iterator<Item = f64>) -> Value {
    Value::List(
        values
            .map(|value| {
                Rc::new(RefCell::new(Value::List(vec![Rc::new(RefCell::new(
                    Value::Number(value),
                ))])))
            })
            .collect(),
    )
}

pub(super) fn neural_energy_display_value(value: f32) -> f64 {
    let value = value.clamp(0.0, 4.0) as f64;
    (value * 100.0).round() / 100.0
}

pub(super) fn graph_energy_display_value(value: f64) -> f64 {
    let value = value.clamp(0.0, 4.0);
    (value * 100.0).round() / 100.0
}

pub(super) fn graph_weight_display_value(value: f64) -> f64 {
    (value * 100.0).round() / 100.0
}

pub(super) fn neural_trigger_display_value(value: f32) -> f64 {
    value.clamp(0.0, 1.0) as f64
}

pub(super) fn neural_dampening_display_value(value: f32) -> f64 {
    let value = value.clamp(0.0, 1.0) as f64;
    (value * 100.0).round() / 100.0
}

pub(super) fn neural_network_value(network: &sequencer::neural::ProjectNeuralNetwork) -> Value {
    let mut map = HashMap::new();
    map.insert(
        "id".to_string(),
        Rc::new(RefCell::new(Value::Number(network.id as f64))),
    );
    map.insert(
        "name".to_string(),
        Rc::new(RefCell::new(Value::String(network.name.clone()))),
    );
    map.insert(
        "enabled".to_string(),
        Rc::new(RefCell::new(Value::Bool(network.enabled))),
    );
    map.insert(
        "num-neurons".to_string(),
        Rc::new(RefCell::new(Value::Number(network.num_neurons as f64))),
    );
    map.insert(
        "reset-bars".to_string(),
        Rc::new(RefCell::new(Value::Number(
            network.reset_interval_bars as f64,
        ))),
    );
    map.insert(
        "energy-decay".to_string(),
        Rc::new(RefCell::new(Value::Number(network.energy_decay as f64))),
    );
    map.insert(
        "max-poly".to_string(),
        Rc::new(RefCell::new(Value::Number(network.max_poly as f64))),
    );
    map.insert(
        "max-poly-selection".to_string(),
        Rc::new(RefCell::new(Value::String(
            network.max_poly_selection.as_str().to_string(),
        ))),
    );
    map.insert(
        "weights".to_string(),
        Rc::new(RefCell::new(Value::List(
            network
                .weights
                .iter()
                .map(|row| {
                    Rc::new(RefCell::new(Value::List(
                        row.iter()
                            .map(|value| Rc::new(RefCell::new(Value::Number(*value as f64))))
                            .collect(),
                    )))
                })
                .collect(),
        ))),
    );
    map.insert(
        "neurons".to_string(),
        Rc::new(RefCell::new(Value::List(
            network
                .neurons
                .iter()
                .enumerate()
                .map(|(idx, neuron)| Rc::new(RefCell::new(neural_neuron_value(idx, neuron))))
                .collect(),
        ))),
    );
    Value::Map(map)
}

pub(super) fn neural_neuron_value(idx: usize, neuron: &sequencer::neural::ProjectNeuron) -> Value {
    let mut map = HashMap::new();
    map.insert(
        "index".to_string(),
        Rc::new(RefCell::new(Value::Number(idx as f64))),
    );
    map.insert(
        "route".to_string(),
        Rc::new(RefCell::new(
            neuron
                .route
                .map(|route| Value::Number(route as f64))
                .unwrap_or(Value::Nil),
        )),
    );
    map.insert(
        "resolution".to_string(),
        Rc::new(RefCell::new(Value::Keyword(
            neuron.resolution_timebase().label().to_string(),
        ))),
    );
    map.insert(
        "delay".to_string(),
        Rc::new(RefCell::new(Value::Number(neuron.delay_steps as f64))),
    );
    map.insert(
        "threshold".to_string(),
        Rc::new(RefCell::new(Value::Number(neuron.threshold as f64))),
    );
    map.insert(
        "transpose".to_string(),
        Rc::new(RefCell::new(Value::Number(neuron.transpose as f64))),
    );
    map.insert(
        "quantize".to_string(),
        Rc::new(RefCell::new(
            neuron
                .quantize_timebase()
                .map(|timebase| Value::Keyword(timebase.label().to_string()))
                .unwrap_or(Value::Nil),
        )),
    );
    map.insert(
        "dampening".to_string(),
        Rc::new(RefCell::new(Value::Number(neuron.dampening_amount as f64))),
    );
    map.insert(
        "dampening-recovery".to_string(),
        Rc::new(RefCell::new(Value::Number(
            neuron.dampening_recovery as f64,
        ))),
    );
    Value::Map(map)
}

pub(crate) fn build_sync_labels() -> Value {
    let items: Vec<Rc<RefCell<Value>>> = SYNC_RESOLUTIONS
        .iter()
        .map(|(_, label)| {
            let mut compact = label.replace(' ', "");
            compact.truncate(4);
            Rc::new(RefCell::new(Value::String(compact)))
        })
        .collect();
    Value::List(items)
}
