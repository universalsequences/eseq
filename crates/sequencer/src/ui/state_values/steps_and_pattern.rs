use super::*;

pub(super) fn sampler_modulation_depth_display_range(
    depth_desc: &sequencer::effects::ParamDescriptor,
    target: &sequencer::effects::InstrumentModulationTarget,
) -> (f32, f32) {
    (
        depth_desc.stored_to_user(target.depth_min),
        depth_desc.stored_to_user(target.depth_max),
    )
}

pub(super) fn instrument_modulation_depth_display_range(
    target: &sequencer::effects::InstrumentModulationTarget,
) -> (f32, f32) {
    // Custom-instrument manifests define modulation depth ranges in display
    // units already; sampler ranges are stored in DSP units and scaled above.
    (target.depth_min, target.depth_max)
}

/// A modulation lane's depth range in display units: a sampler's lanes
/// store DSP units (scaled), every other instrument's display units. Shared
/// by the rack panel and the host kinds' `mod-target`.
pub(crate) fn mod_target_depth_range(
    depth_desc: &sequencer::effects::ParamDescriptor,
    target: &sequencer::effects::InstrumentModulationTarget,
    sampler: bool,
) -> (f32, f32) {
    if sampler {
        sampler_modulation_depth_display_range(depth_desc, target)
    } else {
        instrument_modulation_depth_display_range(target)
    }
}

pub(super) fn modulation_routing_param_indices(
    desc: &sequencer::effects::EffectDescriptor,
) -> std::collections::HashSet<usize> {
    let mut indices = std::collections::HashSet::new();
    for target in &desc.instrument_modulation_targets {
        indices.insert(target.depth_param_idx);
        if let Some(source_param_idx) = target.source_param_idx {
            indices.insert(source_param_idx);
        }
        if let Some(active_param_idx) = target.active_param_idx {
            indices.insert(active_param_idx);
        }
    }
    indices
}

/// Build a Lisp Value::List of bools from the step pattern for a given track.
pub(crate) fn build_steps_value(state: &Arc<SequencerState>, track: usize) -> Value {
    let items: Vec<Rc<RefCell<Value>>> = (0..MAX_STEPS)
        .map(|s| {
            Rc::new(RefCell::new(Value::Bool(
                state.pattern.patterns[track].is_active(s),
            )))
        })
        .collect();
    Value::List(items)
}

/// Build a list-of-lists of bools: one step list per track for the *sequencer* buffer.
pub(crate) fn build_all_track_steps_value(state: &Arc<SequencerState>, app: &app::App) -> Value {
    let tracks: Vec<Rc<RefCell<Value>>> = (0..app.tracks.len())
        .map(|t| {
            let steps: Vec<Rc<RefCell<Value>>> = (0..MAX_STEPS)
                .map(|s| {
                    Rc::new(RefCell::new(Value::Bool(
                        state.pattern.patterns[t].is_active(s),
                    )))
                })
                .collect();
            Rc::new(RefCell::new(Value::List(steps)))
        })
        .collect();
    Value::List(tracks)
}

#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct ReactiveSetStats {
    pub calls: usize,
    pub effects_dirty: usize,
    pub widgets_dirty: usize,
}

impl ReactiveSetStats {
    pub(super) fn note(&mut self, result: ReactiveSetResult) {
        self.calls += 1;
        if result.effects_dirty {
            self.effects_dirty += 1;
        }
        if result.widgets_dirty {
            self.widgets_dirty += 1;
        }
    }
}

#[derive(Clone, Debug, Default)]
pub(crate) struct AllTrackSequencerSyncProfile {
    pub elapsed: Duration,
    pub track_steps: Duration,
    pub track_num_steps: Duration,
    pub track_duration_spans: Duration,
    pub track_step_has_plocks: Duration,
    pub track_playheads: Duration,
    pub track_velocities: Duration,
    pub track_durations: Duration,
    pub track_auxas: Duration,
    pub track_transposes: Duration,
    pub track_pans: Duration,
    pub track_syncs: Duration,
    pub track_delays: Duration,
    pub playhead_fields: Duration,
}

pub(crate) fn build_all_track_num_steps_value(
    state: &Arc<SequencerState>,
    app: &app::App,
) -> Value {
    let items: Vec<Rc<RefCell<Value>>> = (0..app.tracks.len())
        .map(|t| {
            Rc::new(RefCell::new(Value::Number(
                state.pattern.track_params[t].get_num_steps() as f64,
            )))
        })
        .collect();
    Value::List(items)
}

pub(super) fn resolved_track_timebase_label(
    state: &Arc<SequencerState>,
    track: usize,
    current_track_idx: usize,
    selected_step: Option<usize>,
) -> String {
    let timebase = if track == current_track_idx {
        selected_step
            .and_then(|step| state.pattern.timebase_plocks[track].get(step))
            .unwrap_or_else(|| state.pattern.track_params[track].get_timebase())
    } else {
        state.pattern.track_params[track].get_timebase()
    };
    timebase.label().to_string()
}

pub(super) fn build_track_timebase_labels_value(
    state: &Arc<SequencerState>,
    track_count: usize,
    current_track_idx: usize,
    selected_step: Option<usize>,
) -> Value {
    let items: Vec<Rc<RefCell<Value>>> = (0..track_count)
        .map(|track| {
            Rc::new(RefCell::new(Value::String(resolved_track_timebase_label(
                state,
                track,
                current_track_idx,
                selected_step,
            ))))
        })
        .collect();
    Value::List(items)
}

pub(crate) fn build_track_duration_spans_value(state: &Arc<SequencerState>, track: usize) -> Value {
    let num_steps = state.pattern.track_params[track]
        .get_num_steps()
        .min(MAX_STEPS);
    let held = track_held_steps(state, track, num_steps);
    let spans: Vec<Rc<RefCell<Value>>> = (0..MAX_STEPS)
        .map(|step| Rc::new(RefCell::new(Value::Bool(held.get(step) == Some(&true)))))
        .collect();
    Value::List(spans)
}

pub(crate) fn track_step_duration_covered(
    state: &Arc<SequencerState>,
    track: usize,
    target_step: usize,
) -> bool {
    let num_steps = state.pattern.track_params[track]
        .get_num_steps()
        .min(MAX_STEPS);
    target_step < num_steps && track_held_steps(state, track, target_step + 1)[target_step]
}

/// Per step of the first `num_steps`, whether it lies inside an active
/// step's duration, that step included (`seq-track-step-duration-*`,
/// `step.held`): one scan.
pub(crate) fn track_held_steps(
    state: &SequencerState,
    track: usize,
    num_steps: usize,
) -> Vec<bool> {
    let mut held = Vec::new();
    fill_track_held_steps(state, track, num_steps, &mut held);
    held
}

/// [`track_held_steps`] into a reused buffer.
pub(crate) fn fill_track_held_steps(
    state: &SequencerState,
    track: usize,
    num_steps: usize,
    held: &mut Vec<bool>,
) {
    let pattern = &state.pattern.patterns[track];
    let data = &state.pattern.step_data[track];
    // How far the active steps so far reach: a step is held while
    // `source + duration > step` for some active `source <= step`.
    let mut reach = f64::NEG_INFINITY;
    held.clear();
    held.extend((0..num_steps).map(|step| {
        if pattern.is_active(step) {
            let duration = data.get(step, StepParam::Duration).max(0.0) as f64;
            reach = reach.max(step as f64 + duration);
        }
        reach > step as f64
    }));
}

/// The delete target that selects a mod route (`route.selected`).
pub(crate) fn mod_route_delete_target(
    connection: &sequencer::sequencer::ModConnection,
) -> ActiveDeleteTarget {
    ActiveDeleteTarget::ModRoute {
        source: connection.source_track,
        destination: connection.destination,
        input: connection.dest_input,
    }
}

/// Whether the delete target selects `track`'s pool pattern `pattern_id`
/// (`cell.selected`).
pub(crate) fn track_pattern_cell_selected(
    active_delete_target: Option<&ActiveDeleteTarget>,
    track: usize,
    pattern_id: u64,
) -> bool {
    matches!(
        active_delete_target,
        Some(ActiveDeleteTarget::TrackPattern {
            track: selected_track,
            pattern_id: selected_pattern_id,
        }) if *selected_track == track && selected_pattern_id.0 == pattern_id
    )
}

pub(crate) fn mixer_track_delete_target_selected(
    active_delete_target: Option<&ActiveDeleteTarget>,
    track: usize,
) -> bool {
    match active_delete_target {
        Some(ActiveDeleteTarget::MixerTrack { track: selected }) => *selected == track,
        Some(ActiveDeleteTarget::MixerTracks { tracks }) => tracks.contains(&track),
        _ => false,
    }
}

#[cfg(test)]
mod delete_target_binding_tests {
    use super::*;

    #[test]
    fn multiple_mixer_track_target_selects_each_member_only() {
        let target = ActiveDeleteTarget::MixerTracks { tracks: vec![1, 3] };
        assert!(!mixer_track_delete_target_selected(Some(&target), 0));
        assert!(mixer_track_delete_target_selected(Some(&target), 1));
        assert!(!mixer_track_delete_target_selected(Some(&target), 2));
        assert!(mixer_track_delete_target_selected(Some(&target), 3));
    }
}

/// Builds the `SEQ.selected-tracks` reactive list (sorted track indices).
pub(crate) fn build_selected_tracks_value(selected: &HashSet<usize>) -> Value {
    let tracks = sorted_selected_tracks(selected);
    list_value(tracks.into_iter().map(|t| Value::Number(t as f64)))
}

/// The multi-track selection in track order (`SEQ.selected-tracks`,
/// `selection.tracks`).
pub(crate) fn sorted_selected_tracks(selected: &HashSet<usize>) -> Vec<usize> {
    let mut tracks: Vec<usize> = selected.iter().copied().collect();
    tracks.sort_unstable();
    tracks
}

/// Refreshes `SEQ.selected-tracks` (the highlight is `track.in-selection`).
pub(crate) fn sync_selected_tracks_bindings(rt: &mut Runtime, selected: &HashSet<usize>) {
    rt.set_reactive(
        "SEQ",
        "selected-tracks",
        build_selected_tracks_value(selected),
    );
}

pub(crate) fn set_current_track_reactive(rt: &mut Runtime, current_track_idx: usize) {
    rt.set_reactive(
        "SEQ",
        "current-track",
        Value::Number(current_track_idx as f64),
    );
}
