use super::*;

/// After a topology change (a track added, removed or replaced, a project
/// opened): the host-side caches that follow it. The track names cache, the
/// scene slot invalidations and, with tracks, the accumulator names and the
/// sidebar. (The views read the host kinds, which follow the model.)
pub(crate) fn sync_track_topology_state(
    rt: &mut Runtime,
    app: &app::App,
    state: &Arc<SequencerState>,
    track_names: &mut Vec<String>,
    current_track_idx: usize,
    accumulator_names: &Arc<Mutex<Vec<String>>>,
) {
    refresh_track_names_cache(track_names, app);
    sync_scene_slot_state(rt, state);
    if app.tracks.is_empty() {
        return;
    }
    *accumulator_names.lock().unwrap() = build_accumulator_names(app);
    sync_sidebar_browser(app, current_track_idx);
}

/// After a pattern (scene) switch: `defscene` values are host-owned
/// dependencies each reader injects. Queue every currently subscribed slot
/// with the newly-current scene's epoch so the repaint joins this sync's
/// reactive cycle.
pub(crate) fn sync_scene_slot_state(rt: &mut Runtime, state: &Arc<SequencerState>) {
    let scene_slots = state.current_scene_slots();
    rt.queue_reactive_namespace_invalidation(
        sequencer::lisp_host::SCENE_SLOT_REACTIVE_NAMESPACE,
        |name| Value::String(scene_slots.epoch(name).to_string()),
    );
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

/// `step.sync` labels by value (`project.sync-options`): the sync
/// resolutions, compacted to four characters.
pub(crate) fn sync_labels() -> impl Iterator<Item = String> {
    SYNC_RESOLUTIONS.iter().map(|(_, label)| {
        let mut compact = label.replace(' ', "");
        compact.truncate(4);
        compact
    })
}
