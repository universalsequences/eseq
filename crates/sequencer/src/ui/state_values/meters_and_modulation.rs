use super::*;

pub(crate) fn push_panner_bool(
    lg_raw: *mut sequencer::audiograph::LiveGraph,
    node_id: u64,
    param_idx: u64,
    value: bool,
) {
    if node_id == 0 {
        return;
    }
    unsafe {
        sequencer::audiograph::params_push_wrapper(
            lg_raw,
            sequencer::audiograph::ParamMsg {
                idx: param_idx,
                logical_id: node_id,
                fvalue: if value { 1.0 } else { 0.0 },
            },
        );
    }
}

pub(crate) fn push_solo_mutes(
    lg_raw: *mut sequencer::audiograph::LiveGraph,
    state: &Arc<SequencerState>,
) {
    let count = state.active_track_count();
    let has_solo = (0..count).any(|track| state.pattern.track_params[track].is_solo());
    for track in 0..count {
        let muted_by_solo = has_solo && !state.pattern.track_params[track].is_solo();
        let fader_id = state.runtime.delay_lids[track].load(Ordering::Acquire);
        push_panner_bool(
            lg_raw,
            fader_id,
            sequencer::effects::stereo_panner::STEREO_PANNER_PARAM_MUTED_BY_SOLO,
            muted_by_solo,
        );
    }
}

pub(super) fn read_panner_peak_levels(lg: sequencer::audiograph::LiveGraphPtr, node_ids: &[i32]) -> Vec<f64> {
    node_ids
        .iter()
        .map(|&node_id| read_panner_peak_level(lg, node_id))
        .collect()
}

pub(super) fn read_panner_peak_level(lg: sequencer::audiograph::LiveGraphPtr, node_id: i32) -> f64 {
    const PANNER_STATE_LEN: usize = sequencer::effects::stereo_panner::STEREO_PANNER_STATE_SIZE;
    const PANNER_STATE_BYTES: usize = PANNER_STATE_LEN * std::mem::size_of::<f32>();
    if node_id < 0 {
        return 0.0;
    }
    let mut state_size = 0usize;
    let mut state = [0.0_f32; PANNER_STATE_LEN];
    let copied = unsafe {
        sequencer::audiograph::get_node_state_into(
            lg.0,
            node_id,
            state.as_mut_ptr().cast(),
            PANNER_STATE_BYTES,
            &mut state_size as *mut usize,
        )
    };
    if !copied || state_size < PANNER_STATE_BYTES {
        return 0.0;
    }
    let peak = state[sequencer::effects::stereo_panner::STATE_PEAK_L]
        .max(state[sequencer::effects::stereo_panner::STATE_PEAK_R]);
    meter_display_level(peak)
}

pub(crate) fn read_track_peak_levels(
    lg: sequencer::audiograph::LiveGraphPtr,
    track_nodes: &[app::TrackNodeIds],
) -> Vec<f64> {
    track_nodes
        .iter()
        .map(|nodes| read_panner_peak_level(lg, nodes.delay_id))
        .collect()
}

pub(crate) fn rack_slot_peak_field(track: usize, slot_idx: usize) -> String {
    format!("rack-slot-peak-{track}-{slot_idx}")
}

/// Per-track, per-slot peak levels read from each rack slot's panner node.
/// Tracks without a rack yield an empty inner vec.
pub(crate) fn read_rack_slot_peak_levels(
    lg: sequencer::audiograph::LiveGraphPtr,
    app: &app::App,
) -> Vec<Vec<f64>> {
    app.graph
        .track_node_ids
        .iter()
        .map(|track_nodes| {
            track_nodes
                .rack_slots
                .iter()
                .map(|nodes| read_panner_peak_level(lg, nodes.slot_pan_id))
                .collect()
        })
        .collect()
}

pub(crate) fn sync_rack_slot_peak_field_delta(
    rt: &mut Runtime,
    previous: &[Vec<f64>],
    levels: &[Vec<f64>],
) -> bool {
    let mut effects_dirty = false;
    for (track, slots) in levels.iter().enumerate() {
        let prev_slots = previous.get(track);
        for (slot_idx, &level) in slots.iter().enumerate() {
            let changed = prev_slots
                .and_then(|prev| prev.get(slot_idx))
                .is_none_or(|&old_level| old_level != level);
            if changed {
                effects_dirty |= rt
                    .set_reactive(
                        "SEQ",
                        &rack_slot_peak_field(track, slot_idx),
                        Value::Number(level),
                    )
                    .effects_dirty;
            }
        }
        // Zero out slots that disappeared so stale meters don't stick.
        if let Some(prev) = prev_slots {
            for slot_idx in slots.len()..prev.len() {
                effects_dirty |= rt
                    .set_reactive(
                        "SEQ",
                        &rack_slot_peak_field(track, slot_idx),
                        Value::Number(0.0),
                    )
                    .effects_dirty;
            }
        }
    }
    effects_dirty
}

pub(crate) fn read_bus_peak_levels(
    lg: sequencer::audiograph::LiveGraphPtr,
    bus_nodes: &[app::BusNodeIds],
) -> Vec<f64> {
    const STATE_LEN: usize = sequencer::effects::peak_meter::PEAK_METER_STATE_SIZE;
    const STATE_BYTES: usize = STATE_LEN * std::mem::size_of::<f32>();
    bus_nodes
        .iter()
        .map(|bus| {
            if bus.meter_id < 0 {
                return 0.0;
            }
            let mut state_size = 0usize;
            let mut state = [0.0_f32; STATE_LEN];
            let copied = unsafe {
                sequencer::audiograph::get_node_state_into(
                    lg.0,
                    bus.meter_id,
                    state.as_mut_ptr().cast(),
                    STATE_BYTES,
                    &mut state_size as *mut usize,
                )
            };
            if !copied || state_size < STATE_BYTES {
                return 0.0;
            }
            let peak = state[sequencer::effects::peak_meter::STATE_PEAK_L]
                .max(state[sequencer::effects::peak_meter::STATE_PEAK_R]);
            meter_display_level(peak)
        })
        .collect()
}

pub(crate) fn modulator_phase_field(track: usize) -> String {
    format!("modulator-phase-{track}")
}

pub(crate) fn modulator_level_field(track: usize) -> String {
    format!("modulator-level-{track}")
}

pub(crate) fn read_modulator_display_values(
    lg: sequencer::audiograph::LiveGraphPtr,
    app: &app::App,
) -> (Vec<f64>, Vec<f64>) {
    let mut phases = Vec::with_capacity(app.graph.track_node_ids.len());
    let mut levels = Vec::with_capacity(app.graph.track_node_ids.len());
    for (nodes, instrument_type) in app
        .graph
        .track_node_ids
        .iter()
        .zip(app.graph.track_instrument_types.iter())
    {
        if *instrument_type == sequencer::sequencer::InstrumentType::Modulator {
            let (phase, level) = read_modulator_display_value(lg, nodes.mod_env_id);
            phases.push(phase);
            levels.push(level);
        } else {
            phases.push(0.0);
            levels.push(0.0);
        }
    }
    (phases, levels)
}

pub(super) fn read_modulator_display_value(
    lg: sequencer::audiograph::LiveGraphPtr,
    node_id: i32,
) -> (f64, f64) {
    const STATE_LEN: usize = sequencer::instruments::track_modulator::MODULATOR_ENVELOPE_STATE_SIZE;
    const STATE_BYTES: usize = STATE_LEN * std::mem::size_of::<f32>();
    if node_id < 0 {
        return (0.0, 0.0);
    }
    let mut state_size = 0usize;
    let mut state = [0.0_f32; STATE_LEN];
    let copied = unsafe {
        sequencer::audiograph::get_node_state_into(
            lg.0,
            node_id,
            state.as_mut_ptr().cast(),
            STATE_BYTES,
            &mut state_size as *mut usize,
        )
    };
    if !copied || state_size < STATE_BYTES {
        return (0.0, 0.0);
    }
    decode_modulator_display_state(&state)
}

pub(super) fn decode_modulator_display_state(state: &[f32]) -> (f64, f64) {
    let phase = state
        .get(sequencer::instruments::track_modulator::STATE_DISPLAY_PHASE)
        .copied()
        .unwrap_or(0.0);
    let level = state
        .get(sequencer::instruments::track_modulator::STATE_VALUE)
        .copied()
        .unwrap_or(0.0);
    (
        quantize_modulator_unit_value(phase),
        quantize_modulator_unit_value(level),
    )
}

pub(super) fn quantize_modulator_unit_value(value: f32) -> f64 {
    ((value.clamp(0.0, 1.0) * 128.0).round() / 128.0) as f64
}

pub(crate) fn build_track_peaks_value(levels: &[f64]) -> Value {
    let items: Vec<Rc<RefCell<Value>>> = levels
        .iter()
        .map(|&level| Rc::new(RefCell::new(Value::Number(level))))
        .collect();
    Value::List(items)
}

pub(crate) fn sync_modulator_phase_fields(rt: &mut Runtime, phases: &[f64]) -> bool {
    let mut effects_dirty = false;
    for (idx, &phase) in phases.iter().enumerate() {
        effects_dirty |= rt
            .set_reactive("SEQ", &modulator_phase_field(idx), Value::Number(phase))
            .effects_dirty;
    }
    effects_dirty
}

pub(crate) fn sync_modulator_level_fields(rt: &mut Runtime, levels: &[f64]) -> bool {
    let mut effects_dirty = false;
    for (idx, &level) in levels.iter().enumerate() {
        effects_dirty |= rt
            .set_reactive("SEQ", &modulator_level_field(idx), Value::Number(level))
            .effects_dirty;
    }
    effects_dirty
}

pub(crate) fn sync_modulator_phase_field_delta(
    rt: &mut Runtime,
    previous: &[f64],
    phases: &[f64],
) -> bool {
    let mut effects_dirty = false;
    if previous.len() != phases.len() {
        effects_dirty |= sync_modulator_phase_fields(rt, phases);
        for idx in phases.len()..previous.len() {
            effects_dirty |= rt
                .set_reactive("SEQ", &modulator_phase_field(idx), Value::Number(0.0))
                .effects_dirty;
        }
        return effects_dirty;
    }

    for (idx, (&old_phase, &phase)) in previous.iter().zip(phases.iter()).enumerate() {
        if old_phase != phase {
            effects_dirty |= rt
                .set_reactive("SEQ", &modulator_phase_field(idx), Value::Number(phase))
                .effects_dirty;
        }
    }
    effects_dirty
}

pub(crate) fn sync_modulator_level_field_delta(
    rt: &mut Runtime,
    previous: &[f64],
    levels: &[f64],
) -> bool {
    let mut effects_dirty = false;
    if previous.len() != levels.len() {
        effects_dirty |= sync_modulator_level_fields(rt, levels);
        for idx in levels.len()..previous.len() {
            effects_dirty |= rt
                .set_reactive("SEQ", &modulator_level_field(idx), Value::Number(0.0))
                .effects_dirty;
        }
        return effects_dirty;
    }

    for (idx, (&old_level, &level)) in previous.iter().zip(levels.iter()).enumerate() {
        if old_level != level {
            effects_dirty |= rt
                .set_reactive("SEQ", &modulator_level_field(idx), Value::Number(level))
                .effects_dirty;
        }
    }
    effects_dirty
}

pub(crate) fn sync_track_peak_fields(rt: &mut Runtime, levels: &[f64]) -> bool {
    let mut effects_dirty = false;
    for (idx, &level) in levels.iter().enumerate() {
        effects_dirty |= rt
            .set_reactive("SEQ", &format!("track-peak-{idx}"), Value::Number(level))
            .effects_dirty;
    }
    effects_dirty
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct VisualizationLiveness {
    neural: bool,
    graph: bool,
    track_output: bool,
}

pub(crate) fn sync_neural_visualization_fields(
    rt: &mut Runtime,
    state: &Arc<SequencerState>,
    previous_liveness: &mut VisualizationLiveness,
) -> bool {
    let liveness = VisualizationLiveness {
        neural: state.has_neural_visualization(),
        graph: state.has_graph_visualizations(),
        track_output: state.has_track_output_events(),
    };
    let mut effects_dirty = false;

    if liveness.neural || previous_liveness.neural {
        effects_dirty |= rt
            .set_reactive(
                "SEQ",
                "neural-energy-matrix",
                build_neural_energy_matrix_value(state),
            )
            .effects_dirty;
        effects_dirty |= rt
            .set_reactive(
                "SEQ",
                "neural-trigger-matrix",
                build_neural_trigger_matrix_value(state),
            )
            .effects_dirty;
        effects_dirty |= rt
            .set_reactive(
                "SEQ",
                "neural-dampening-matrix",
                build_neural_dampening_matrix_value(state),
            )
            .effects_dirty;
    }
    if liveness.graph || previous_liveness.graph {
        effects_dirty |= rt
            .set_reactive(
                "SEQ",
                "graph-visualizations",
                build_graph_visualizations_value(state),
            )
            .effects_dirty;
    }
    if liveness.track_output || previous_liveness.track_output {
        effects_dirty |= rt
            .set_reactive(
                "SEQ",
                "track-events",
                build_track_output_events_value(state),
            )
            .effects_dirty;
        effects_dirty |= rt
            .set_reactive(
                "SEQ",
                "track-event-current-beat",
                build_track_output_current_beat_value(state),
            )
            .effects_dirty;
    }
    *previous_liveness = liveness;
    effects_dirty
}

pub(crate) fn sync_bus_peak_fields(rt: &mut Runtime, levels: &[f64]) -> bool {
    let mut effects_dirty = false;
    for (idx, &level) in levels.iter().enumerate() {
        effects_dirty |= rt
            .set_reactive("SEQ", &format!("bus-peak-{idx}"), Value::Number(level))
            .effects_dirty;
    }
    effects_dirty
}

pub(crate) fn sync_track_peak_field_delta(
    rt: &mut Runtime,
    previous: &[f64],
    levels: &[f64],
) -> bool {
    let mut effects_dirty = false;
    if previous.len() != levels.len() {
        effects_dirty |= sync_track_peak_fields(rt, levels);
        for idx in levels.len()..previous.len() {
            effects_dirty |= rt
                .set_reactive("SEQ", &format!("track-peak-{idx}"), Value::Number(0.0))
                .effects_dirty;
        }
        return effects_dirty;
    }

    for (idx, (&old_level, &level)) in previous.iter().zip(levels.iter()).enumerate() {
        if old_level != level {
            effects_dirty |= rt
                .set_reactive("SEQ", &format!("track-peak-{idx}"), Value::Number(level))
                .effects_dirty;
        }
    }
    effects_dirty
}

pub(crate) fn sync_bus_peak_field_delta(
    rt: &mut Runtime,
    previous: &[f64],
    levels: &[f64],
) -> bool {
    let mut effects_dirty = false;
    if previous.len() != levels.len() {
        effects_dirty |= sync_bus_peak_fields(rt, levels);
        for idx in levels.len()..previous.len() {
            effects_dirty |= rt
                .set_reactive("SEQ", &format!("bus-peak-{idx}"), Value::Number(0.0))
                .effects_dirty;
        }
        return effects_dirty;
    }

    for (idx, (&old_level, &level)) in previous.iter().zip(levels.iter()).enumerate() {
        if old_level != level {
            effects_dirty |= rt
                .set_reactive("SEQ", &format!("bus-peak-{idx}"), Value::Number(level))
                .effects_dirty;
        }
    }
    effects_dirty
}

pub(crate) fn sync_playhead_fields(rt: &mut Runtime, playhead: usize, num_steps: usize) -> bool {
    let clamped_steps = num_steps.max(1).min(MAX_STEPS);
    let active_step = playhead.min(clamped_steps.saturating_sub(1));
    let mut effects_dirty = rt
        .set_reactive(
            "SEQ",
            "playhead-page",
            Value::Number((active_step / PAGE_SIZE) as f64),
        )
        .effects_dirty;
    effects_dirty |= rt
        .set_reactive("SEQ", "playhead", Value::Number(active_step as f64))
        .effects_dirty;
    for idx in 0..MAX_STEPS {
        effects_dirty |= rt
            .set_reactive(
                "SEQ",
                &format!("playhead-active-{idx}"),
                Value::Bool(idx == active_step && idx < clamped_steps),
            )
            .effects_dirty;
    }
    effects_dirty
}

pub(crate) fn sync_playhead_field_delta(
    rt: &mut Runtime,
    prev_playhead: usize,
    playhead: usize,
    num_steps: usize,
) -> bool {
    let clamped_steps = num_steps.max(1).min(MAX_STEPS);
    let prev_active = prev_playhead.min(clamped_steps.saturating_sub(1));
    let active_step = playhead.min(clamped_steps.saturating_sub(1));
    let mut effects_dirty = rt
        .set_reactive(
            "SEQ",
            "playhead-page",
            Value::Number((active_step / PAGE_SIZE) as f64),
        )
        .effects_dirty;
    effects_dirty |= rt
        .set_reactive("SEQ", "playhead", Value::Number(active_step as f64))
        .effects_dirty;
    if prev_active != active_step {
        effects_dirty |= rt
            .set_reactive(
                "SEQ",
                &format!("playhead-active-{prev_active}"),
                Value::Bool(false),
            )
            .effects_dirty;
        effects_dirty |= rt
            .set_reactive(
                "SEQ",
                &format!("playhead-active-{active_step}"),
                Value::Bool(true),
            )
            .effects_dirty;
    }
    effects_dirty
}
