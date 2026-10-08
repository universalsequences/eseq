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

/// The multi-track selection in track order (`selection.tracks`).
pub(crate) fn sorted_selected_tracks(selected: &HashSet<usize>) -> Vec<usize> {
    let mut tracks: Vec<usize> = selected.iter().copied().collect();
    tracks.sort_unstable();
    tracks
}

