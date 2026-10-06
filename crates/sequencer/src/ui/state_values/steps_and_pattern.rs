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
pub(crate) struct AllTrackStepBindingSyncProfile {
    pub elapsed: Duration,
    pub active_elapsed: Duration,
    pub duration_elapsed: Duration,
    pub plocked_elapsed: Duration,
    pub selected_elapsed: Duration,
    pub slider_elapsed: Duration,
    pub haptic_elapsed: Duration,
    pub active_sets: ReactiveSetStats,
    pub duration_sets: ReactiveSetStats,
    pub plocked_sets: ReactiveSetStats,
    pub selected_sets: ReactiveSetStats,
    pub slider_sets: ReactiveSetStats,
    pub haptic_sets: ReactiveSetStats,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct AllTrackSequencerSyncProfile {
    pub elapsed: Duration,
    pub track_steps: Duration,
    pub track_num_steps: Duration,
    pub track_timebases: Duration,
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
    pub step_bindings: AllTrackStepBindingSyncProfile,
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

pub(crate) fn build_all_track_timebase_labels_value(
    state: &Arc<SequencerState>,
    app: &app::App,
    current_track_idx: usize,
    selected_steps: &Arc<Mutex<HashSet<usize>>>,
) -> Value {
    let selected_step = {
        let selected = selected_steps.lock().unwrap();
        selected.iter().copied().min()
    };
    build_track_timebase_labels_value(state, app.tracks.len(), current_track_idx, selected_step)
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

pub(crate) fn track_step_active_field(track: usize, step: usize) -> String {
    format!("seq-track-step-active-{track}-{step}")
}

/// Registry field caching a hex digest of all four per-step binding lanes for
/// a track. When it is unchanged, the per-step field writes are skipped
/// entirely; single-step sync paths invalidate it by writing Nil.
pub(crate) fn track_step_binding_rev_field(track: usize) -> String {
    format!("seq-track-step-binding-rev-{track}")
}

pub(crate) fn track_step_duration_field(track: usize, step: usize) -> String {
    format!("seq-track-step-duration-{track}-{step}")
}

pub(crate) fn track_step_plocked_field(track: usize, step: usize) -> String {
    format!("seq-track-step-plocked-{track}-{step}")
}

pub(crate) fn track_step_plock_kind_field(track: usize, step: usize) -> String {
    format!("seq-track-step-plock-kind-{track}-{step}")
}

pub(crate) fn track_step_variant_color_field(
    track: usize,
    step: usize,
    channel: char,
) -> String {
    format!("seq-track-step-variant-{channel}-{track}-{step}")
}

pub(crate) fn track_step_selected_field(track: usize, step: usize) -> String {
    format!("seq-track-step-selected-{track}-{step}")
}

pub(crate) fn track_selected_field(track: usize) -> String {
    format!("track-selected-{track}")
}

pub(crate) fn rack_slot_delete_target_field(track: usize, slot: usize) -> String {
    format!("rack-slot-delete-target-{track}-{slot}")
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

pub(crate) fn sync_mixer_delete_target_binding_fields(
    rt: &mut Runtime,
    track_count: usize,
    state: &Arc<SequencerState>,
    active_delete_target: Option<&ActiveDeleteTarget>,
) {
    let rack_tracks = state.pattern.rack_tracks.lock().unwrap();
    for track in 0..track_count {
        let rack_slot_count = rack_tracks
            .get(track)
            .and_then(|rack| rack.as_ref())
            .map(|rack| rack.slots.len())
            .unwrap_or(0);
        for slot in 0..sequencer::sequencer::MAX_RACK_SLOTS {
            rt.set_reactive(
                "SEQ",
                &rack_slot_delete_target_field(track, slot),
                Value::Bool(matches!(
                    active_delete_target,
                    Some(ActiveDeleteTarget::RackSlot {
                        track: selected_track,
                        slot: selected_slot,
                    }) if *selected_track == track
                        && *selected_slot == slot
                        && slot < rack_slot_count
                )),
            );
        }
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

pub(crate) fn sync_track_selection_binding_fields(
    rt: &mut Runtime,
    track_count: usize,
    current_track_idx: usize,
) {
    for track in 0..track_count {
        rt.set_reactive(
            "SEQ",
            &track_selected_field(track),
            Value::Bool(track == current_track_idx),
        );
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

/// Lights `track-selected-{i}` for every track in the multi-select set (union
/// with the focused current track) and refreshes `SEQ.selected-tracks`.
pub(crate) fn sync_selected_tracks_bindings(
    rt: &mut Runtime,
    track_count: usize,
    current_track_idx: usize,
    selected: &HashSet<usize>,
) {
    for track in 0..track_count {
        let on = track == current_track_idx || selected.contains(&track);
        rt.set_reactive("SEQ", &track_selected_field(track), Value::Bool(on));
    }
    rt.set_reactive(
        "SEQ",
        "selected-tracks",
        build_selected_tracks_value(selected),
    );
}

/// Builds `SEQ.groups`: one map per group with id, name, color, bus-id, anchor
/// (lowest member index), collapsed flag, ordered member indices, and — for a
/// drum rack (`docs/drum-rack-v2-spec.md`) — the `rack` flag plus its pad map.
///
/// Each pad carries both `member` (its position in `members`, the serialized
/// form) and `track` (the resolved track index), so the UI can badge a member
/// row without re-walking the member list.
pub(crate) fn build_groups_value(groups: &[sequencer::project::ProjectTrackGroup]) -> Value {
    list_value(groups.iter().map(|group| {
        let anchor = group.members.iter().copied().min().unwrap_or(0);
        // Nesting, both ways: `rack-members` are the child racks this group
        // draws inside its own block, `parent` is the group id drawing this
        // one (-1 when it is top level). See docs/drum-rack-v2-spec.md,
        // "Racks inside track groups".
        let parent = sequencer::project::rack_parent(groups, group.id)
            .map_or(-1.0, |parent| groups[parent].id as f64);
        map_value([
            ("id", Value::Number(group.id as f64)),
            ("name", Value::String(group.name.clone().into())),
            (
                "color",
                list_value(themed_track_rgb(group.color).into_iter().map(|c| Value::Number(c as f64))),
            ),
            ("bus-id", Value::Number(group.bus_id as f64)),
            ("anchor", Value::Number(anchor as f64)),
            ("collapsed", Value::Bool(group.collapsed)),
            (
                "members",
                list_value(group.members.iter().map(|m| Value::Number(*m as f64))),
            ),
            ("rack", Value::Bool(group.is_rack())),
            ("pads", build_rack_pads_value(group)),
            (
                "rack-members",
                list_value(
                    group
                        .rack_members
                        .iter()
                        .map(|id| Value::Number(*id as f64)),
                ),
            ),
            ("parent", Value::Number(parent)),
        ])
    }))
}

/// Builds a rack group's `pads` entry: one map per pad with its note, its note
/// name (the pad badge's label), member position, resolved track index,
/// choke group (`-1` when unassigned) and drum role.
/// Empty for a plain group.
fn build_rack_pads_value(group: &sequencer::project::ProjectTrackGroup) -> Value {
    let Some(rack) = group.rack.as_ref() else {
        return list_value(std::iter::empty());
    };
    list_value(rack.pads.iter().enumerate().map(|(pad_idx, pad)| {
        let track = group.members.get(pad.member).copied();
        let choke = rack
            .choke_groups
            .get(pad_idx)
            .copied()
            .flatten()
            .map(|g| g as f64)
            .unwrap_or(-1.0);
        map_value([
            ("pad-note", Value::Number(pad.pad_note as f64)),
            (
                "label",
                Value::String(super::drum_rack::drum_rack_pad_label(pad.pad_note)),
            ),
            ("member", Value::Number(pad.member as f64)),
            (
                "track",
                Value::Number(track.map(|t| t as f64).unwrap_or(-1.0)),
            ),
            ("choke", Value::Number(choke)),
            // Pad role (docs/rack-groove-spec.md, "Pad roles"): the explicit
            // key ("" = Standard), the effective role's short tag and name
            // ("" when neither the pad nor the standard layout has one), and
            // the standard layout's own name for the "Standard (...)" entry.
            (
                "role",
                Value::String(pad.role.map_or("", |role| role.key()).into()),
            ),
            (
                "role-tag",
                Value::String(pad.effective_role().map_or("", |role| role.tag()).into()),
            ),
            (
                "role-label",
                Value::String(pad.effective_role().map_or("", |role| role.label()).into()),
            ),
            (
                "standard-role-label",
                Value::String(
                    sequencer::project::PadRole::standard(pad.pad_note)
                        .map_or("", |role| role.label())
                        .into(),
                ),
            ),
        ])
    }))
}

/// Builds `SEQ.group-collapsed`: parallel list of collapsed flags per group.
pub(crate) fn build_group_collapsed_value(
    groups: &[sequencer::project::ProjectTrackGroup],
) -> Value {
    list_value(groups.iter().map(|g| Value::Bool(g.collapsed)))
}

pub(crate) fn sync_groups_bindings(
    rt: &mut Runtime,
    groups: &[sequencer::project::ProjectTrackGroup],
    grooves: &[sequencer::project::ProjectGroove],
) {
    rt.set_reactive("SEQ", "groups", build_groups_value(groups));
    rt.set_reactive(
        "SEQ",
        "group-collapsed",
        build_group_collapsed_value(groups),
    );
    // Rack grooves derive from the same rack configs (plus the project
    // groove pool they point into), so every path that republishes group
    // topology (undo/redo, project load, pad-map edits) republishes the
    // groove section too.
    sync_rack_groove_state(rt, groups, grooves);
}

pub(crate) fn set_current_track_reactive(
    rt: &mut Runtime,
    track_count: usize,
    current_track_idx: usize,
) {
    rt.set_reactive(
        "SEQ",
        "current-track",
        Value::Number(current_track_idx as f64),
    );
    sync_track_selection_binding_fields(rt, track_count, current_track_idx);
}

pub(crate) fn track_step_param_slider_field(track: usize, mode: usize, step: usize) -> String {
    format!("seq-track-step-param-slider-{track}-{mode}-{step}")
}

pub(crate) fn track_step_param_haptic_field(track: usize, mode: usize, step: usize) -> String {
    format!("seq-track-step-param-haptic-{track}-{mode}-{step}")
}
