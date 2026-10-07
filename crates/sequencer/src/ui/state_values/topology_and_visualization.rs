use super::*;

pub(crate) fn sync_track_topology_state(
    rt: &mut Runtime,
    app: &app::App,
    state: &Arc<SequencerState>,
    track_names: &mut Vec<String>,
    current_track_idx: usize,
    selected_steps: &Arc<Mutex<HashSet<usize>>>,
    accumulator_names: &Arc<Mutex<Vec<String>>>,
    record_armed: &Arc<Mutex<Vec<bool>>>,
    track_peak_levels: &[f64],
) {
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
        rt.set_reactive("SEQ", "velocities", Value::List(vec![]));
        rt.set_reactive("SEQ", "durations", Value::List(vec![]));
        rt.set_reactive("SEQ", "transposes", Value::List(vec![]));
        rt.set_reactive("SEQ", "auxas", Value::List(vec![]));
        rt.set_reactive("SEQ", "pans", Value::List(vec![]));
        rt.set_reactive("SEQ", "syncs", Value::List(vec![]));
        rt.set_reactive("SEQ", "delays", Value::List(vec![]));
        rt.set_reactive("SEQ", "retrigs", Value::List(vec![]));
        rt.set_reactive("SEQ", "retrig-rates", Value::List(vec![]));
        sync_track_mixer_state(rt, app, state);
        sync_bus_mixer_state(rt, app);
        rt.set_reactive("SEQ", "effects", Value::List(vec![]));
        rt.set_reactive("SEQ", "track-device-chains", Value::List(vec![]));
        rt.set_reactive("SEQ", "bus-device-chains", Value::List(vec![]));
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
            rt.set_reactive("SEQ", "track-pans", Value::List(vec![]));
        rt.set_reactive("SEQ", "track-syncs", Value::List(vec![]));
        rt.set_reactive("SEQ", "track-delays", Value::List(vec![]));
        rt.set_reactive("SEQ", "track-retrigs", Value::List(vec![]));
        rt.set_reactive("SEQ", "track-retrig-rates", Value::List(vec![]));
        rt.set_reactive("SEQ", "track-process-lanes", Value::List(vec![]));
        rt.set_reactive("SEQ", "track-process-lane-values", Value::List(vec![]));
        rt.set_reactive("SEQ", "process-lanes", Value::List(vec![]));
        rt.set_reactive("SEQ", "process-slots", Value::List(vec![]));
        rt.set_reactive("SEQ", "track-process-slots", Value::List(vec![]));
        rt.set_reactive("SEQ", "track-lane-patch", Value::List(vec![]));
        rt.set_reactive("SEQ", "process-library", Value::List(vec![]));
        rt.set_reactive("SEQ", "track-ids", Value::List(vec![]));
        rt.set_reactive("SEQ", "track-plocks", Value::List(vec![]));
        rt.set_reactive("SEQ", "track-plock-any", Value::List(vec![]));
        rt.set_reactive("SEQ", "track-plock-variants", Value::List(vec![]));
        for param in STEP_INSPECTOR_PARAMS {
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
    sync_track_automation_state(rt, app, state);
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
        "track-device-chains",
        build_track_device_chains_value(app, state),
    );
    rt.set_reactive("SEQ", "bus-device-chains", build_bus_device_chains_value(app));
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
    sync_rack_clip_state(rt, state);
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

    // `defscene` values are not ordinary SEQ fields: each reader injects a
    // qualified host-owned dependency. Queue every currently subscribed slot
    // with the newly-current scene's epoch so the repaint joins this pattern
    // sync's reactive cycle and observes all of the fields staged above.
    let scene_slots = state.current_scene_slots();
    rt.queue_reactive_namespace_invalidation(
        sequencer::lisp_host::SCENE_SLOT_REACTIVE_NAMESPACE,
        |name| Value::String(scene_slots.epoch(name).to_string()),
    );
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

pub(crate) fn build_track_active_notes_value(
    state: &Arc<SequencerState>,
    track_count: usize,
) -> Value {
    let notes_by_track: Vec<Vec<sequencer::sequencer::ActiveNoteActivity>> = (0..track_count)
        .map(|track| state.active_note_activity(track))
        .collect();
    build_track_active_notes_snapshot_value(&notes_by_track)
}

pub(crate) fn build_track_active_notes_snapshot_value(
    notes_by_track: &[Vec<sequencer::sequencer::ActiveNoteActivity>],
) -> Value {
    Value::List(
        notes_by_track
            .iter()
            .map(|notes| {
                Rc::new(RefCell::new(Value::List(
                    notes
                        .iter()
                        .map(|activity| {
                            Rc::new(RefCell::new(map_value([
                                ("note", Value::Number(activity.note as f64)),
                                ("velocity", Value::Number(activity.velocity as f64)),
                                ("trigger-id", Value::Number(activity.trigger_id as f64)),
                            ])))
                        })
                        .collect(),
                )))
            })
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
        "group-activity-matrix".to_string(),
        value_cell(neural_column_matrix_value(
            snapshot.group_activity.iter().copied(),
        )),
    );
    map.insert(
        "group-suppression-matrix".to_string(),
        value_cell(neural_column_matrix_value(
            snapshot.group_suppression.iter().copied(),
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
    let nodes = snapshot.num_nodes;
    let (mut delta_matrix, mut node_delta_magnitudes) =
        (vec![0.0; nodes * nodes], vec![0.0; nodes]);
    graph_delta_values(snapshot, &mut delta_matrix, &mut node_delta_magnitudes);
    map.insert(
        "delta-matrix".to_string(),
        value_cell(Value::List(
            delta_matrix
                .chunks(nodes.max(1))
                .map(|row| {
                    Rc::new(RefCell::new(Value::List(
                        row.iter()
                            .map(|cell| Rc::new(RefCell::new(Value::Number(*cell))))
                            .collect(),
                    )))
                })
                .collect(),
        )),
    );
    map.insert(
        "node-delta-column".to_string(),
        value_cell(neural_column_matrix_value(
            node_delta_magnitudes.into_iter(),
        )),
    );
    map.insert(
        "delta-leak-per-beat".to_string(),
        value_cell(Value::Number(snapshot.delta_leak_per_beat as f64)),
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

/// A graph's live deltas (the legacy `delta-matrix` and `node-delta-column`,
/// and the host kinds' `graph.deltas` / `node-deltas`): into `matrix`
/// (zeroed, `num_nodes` square, row-major) each weight delta by from row and
/// to column, and into `magnitudes` (zeroed, `num_nodes` long) the sum of
/// each node's delay and param deltas' magnitudes.
pub(crate) fn graph_delta_values(
    snapshot: &sequencer::graph::GraphVisualizationSnapshot,
    matrix: &mut [f64],
    magnitudes: &mut [f64],
) {
    use sequencer::graph::GraphDeltaKey;
    let nodes = snapshot.num_nodes;
    for entry in &snapshot.deltas {
        match &entry.key {
            GraphDeltaKey::NodeDelay { node } | GraphDeltaKey::NodeParam { node, .. } => {
                if let Some(total) = magnitudes.get_mut(*node) {
                    *total += entry.delta.abs() as f64;
                }
            }
            GraphDeltaKey::EdgeParam { from, to, param } if param == "weight" => {
                if *from < nodes && *to < nodes {
                    matrix[from * nodes + to] = entry.delta as f64;
                }
            }
            GraphDeltaKey::EdgeParam { .. } => {}
        }
    }
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

/// The neurons the engine's visualization snapshot covers (the host kinds'
/// `neuron` live values).
pub(crate) fn neural_snapshot_size(
    snapshot: &sequencer::neural::NeuralVisualizationSnapshot,
) -> usize {
    snapshot.num_neurons.min(sequencer::neural::NUM_NEURONS)
}

pub(crate) fn neural_energy_display_value(value: f32) -> f64 {
    let value = value.clamp(0.0, 4.0) as f64;
    (value * 100.0).round() / 100.0
}

pub(crate) fn graph_energy_display_value(value: f64) -> f64 {
    let value = value.clamp(0.0, 4.0);
    (value * 100.0).round() / 100.0
}

pub(crate) fn graph_weight_display_value(value: f64) -> f64 {
    (value * 100.0).round() / 100.0
}

pub(crate) fn neural_trigger_display_value(value: f32) -> f64 {
    value.clamp(0.0, 1.0) as f64
}

pub(crate) fn neural_dampening_display_value(value: f32) -> f64 {
    let value = value.clamp(0.0, 1.0) as f64;
    (value * 100.0).round() / 100.0
}

pub(crate) fn build_sync_labels() -> Value {
    list_value(sync_labels().map(Value::String))
}

/// `step.sync` labels by value (`SEQ.sync-labels`,
/// `project.sync-options`): the sync resolutions, compacted to four
/// characters.
pub(crate) fn sync_labels() -> impl Iterator<Item = String> {
    SYNC_RESOLUTIONS.iter().map(|(_, label)| {
        let mut compact = label.replace(' ', "");
        compact.truncate(4);
        compact
    })
}

/// Per-rack clip bank for the collapsed rack row and the mixer strip
/// (rack-clips spec §6): `{group-id, active, clips: [{id name}]}`. `active` is
/// the clip the CURRENT scene points at, or -1 for silence. A rack with no
/// bank (legacy) contributes no entry, which is how the UI tells the two
/// apart and offers "Convert to clips".
pub(crate) fn sync_rack_clip_state(rt: &mut Runtime, state: &Arc<SequencerState>) -> bool {
    let mut changed = false;
    let banks = state.with_scenes(|scenes| {
        list_value(scenes.rack_banks().iter().map(|bank| {
            let active = scenes.current_rack_clip(bank.group_id);
            let index = bank.clips.iter().position(|clip| Some(clip.id) == active)
                .map_or(0, |index| index + 1);
            changed |= rt.set_reactive("SEQ", &format!("rack-clip-index-{}", bank.group_id),
                Value::Number(index as f64)).changed;
            for clip in &bank.clips {
                changed |= rt.set_reactive("SEQ", &format!("rack-clip-active-{}-{}", bank.group_id, clip.id),
                    Value::Number(if Some(clip.id) == active { 1.0 } else { 0.0 })).changed;
            }
            map_value([
                ("group-id", Value::Number(bank.group_id as f64)),
                ("clips", list_value(bank.clips.iter().map(|clip| map_value([
                    ("id", Value::Number(clip.id as f64)),
                    ("name", Value::String(clip.name.clone().into())),
                ])))),
            ])
        }))
    });
    // Roster/labels are structural; changing the active clip only updates
    // retained numeric bindings, without rebuilding the sequencer header.
    changed |= rt.set_reactive("SEQ", "rack-clip-banks", banks).changed;
    changed |= rt.set_reactive("SEQ", "rack-clips", build_rack_clips_value(state)).changed;
    changed
}

pub(crate) fn build_rack_clips_value(state: &Arc<SequencerState>) -> Value {
    state.with_scenes(|scenes| {
        list_value(scenes.rack_banks().iter().map(|bank| {
            let active = scenes
                .current_rack_clip(bank.group_id)
                .map(|id| id as f64)
                .unwrap_or(-1.0);
            map_value([
                ("group-id", Value::Number(bank.group_id as f64)),
                ("active", Value::Number(active)),
                // One entry per project scene: the clip that scene points at,
                // or -1 for silence. The "Export as kit..." checklist defaults
                // to the scenes this rack actually plays (spec 7.2).
                (
                    "scene-clips",
                    list_value((0..scenes.scenes.len()).map(|scene| {
                        Value::Number(
                            scenes
                                .scene_rack_clip(scene, bank.group_id)
                                .map(|id| id as f64)
                                .unwrap_or(-1.0),
                        )
                    })),
                ),
                (
                    "clips",
                    list_value(bank.clips.iter().map(|clip| {
                        map_value([
                            ("id", Value::Number(clip.id as f64)),
                            ("name", Value::String(clip.name.clone().into())),
                        ])
                    })),
                ),
            ])
        }))
    })
}
