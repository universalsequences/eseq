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
    set_current_track_reactive(rt, current_track_idx);
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
        rt.set_reactive("SEQ", "track-process-slots", Value::List(vec![]));
        rt.set_reactive("SEQ", "process-library", Value::List(vec![]));
        rt.set_reactive("SEQ", "track-ids", Value::List(vec![]));
        rt.set_reactive("SEQ", "track-plocks", Value::List(vec![]));
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

    sync_all_track_sequencer_state(rt, state, app);
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

pub(crate) fn build_active_notes_value(notes: &[u8]) -> Value {
    Value::List(
        notes
            .iter()
            .map(|note| Rc::new(RefCell::new(Value::Number(*note as f64))))
            .collect(),
    )
}

/// A graph's live deltas (the host kinds' `graph.deltas` / `node-deltas`):
/// into `matrix` (zeroed, `num_nodes` square, row-major) each weight delta by
/// from row and to column, and into `magnitudes` (zeroed, `num_nodes` long)
/// the sum of each node's delay and param deltas' magnitudes.
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

/// Per-rack clip bank (rack-clips spec §6), read by drum-rack-v2's lookups:
/// `{group-id, active, clips: [{id name}]}`. `active` is
/// the clip the CURRENT scene points at, or -1 for silence. A rack with no
/// bank (legacy) contributes no entry, which is how the UI tells the two
/// apart and offers "Convert to clips".
pub(crate) fn sync_rack_clip_state(rt: &mut Runtime, state: &Arc<SequencerState>) -> bool {
    let mut changed = false;
    let banks = state.with_scenes(|scenes| {
        list_value(scenes.rack_banks().iter().map(|bank| {
            map_value([
                ("group-id", Value::Number(bank.group_id as f64)),
                ("clips", list_value(bank.clips.iter().map(|clip| map_value([
                    ("id", Value::Number(clip.id as f64)),
                    ("name", Value::String(clip.name.clone().into())),
                ])))),
            ])
        }))
    });
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
