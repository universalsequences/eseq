use super::*;
use eseqlisp::runtime::ReactiveSetResult;
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{Duration, Instant};

fn sampler_modulation_depth_display_range(
    depth_desc: &sequencer::effects::ParamDescriptor,
    target: &sequencer::effects::InstrumentModulationTarget,
) -> (f32, f32) {
    (
        depth_desc.stored_to_user(target.depth_min),
        depth_desc.stored_to_user(target.depth_max),
    )
}

fn instrument_modulation_depth_display_range(
    target: &sequencer::effects::InstrumentModulationTarget,
) -> (f32, f32) {
    // Custom-instrument manifests define modulation depth ranges in display
    // units already; sampler ranges are stored in DSP units and scaled above.
    (target.depth_min, target.depth_max)
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
pub(crate) fn build_all_track_steps_value(state: &Arc<SequencerState>, app: &ui::App) -> Value {
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
    fn note(&mut self, result: ReactiveSetResult) {
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

pub(crate) fn build_track_pattern_cells_value(
    state: &Arc<SequencerState>,
    track_count: usize,
) -> Value {
    list_value((0..track_count).map(|track| {
        list_value(
            state
                .track_pattern_cells(track)
                .into_iter()
                .map(|cell| map_value([("id", Value::Number(cell.pattern_id.0 as f64))])),
        )
    }))
}

pub(crate) fn build_all_track_num_steps_value(state: &Arc<SequencerState>, app: &ui::App) -> Value {
    let items: Vec<Rc<RefCell<Value>>> = (0..app.tracks.len())
        .map(|t| {
            Rc::new(RefCell::new(Value::Number(
                state.pattern.track_params[t].get_num_steps() as f64,
            )))
        })
        .collect();
    Value::List(items)
}

fn resolved_track_timebase_label(
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

fn build_track_timebase_labels_value(
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
    app: &ui::App,
    current_track_idx: usize,
    selected_steps: &Arc<Mutex<HashSet<usize>>>,
) -> Value {
    let selected_step = {
        let selected = selected_steps.lock().unwrap();
        selected.iter().copied().min()
    };
    build_track_timebase_labels_value(state, app.tracks.len(), current_track_idx, selected_step)
}

fn build_track_duration_spans_value(state: &Arc<SequencerState>, track: usize) -> Value {
    let num_steps = state.pattern.track_params[track]
        .get_num_steps()
        .min(MAX_STEPS);
    let spans: Vec<Rc<RefCell<Value>>> = (0..MAX_STEPS)
        .map(|target_step| {
            let covered = target_step < num_steps
                && (0..=target_step).any(|source_step| {
                    if !state.pattern.patterns[track].is_active(source_step) {
                        return false;
                    }
                    let duration = state.pattern.step_data[track]
                        .get(source_step, StepParam::Duration)
                        .max(0.0);
                    duration > (target_step - source_step) as f32
                });
            Rc::new(RefCell::new(Value::Bool(covered)))
        })
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
    target_step < num_steps
        && (0..=target_step).any(|source_step| {
            if !state.pattern.patterns[track].is_active(source_step) {
                return false;
            }
            let duration = state.pattern.step_data[track]
                .get(source_step, StepParam::Duration)
                .max(0.0);
            duration > (target_step - source_step) as f32
        })
}

pub(crate) fn track_step_active_field(track: usize, step: usize) -> String {
    format!("seq-track-step-active-{track}-{step}")
}

pub(crate) fn track_step_duration_field(track: usize, step: usize) -> String {
    format!("seq-track-step-duration-{track}-{step}")
}

pub(crate) fn track_step_plocked_field(track: usize, step: usize) -> String {
    format!("seq-track-step-plocked-{track}-{step}")
}

pub(crate) fn track_step_selected_field(track: usize, step: usize) -> String {
    format!("seq-track-step-selected-{track}-{step}")
}

pub(crate) fn track_selected_field(track: usize) -> String {
    format!("track-selected-{track}")
}

pub(crate) fn mixer_track_delete_target_field(track: usize) -> String {
    format!("mixer-track-delete-target-{track}")
}

pub(crate) fn track_pattern_cell_active_field(track: usize, pattern_id: u64) -> String {
    format!("track-pattern-cell-active-{track}-{pattern_id}")
}

pub(crate) fn track_pattern_cell_assigned_field(track: usize, pattern_id: u64) -> String {
    format!("track-pattern-cell-assigned-{track}-{pattern_id}")
}

pub(crate) fn track_pattern_cell_override_field(track: usize, pattern_id: u64) -> String {
    format!("track-pattern-cell-override-{track}-{pattern_id}")
}

pub(crate) fn track_pattern_cell_selected_field(track: usize, pattern_id: u64) -> String {
    format!("track-pattern-cell-selected-{track}-{pattern_id}")
}

pub(crate) fn selected_mod_routes_value(
    active_delete_target: Option<&ActiveDeleteTarget>,
) -> Value {
    match active_delete_target {
        Some(ActiveDeleteTarget::ModRoute {
            source,
            dest,
            input,
        }) => list_value([map_value([
            ("source", Value::Number(*source as f64)),
            ("dest", Value::Number(*dest as f64)),
            ("input", Value::Number(*input as f64)),
        ])]),
        _ => list_value(Vec::<Value>::new()),
    }
}

pub(crate) fn sync_track_pattern_cell_state_fields(
    rt: &mut Runtime,
    state: &Arc<SequencerState>,
    track_count: usize,
) {
    for track in 0..track_count {
        for cell in state.track_pattern_cells(track) {
            let pattern_id = cell.pattern_id.0;
            rt.set_reactive(
                "SEQ",
                &track_pattern_cell_active_field(track, pattern_id),
                Value::Bool(cell.active_effective),
            );
            rt.set_reactive(
                "SEQ",
                &track_pattern_cell_assigned_field(track, pattern_id),
                Value::Bool(cell.assigned_to_current_scene),
            );
            rt.set_reactive(
                "SEQ",
                &track_pattern_cell_override_field(track, pattern_id),
                Value::Bool(cell.overridden),
            );
        }
    }
}

pub(crate) fn sync_track_pattern_cell_selected_fields(
    rt: &mut Runtime,
    state: &Arc<SequencerState>,
    track_count: usize,
    active_delete_target: Option<&ActiveDeleteTarget>,
) {
    for track in 0..track_count {
        for cell in state.track_pattern_cells(track) {
            let pattern_id = cell.pattern_id.0;
            rt.set_reactive(
                "SEQ",
                &track_pattern_cell_selected_field(track, pattern_id),
                Value::Bool(matches!(
                    active_delete_target,
                    Some(ActiveDeleteTarget::TrackPattern {
                        track: selected_track,
                        pattern_id: selected_pattern_id,
                    }) if *selected_track == track && selected_pattern_id.0 == pattern_id
                )),
            );
        }
    }
}

pub(crate) fn sync_mixer_delete_target_binding_fields(
    rt: &mut Runtime,
    track_count: usize,
    state: &Arc<SequencerState>,
    active_delete_target: Option<&ActiveDeleteTarget>,
) {
    for track in 0..track_count {
        rt.set_reactive(
            "SEQ",
            &mixer_track_delete_target_field(track),
            Value::Bool(matches!(
                active_delete_target,
                Some(ActiveDeleteTarget::MixerTrack { track: selected }) if *selected == track
            )),
        );
    }
    sync_track_pattern_cell_selected_fields(rt, state, track_count, active_delete_target);
    rt.set_reactive(
        "SEQ",
        "selected-mod-routes",
        selected_mod_routes_value(active_delete_target),
    );
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct ExpandedStepViewport {
    pub(crate) track: usize,
    pub(crate) track_id: usize,
    pub(crate) page: usize,
    pub(crate) mode: usize,
    pub(crate) cursor_step: usize,
}

#[derive(Debug, Default)]
pub(crate) struct ExpandedStepProjectionRegistry {
    viewports: Mutex<HashMap<usize, ExpandedStepViewport>>,
}

impl ExpandedStepProjectionRegistry {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) fn set_viewport(&self, viewport: ExpandedStepViewport) -> bool {
        let mut viewports = self.viewports.lock().unwrap();
        let changed = viewports.get(&viewport.track_id).copied() != Some(viewport);
        viewports.insert(viewport.track_id, viewport);
        changed
    }

    pub(crate) fn remove_viewport(&self, track_id: usize) -> bool {
        self.viewports.lock().unwrap().remove(&track_id).is_some()
    }

    pub(crate) fn viewport(&self, track_id: usize) -> Option<ExpandedStepViewport> {
        self.viewports.lock().unwrap().get(&track_id).copied()
    }

    pub(crate) fn viewports_for_track(&self, track: usize) -> Vec<ExpandedStepViewport> {
        self.viewports
            .lock()
            .unwrap()
            .values()
            .copied()
            .filter(|viewport| viewport.track == track)
            .collect()
    }

    pub(crate) fn all_viewports(&self) -> Vec<ExpandedStepViewport> {
        self.viewports.lock().unwrap().values().copied().collect()
    }

    pub(crate) fn clear(&self) {
        self.viewports.lock().unwrap().clear();
    }
}

pub(crate) fn expanded_step_slot_index_field(track_id: usize, slot: usize) -> String {
    format!("seqv-slot-step-index-{track_id}-{slot}")
}

pub(crate) fn expanded_step_slot_label_field(track_id: usize, slot: usize) -> String {
    format!("seqv-slot-step-label-{track_id}-{slot}")
}

pub(crate) fn expanded_step_slot_visible_field(track_id: usize, slot: usize) -> String {
    format!("seqv-slot-visible-{track_id}-{slot}")
}

pub(crate) fn expanded_step_slot_active_field(track_id: usize, slot: usize) -> String {
    format!("seqv-slot-active-{track_id}-{slot}")
}

pub(crate) fn expanded_step_slot_plocked_field(track_id: usize, slot: usize) -> String {
    format!("seqv-slot-plocked-{track_id}-{slot}")
}

pub(crate) fn expanded_step_slot_selected_field(track_id: usize, slot: usize) -> String {
    format!("seqv-slot-selected-{track_id}-{slot}")
}

pub(crate) fn expanded_step_slot_playhead_field(track_id: usize, slot: usize) -> String {
    format!("seqv-slot-playhead-active-{track_id}-{slot}")
}

pub(crate) fn expanded_step_slot_cursor_field(track_id: usize, slot: usize) -> String {
    format!("seqv-slot-cursor-active-{track_id}-{slot}")
}

pub(crate) fn expanded_step_slot_param_slider_field(
    track_id: usize,
    mode: usize,
    slot: usize,
) -> String {
    format!("seqv-slot-param-slider-{track_id}-{mode}-{slot}")
}

pub(crate) fn expanded_step_slot_param_haptic_field(
    track_id: usize,
    mode: usize,
    slot: usize,
) -> String {
    format!("seqv-slot-param-haptic-{track_id}-{mode}-{slot}")
}

pub(crate) fn expanded_step_page_active_field(track_id: usize, page: usize) -> String {
    format!("seqv-page-active-{track_id}-{page}")
}

pub(crate) fn visible_slot_for_step(viewport: ExpandedStepViewport, step: usize) -> Option<usize> {
    let first_step = viewport.page.saturating_mul(PAGE_SIZE);
    let slot = step.checked_sub(first_step)?;
    (slot < PAGE_SIZE).then_some(slot)
}

fn expanded_step_param_for_mode(mode: usize) -> StepParam {
    match mode {
        0 => StepParam::Velocity,
        1 => StepParam::Duration,
        2 => StepParam::AuxA,
        3 => StepParam::Transpose,
        4 => StepParam::Pan,
        5 => StepParam::Sync,
        6 => StepParam::Delay,
        _ => StepParam::Velocity,
    }
}

fn expanded_step_param_slider_value(param: StepParam, value: f32) -> f32 {
    if param == StepParam::Duration {
        param.normalize(value)
    } else {
        value
    }
}

pub(crate) fn sync_expanded_step_param_slot(
    rt: &mut Runtime,
    state: &Arc<SequencerState>,
    viewport: ExpandedStepViewport,
    mode: usize,
    slot: usize,
) -> bool {
    let step = viewport.page.saturating_mul(PAGE_SIZE).saturating_add(slot);
    let num_steps = state.pattern.track_params[viewport.track]
        .get_num_steps()
        .min(MAX_STEPS);
    let visible = step < num_steps;
    let param = expanded_step_param_for_mode(mode);
    let value = if visible {
        state.pattern.step_data[viewport.track].get(step, param)
    } else {
        0.0
    };
    let mut dirty = false;
    dirty |= rt
        .set_reactive(
            "SEQ",
            &expanded_step_slot_param_slider_field(viewport.track_id, mode, slot),
            Value::Number(expanded_step_param_slider_value(param, value) as f64),
        )
        .effects_dirty;
    dirty |= rt
        .set_reactive(
            "SEQ",
            &expanded_step_slot_param_haptic_field(viewport.track_id, mode, slot),
            Value::Number(value as f64),
        )
        .effects_dirty;
    dirty
}

pub(crate) fn sync_expanded_step_slot(
    rt: &mut Runtime,
    state: &Arc<SequencerState>,
    app: &ui::App,
    selected_steps: &HashSet<usize>,
    current_track_idx: usize,
    viewport: ExpandedStepViewport,
    slot: usize,
) -> bool {
    let step = viewport.page.saturating_mul(PAGE_SIZE).saturating_add(slot);
    let num_steps = state.pattern.track_params[viewport.track]
        .get_num_steps()
        .min(MAX_STEPS);
    let visible = step < num_steps && step < MAX_STEPS && viewport.track < app.tracks.len();
    let mut dirty = false;
    dirty |= rt
        .set_reactive(
            "SEQ",
            &expanded_step_slot_index_field(viewport.track_id, slot),
            Value::Number(if visible { step as f64 } else { -1.0 }),
        )
        .effects_dirty;
    dirty |= rt
        .set_reactive(
            "SEQ",
            &expanded_step_slot_label_field(viewport.track_id, slot),
            Value::Number((step + 1) as f64),
        )
        .effects_dirty;
    dirty |= rt
        .set_reactive(
            "SEQ",
            &expanded_step_slot_visible_field(viewport.track_id, slot),
            Value::Bool(visible),
        )
        .effects_dirty;
    dirty |= rt
        .set_reactive(
            "SEQ",
            &expanded_step_slot_active_field(viewport.track_id, slot),
            Value::Bool(visible && state.pattern.patterns[viewport.track].is_active(step)),
        )
        .effects_dirty;
    dirty |= rt
        .set_reactive(
            "SEQ",
            &expanded_step_slot_plocked_field(viewport.track_id, slot),
            Value::Bool(
                visible
                    && track_step_has_plock(
                        state,
                        viewport.track,
                        &app.graph.effect_descriptors,
                        step,
                    ),
            ),
        )
        .effects_dirty;
    dirty |= rt
        .set_reactive(
            "SEQ",
            &expanded_step_slot_selected_field(viewport.track_id, slot),
            Value::Bool(
                visible && viewport.track == current_track_idx && selected_steps.contains(&step),
            ),
        )
        .effects_dirty;
    dirty |= rt
        .set_reactive(
            "SEQ",
            &expanded_step_slot_playhead_field(viewport.track_id, slot),
            Value::Bool(visible && step == track_active_playhead_step(state, viewport.track)),
        )
        .effects_dirty;
    dirty |= rt
        .set_reactive(
            "SEQ",
            &expanded_step_slot_cursor_field(viewport.track_id, slot),
            Value::Bool(visible && step == viewport.cursor_step),
        )
        .effects_dirty;
    dirty |= sync_expanded_step_param_slot(rt, state, viewport, viewport.mode, slot);
    dirty
}

pub(crate) fn sync_expanded_step_viewport(
    rt: &mut Runtime,
    state: &Arc<SequencerState>,
    app: &ui::App,
    selected_steps: &HashSet<usize>,
    current_track_idx: usize,
    viewport: ExpandedStepViewport,
) -> bool {
    if viewport.track >= app.tracks.len() {
        return false;
    }
    let mut dirty = false;
    let page_count = track_playhead_row_count(state, viewport.track).max(1);
    for page in 0..((MAX_STEPS + PAGE_SIZE - 1) / PAGE_SIZE) {
        dirty |= rt
            .set_reactive(
                "SEQ",
                &expanded_step_page_active_field(viewport.track_id, page),
                Value::Bool(page == viewport.page && page < page_count),
            )
            .effects_dirty;
    }
    for slot in 0..PAGE_SIZE {
        dirty |= sync_expanded_step_slot(
            rt,
            state,
            app,
            selected_steps,
            current_track_idx,
            viewport,
            slot,
        );
    }
    dirty
}

pub(crate) fn sync_expanded_step_viewport_playhead(
    rt: &mut Runtime,
    state: &Arc<SequencerState>,
    viewport: ExpandedStepViewport,
) -> bool {
    let active_step = track_active_playhead_step(state, viewport.track);
    let num_steps = state.pattern.track_params[viewport.track]
        .get_num_steps()
        .min(MAX_STEPS);
    let mut dirty = false;
    for slot in 0..PAGE_SIZE {
        let step = viewport.page.saturating_mul(PAGE_SIZE).saturating_add(slot);
        dirty |= rt
            .set_reactive(
                "SEQ",
                &expanded_step_slot_playhead_field(viewport.track_id, slot),
                Value::Bool(step < num_steps && step == active_step),
            )
            .effects_dirty;
    }
    dirty
}

pub(crate) fn sync_all_track_step_binding_fields(
    rt: &mut Runtime,
    state: &Arc<SequencerState>,
    app: &ui::App,
    current_track_idx: usize,
    selected_steps: &Arc<Mutex<HashSet<usize>>>,
) {
    sync_all_track_step_binding_fields_inner(
        rt,
        state,
        app,
        current_track_idx,
        selected_steps,
        None,
    );
}

pub(crate) fn sync_all_track_step_binding_fields_profiled(
    rt: &mut Runtime,
    state: &Arc<SequencerState>,
    app: &ui::App,
    current_track_idx: usize,
    selected_steps: &Arc<Mutex<HashSet<usize>>>,
) -> AllTrackStepBindingSyncProfile {
    let mut profile = AllTrackStepBindingSyncProfile::default();
    sync_all_track_step_binding_fields_inner(
        rt,
        state,
        app,
        current_track_idx,
        selected_steps,
        Some(&mut profile),
    );
    profile
}

fn sync_all_track_step_binding_fields_inner(
    rt: &mut Runtime,
    state: &Arc<SequencerState>,
    app: &ui::App,
    current_track_idx: usize,
    selected_steps: &Arc<Mutex<HashSet<usize>>>,
    mut profile: Option<&mut AllTrackStepBindingSyncProfile>,
) {
    let total_started = profile.as_ref().map(|_| Instant::now());
    let selected = selected_steps.lock().unwrap();
    for track in 0..app.tracks.len() {
        let num_steps = state.pattern.track_params[track]
            .get_num_steps()
            .min(MAX_STEPS);
        for step in 0..MAX_STEPS {
            let visible = step < num_steps;
            let started = profile.as_ref().map(|_| Instant::now());
            let result = rt.set_reactive(
                "SEQ",
                &track_step_active_field(track, step),
                Value::Bool(visible && state.pattern.patterns[track].is_active(step)),
            );
            if let Some(profile) = profile.as_deref_mut() {
                profile.active_elapsed += started.expect("profile timer").elapsed();
                profile.active_sets.note(result);
            }

            let started = profile.as_ref().map(|_| Instant::now());
            let result = rt.set_reactive(
                "SEQ",
                &track_step_duration_field(track, step),
                Value::Bool(visible && track_step_duration_covered(state, track, step)),
            );
            if let Some(profile) = profile.as_deref_mut() {
                profile.duration_elapsed += started.expect("profile timer").elapsed();
                profile.duration_sets.note(result);
            }

            let started = profile.as_ref().map(|_| Instant::now());
            let result = rt.set_reactive(
                "SEQ",
                &track_step_plocked_field(track, step),
                Value::Bool(
                    visible
                        && track_step_has_plock(state, track, &app.graph.effect_descriptors, step),
                ),
            );
            if let Some(profile) = profile.as_deref_mut() {
                profile.plocked_elapsed += started.expect("profile timer").elapsed();
                profile.plocked_sets.note(result);
            }

            let started = profile.as_ref().map(|_| Instant::now());
            let result = rt.set_reactive(
                "SEQ",
                &track_step_selected_field(track, step),
                Value::Bool(visible && track == current_track_idx && selected.contains(&step)),
            );
            if let Some(profile) = profile.as_deref_mut() {
                profile.selected_elapsed += started.expect("profile timer").elapsed();
                profile.selected_sets.note(result);
            }

            // Expanded-step param controls use the seqv-slot-param-* projection fields.
            // The legacy seq-track-step-param-* fields are intentionally not synced here;
            // no current Lisp UI binds to them, and writing them dominates scene switches.
        }
    }
    if let Some(profile) = profile.as_deref_mut() {
        profile.elapsed = total_started.expect("profile timer").elapsed();
    }
}

pub(crate) fn build_all_track_duration_spans_value(
    state: &Arc<SequencerState>,
    app: &ui::App,
) -> Value {
    let tracks: Vec<Rc<RefCell<Value>>> = (0..app.tracks.len())
        .map(|track| Rc::new(RefCell::new(build_track_duration_spans_value(state, track))))
        .collect();
    Value::List(tracks)
}

pub(crate) fn build_all_track_playheads_value(state: &Arc<SequencerState>, app: &ui::App) -> Value {
    let items: Vec<Rc<RefCell<Value>>> = (0..app.tracks.len())
        .map(|t| {
            Rc::new(RefCell::new(Value::Number(
                state.transport.track_playheads[t].load(Ordering::Relaxed) as f64,
            )))
        })
        .collect();
    Value::List(items)
}

pub(crate) fn build_all_track_step_has_plocks(state: &Arc<SequencerState>, app: &ui::App) -> Value {
    let tracks: Vec<Rc<RefCell<Value>>> = (0..app.tracks.len())
        .map(|track| {
            Rc::new(RefCell::new(build_step_has_plocks(
                state,
                track,
                &app.graph.effect_descriptors,
            )))
        })
        .collect();
    Value::List(tracks)
}

pub(crate) fn build_all_track_param_lists_value(
    state: &Arc<SequencerState>,
    app: &ui::App,
    param: StepParam,
) -> Value {
    let tracks: Vec<Rc<RefCell<Value>>> = (0..app.tracks.len())
        .map(|track| Rc::new(RefCell::new(build_param_list(state, track, param))))
        .collect();
    Value::List(tracks)
}

pub(crate) fn build_all_active_track_param_lists_value(
    state: &Arc<SequencerState>,
    param: StepParam,
) -> Value {
    let tracks: Vec<Rc<RefCell<Value>>> = (0..state.active_track_count())
        .map(|track| Rc::new(RefCell::new(build_param_list(state, track, param))))
        .collect();
    Value::List(tracks)
}

pub(crate) fn track_playheads_snapshot(state: &Arc<SequencerState>, app: &ui::App) -> Vec<u32> {
    (0..app.tracks.len())
        .map(|t| state.transport.track_playheads[t].load(Ordering::Relaxed))
        .collect()
}

fn track_playhead_row_field(track: usize, row: usize) -> String {
    format!("track-playhead-row-{track}-{row}")
}

pub(crate) fn track_playhead_active_field(track: usize, step: usize) -> String {
    format!("track-playhead-active-{track}-{step}")
}

pub(crate) fn track_playhead_page_field(track: usize) -> String {
    format!("track-playhead-page-{track}")
}

pub(crate) fn track_active_playhead_step(state: &Arc<SequencerState>, track: usize) -> usize {
    let num_steps = state.pattern.track_params[track]
        .get_num_steps()
        .max(1)
        .min(MAX_STEPS);
    let playhead = state.transport.track_playheads[track].load(Ordering::Relaxed) as usize;
    playhead.min(num_steps.saturating_sub(1))
}

pub(crate) fn selected_plock_step(selected_steps: &Arc<Mutex<HashSet<usize>>>) -> Option<usize> {
    selected_steps.lock().unwrap().iter().copied().min()
}

pub(crate) fn displayed_plock_step(
    state: &Arc<SequencerState>,
    track: usize,
    selected_step: Option<usize>,
) -> Option<usize> {
    selected_step.or_else(|| {
        state
            .transport
            .playing
            .load(Ordering::Relaxed)
            .then(|| track_active_playhead_step(state, track))
    })
}

fn track_playhead_row_count(state: &Arc<SequencerState>, track: usize) -> usize {
    let num_steps = state.pattern.track_params[track]
        .get_num_steps()
        .max(1)
        .min(MAX_STEPS);
    (num_steps + PAGE_SIZE - 1) / PAGE_SIZE
}

pub(crate) fn sync_all_track_playhead_fields(
    rt: &mut Runtime,
    state: &Arc<SequencerState>,
    app: &ui::App,
) {
    for track in 0..app.tracks.len() {
        let active_step = track_active_playhead_step(state, track);
        let active_row = active_step / PAGE_SIZE;
        let active_col = active_step % PAGE_SIZE;
        let row_count = track_playhead_row_count(state, track);
        let max_rows = (MAX_STEPS + PAGE_SIZE - 1) / PAGE_SIZE;
        for row in 0..max_rows {
            rt.set_reactive(
                "SEQ",
                &track_playhead_row_field(track, row),
                Value::Number(if row == active_row && row < row_count {
                    active_col as f64
                } else {
                    -1.0
                }),
            );
        }
    }
}

pub(crate) fn clear_all_track_playhead_fields(rt: &mut Runtime, app: &ui::App) {
    let max_rows = (MAX_STEPS + PAGE_SIZE - 1) / PAGE_SIZE;
    for track in 0..app.tracks.len() {
        for row in 0..max_rows {
            rt.set_reactive(
                "SEQ",
                &track_playhead_row_field(track, row),
                Value::Number(-1.0),
            );
        }
    }
}

fn field_safe_name(name: &str) -> String {
    name.chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '_' || ch == '-' {
                ch
            } else {
                '_'
            }
        })
        .collect()
}

pub(crate) fn instrument_param_value_field(track: usize, param_idx: usize, name: &str) -> String {
    format!(
        "track-{track}-instrument-param-{param_idx}-{}",
        field_safe_name(name)
    )
}

pub(crate) fn sampler_selection_time_field(track: usize, marker: &str) -> String {
    format!("track-{track}-sampler-selection-{marker}-time")
}

pub(crate) fn instrument_base_note_value_field(track: usize) -> String {
    format!("track-{track}-instrument-base-note")
}

pub(crate) fn track_effect_param_value_field(
    track: usize,
    slot_idx: usize,
    param_idx: usize,
    name: &str,
) -> String {
    format!(
        "track-{track}-fx-{slot_idx}-param-{param_idx}-{}",
        field_safe_name(name)
    )
}

pub(crate) fn midi_fx_param_value_field(
    track: usize,
    slot_idx: usize,
    param_idx: usize,
    name: &str,
) -> String {
    format!(
        "track-{track}-midi-fx-{slot_idx}-param-{param_idx}-{}",
        field_safe_name(name)
    )
}

pub(crate) fn bus_effect_param_value_field(
    bus_idx: usize,
    slot_idx: usize,
    param_idx: usize,
    name: &str,
) -> String {
    format!(
        "bus-{bus_idx}-fx-{slot_idx}-param-{param_idx}-{}",
        field_safe_name(name)
    )
}

fn insert_string_prop(
    map: &mut HashMap<String, Rc<RefCell<Value>>>,
    key: &str,
    value: impl Into<String>,
) {
    map.insert(
        key.to_string(),
        Rc::new(RefCell::new(Value::String(value.into()))),
    );
}

fn insert_param_ui_metadata(
    map: &mut HashMap<String, Rc<RefCell<Value>>>,
    metadata: Option<&sequencer::effects::ParamUiMetadata>,
) {
    let Some(metadata) = metadata else { return };
    if let Some(group) = &metadata.group {
        insert_string_prop(map, "group", group);
    }
    if let Some(env) = &metadata.env {
        insert_string_prop(map, "env", env);
    }
    if let Some(role) = &metadata.role {
        insert_string_prop(map, "role", role);
    }
}

fn instrument_slot_param_value(
    slot: &sequencer::effects::EffectSlotState,
    desc: &sequencer::effects::EffectDescriptor,
    param_idx: usize,
    plock_step: Option<usize>,
) -> f32 {
    plock_step
        .and_then(|step| slot.plocks.get(step, param_idx))
        .unwrap_or_else(|| {
            if param_idx < slot.num_params.load(Ordering::Relaxed) as usize {
                slot.defaults.get(param_idx)
            } else {
                desc.params
                    .get(param_idx)
                    .map(|param| param.default)
                    .unwrap_or_default()
            }
        })
}

fn selected_voice_mod_source_indices(
    desc: &sequencer::effects::EffectDescriptor,
    slot: &sequencer::effects::EffectSlotState,
    plock_step: Option<usize>,
) -> Vec<usize> {
    sequencer::voice_modulator::selected_source_param_indices(&desc.params, |idx, _| {
        instrument_slot_param_value(slot, desc, idx, plock_step)
    })
}

fn selected_voice_mod_source_indices_for_optional_slot(
    desc: &sequencer::effects::EffectDescriptor,
    slot: Option<&sequencer::effects::EffectSlotState>,
    plock_step: Option<usize>,
) -> Vec<usize> {
    if let Some(slot) = slot {
        return selected_voice_mod_source_indices(desc, slot, plock_step);
    }
    sequencer::voice_modulator::selected_source_param_indices(&desc.params, |_, param| {
        param.default
    })
}

fn param_supports_value_binding(pdesc: &sequencer::effects::ParamDescriptor) -> bool {
    matches!(pdesc.kind, sequencer::effects::ParamKind::Continuous { .. })
        || matches!(pdesc.kind, sequencer::effects::ParamKind::Enum { .. })
        || pdesc.name.eq_ignore_ascii_case("enabled")
}

fn slot_param_stored_value(
    slot: &sequencer::effects::EffectSlotState,
    pdesc: &sequencer::effects::ParamDescriptor,
    param_idx: usize,
    display_step: Option<usize>,
) -> f32 {
    display_step
        .and_then(|step| slot.plocks.get(step, param_idx))
        .unwrap_or_else(|| {
            if param_idx < slot.num_params.load(Ordering::Relaxed) as usize {
                slot.defaults.get(param_idx)
            } else {
                pdesc.default
            }
        })
}

fn reactive_set_needs_ui(result: eseqlisp::runtime::ReactiveSetResult) -> bool {
    result.effects_dirty || result.widgets_dirty
}

pub(crate) fn sync_instrument_param_value_field(
    rt: &mut Runtime,
    app: &ui::App,
    track: usize,
    param_idx: usize,
    display_step: Option<usize>,
) -> bool {
    if let Some((name, value)) = app
        .graph
        .instrument_descriptors
        .get(track)
        .and_then(|desc| desc.params.get(param_idx))
        .and_then(|pdesc| {
            app.state.pattern.instrument_slots.get(track).map(|slot| {
                let stored = slot_param_stored_value(slot, pdesc, param_idx, display_step);
                (pdesc.name.clone(), pdesc.stored_to_user(stored))
            })
        })
    {
        return reactive_set_needs_ui(rt.set_reactive(
            "SEQ",
            &instrument_param_value_field(track, param_idx, &name),
            Value::Number(value as f64),
        ));
    }
    false
}

pub(crate) fn sync_instrument_param_value_field_with_neural_selection(
    rt: &mut Runtime,
    app: &ui::App,
    track: usize,
    param_idx: usize,
    display_step: Option<usize>,
    selected_neural_neurons: Option<
        &std::collections::BTreeSet<sequencer::lisp_host::SelectedNeuralNeuron>,
    >,
) -> bool {
    if let Some((name, value)) = app
        .graph
        .instrument_descriptors
        .get(track)
        .and_then(|desc| desc.params.get(param_idx))
        .and_then(|pdesc| {
            app.state.pattern.instrument_slots.get(track).map(|slot| {
                let stored = selected_neural_neurons
                    .and_then(|selection| {
                        sequencer::lisp_host::selected_neural_instrument_plock_value(
                            &app.state, selection, track, param_idx,
                        )
                    })
                    .unwrap_or_else(|| {
                        slot_param_stored_value(slot, pdesc, param_idx, display_step)
                    });
                (pdesc.name.clone(), pdesc.stored_to_user(stored))
            })
        })
    {
        return reactive_set_needs_ui(rt.set_reactive(
            "SEQ",
            &instrument_param_value_field(track, param_idx, &name),
            Value::Number(value as f64),
        ));
    }
    false
}

pub(crate) fn sync_sampler_selection_time_fields(
    rt: &mut Runtime,
    app: &ui::App,
    track: usize,
    display_step: Option<usize>,
) -> bool {
    if !app.is_sampler_track(track) {
        return false;
    }
    let sample_duration = app
        .sampler_path_for_track(track)
        .as_ref()
        .and_then(|p| eseqlisp::audio::sample::get_registered_sample(&p.display().to_string()))
        .map(|sample| sample.duration_seconds)
        .unwrap_or(1.0);
    let Some(slot) = app.state.pattern.instrument_slots.get(track) else {
        return false;
    };
    let start_raw = display_step
        .and_then(|step| slot.plocks.get(step, 2))
        .unwrap_or_else(|| slot.defaults.get(2));
    let end_raw = display_step
        .and_then(|step| slot.plocks.get(step, 3))
        .unwrap_or_else(|| slot.defaults.get(3));
    let start = start_raw as f64 * sample_duration;
    let end = end_raw as f64 * sample_duration;
    let mut needs_ui = reactive_set_needs_ui(rt.set_reactive(
        "SEQ",
        &sampler_selection_time_field(track, "start"),
        Value::Number(start),
    ));
    needs_ui |= reactive_set_needs_ui(rt.set_reactive(
        "SEQ",
        &sampler_selection_time_field(track, "end"),
        Value::Number(end),
    ));
    needs_ui
}

pub(crate) fn sync_instrument_base_note_value_field(
    rt: &mut Runtime,
    app: &ui::App,
    track: usize,
) -> bool {
    if track < app.tracks.len() {
        let value = f32::from_bits(
            app.state.pattern.instrument_base_note_offsets[track].load(Ordering::Relaxed),
        );
        return reactive_set_needs_ui(rt.set_reactive(
            "SEQ",
            &instrument_base_note_value_field(track),
            Value::Number(value as f64),
        ));
    }
    false
}

pub(crate) fn sync_track_effect_param_value_field(
    rt: &mut Runtime,
    state: &Arc<SequencerState>,
    descriptors: &[Vec<sequencer::effects::EffectDescriptor>],
    track: usize,
    slot_idx: usize,
    param_idx: usize,
    display_step: Option<usize>,
) -> bool {
    if let Some((name, value)) = descriptors
        .get(track)
        .and_then(|slots| slots.get(slot_idx))
        .and_then(|desc| desc.params.get(param_idx).map(|p| (&desc.name, p)))
        .and_then(|(_, pdesc)| {
            state
                .pattern
                .effect_chains
                .get(track)
                .and_then(|chain| chain.get(slot_idx))
                .map(|slot| {
                    let stored = slot_param_stored_value(slot, pdesc, param_idx, display_step);
                    (pdesc.name.clone(), stored)
                })
        })
    {
        return reactive_set_needs_ui(rt.set_reactive(
            "SEQ",
            &track_effect_param_value_field(track, slot_idx, param_idx, &name),
            Value::Number(value as f64),
        ));
    }
    false
}

pub(crate) fn sync_track_effect_param_value_field_with_neural_selection(
    rt: &mut Runtime,
    state: &Arc<SequencerState>,
    descriptors: &[Vec<sequencer::effects::EffectDescriptor>],
    track: usize,
    slot_idx: usize,
    param_idx: usize,
    display_step: Option<usize>,
    selected_neural_neurons: Option<
        &std::collections::BTreeSet<sequencer::lisp_host::SelectedNeuralNeuron>,
    >,
) -> bool {
    if let Some((name, value)) = descriptors
        .get(track)
        .and_then(|slots| slots.get(slot_idx))
        .and_then(|desc| desc.params.get(param_idx).map(|p| (&desc.name, p)))
        .and_then(|(_, pdesc)| {
            state
                .pattern
                .effect_chains
                .get(track)
                .and_then(|chain| chain.get(slot_idx))
                .map(|slot| {
                    let stored = selected_neural_neurons
                        .and_then(|selection| {
                            sequencer::lisp_host::selected_neural_effect_plock_value(
                                state, selection, track, slot_idx, param_idx,
                            )
                        })
                        .unwrap_or_else(|| {
                            slot_param_stored_value(slot, pdesc, param_idx, display_step)
                        });
                    (pdesc.name.clone(), stored)
                })
        })
    {
        return reactive_set_needs_ui(rt.set_reactive(
            "SEQ",
            &track_effect_param_value_field(track, slot_idx, param_idx, &name),
            Value::Number(value as f64),
        ));
    }
    false
}

pub(crate) fn sync_midi_fx_param_value_field(
    rt: &mut Runtime,
    state: &Arc<SequencerState>,
    track: usize,
    slot_idx: usize,
    param_idx: usize,
    display_step: Option<usize>,
) -> bool {
    let chain = state.pattern.track_params[track].midi_fx_chain();
    if let Some((name, value)) = chain
        .get(slot_idx)
        .and_then(|fx_name| sequencer::lisp_host::load_midi_fx_descriptor(fx_name))
        .and_then(|desc| desc.params.get(param_idx).cloned())
        .and_then(|pdesc| {
            state
                .pattern
                .midi_fx_slots
                .get(track)
                .and_then(|slots| slots.get(slot_idx))
                .map(|slot| {
                    let stored = slot_param_stored_value(slot, &pdesc, param_idx, display_step);
                    (pdesc.name, stored)
                })
        })
    {
        return reactive_set_needs_ui(rt.set_reactive(
            "SEQ",
            &midi_fx_param_value_field(track, slot_idx, param_idx, &name),
            Value::Number(value as f64),
        ));
    }
    false
}

pub(crate) fn sync_bus_effect_param_value_field(
    rt: &mut Runtime,
    app: &ui::App,
    bus_idx: usize,
    slot_idx: usize,
    param_idx: usize,
) -> bool {
    if let Some((name, value)) = app.buses.get(bus_idx).and_then(|bus| {
        bus.effect_descriptors
            .get(slot_idx)
            .and_then(|desc| desc.params.get(param_idx))
            .and_then(|pdesc| {
                bus.effect_slots.get(slot_idx).map(|slot| {
                    (
                        pdesc.name.clone(),
                        slot.defaults
                            .get(param_idx)
                            .copied()
                            .unwrap_or(pdesc.default),
                    )
                })
            })
    }) {
        return reactive_set_needs_ui(rt.set_reactive(
            "SEQ",
            &bus_effect_param_value_field(bus_idx, slot_idx, param_idx, &name),
            Value::Number(value as f64),
        ));
    }
    false
}

pub(crate) fn sync_fx_param_binding_fields(
    rt: &mut Runtime,
    app: &ui::App,
    state: &Arc<SequencerState>,
    track: usize,
    selected_steps: &Arc<Mutex<HashSet<usize>>>,
) -> bool {
    sync_fx_param_binding_fields_with_neural_selection(rt, app, state, track, selected_steps, None)
}

pub(crate) fn sync_fx_param_binding_fields_with_neural_selection(
    rt: &mut Runtime,
    app: &ui::App,
    state: &Arc<SequencerState>,
    track: usize,
    selected_steps: &Arc<Mutex<HashSet<usize>>>,
    selected_neural_neurons: Option<
        &std::collections::BTreeSet<sequencer::lisp_host::SelectedNeuralNeuron>,
    >,
) -> bool {
    let mut needs_ui = false;
    if track < app.tracks.len() {
        let selected_step = selected_plock_step(selected_steps);
        let display_step = displayed_plock_step(state, track, selected_step);
        needs_ui |= sync_instrument_base_note_value_field(rt, app, track);
        needs_ui |= sync_sampler_selection_time_fields(rt, app, track, display_step);
        if let Some(desc) = app.graph.instrument_descriptors.get(track) {
            for (param_idx, pdesc) in desc.params.iter().enumerate() {
                if param_supports_value_binding(pdesc) {
                    needs_ui |= sync_instrument_param_value_field_with_neural_selection(
                        rt,
                        app,
                        track,
                        param_idx,
                        display_step,
                        selected_neural_neurons,
                    );
                }
            }
        }
        if let Some(slots) = app.graph.effect_descriptors.get(track) {
            for (slot_idx, desc) in slots.iter().enumerate() {
                for (param_idx, pdesc) in desc.params.iter().enumerate() {
                    if param_supports_value_binding(pdesc) {
                        needs_ui |= sync_track_effect_param_value_field_with_neural_selection(
                            rt,
                            state,
                            &app.graph.effect_descriptors,
                            track,
                            slot_idx,
                            param_idx,
                            display_step,
                            selected_neural_neurons,
                        );
                    }
                }
            }
        }
        for (slot_idx, name) in state.pattern.track_params[track]
            .midi_fx_chain()
            .iter()
            .enumerate()
        {
            if let Some(desc) = sequencer::lisp_host::load_midi_fx_descriptor(name) {
                for (param_idx, pdesc) in desc.params.iter().enumerate() {
                    if param_supports_value_binding(pdesc) {
                        needs_ui |= sync_midi_fx_param_value_field(
                            rt,
                            state,
                            track,
                            slot_idx,
                            param_idx,
                            display_step,
                        );
                    }
                }
            }
        }
    }

    for (bus_idx, bus) in app.buses.iter().enumerate() {
        for (slot_idx, desc) in bus.effect_descriptors.iter().enumerate() {
            for (param_idx, pdesc) in desc.params.iter().enumerate() {
                if param_supports_value_binding(pdesc) {
                    needs_ui |=
                        sync_bus_effect_param_value_field(rt, app, bus_idx, slot_idx, param_idx);
                }
            }
        }
    }
    needs_ui
}

pub(crate) fn sync_track_playhead_field_delta(
    rt: &mut Runtime,
    state: &Arc<SequencerState>,
    app: &ui::App,
    previous: &mut Vec<u32>,
) -> bool {
    let track_count = app.tracks.len();
    let mut current = Vec::with_capacity(track_count);
    let mut effects_dirty = false;
    let mut snapshot_changed = previous.len() != track_count;

    for t in 0..track_count {
        let playhead = state.transport.track_playheads[t].load(Ordering::Relaxed);
        let active_step = track_active_playhead_step(state, t);
        let active_row = active_step / PAGE_SIZE;
        let active_col = active_step % PAGE_SIZE;
        if let Some(prev_playhead) = previous.get(t).copied() {
            if prev_playhead != playhead {
                snapshot_changed = true;
                let num_steps = state.pattern.track_params[t]
                    .get_num_steps()
                    .max(1)
                    .min(MAX_STEPS);
                let prev_active_step = (prev_playhead as usize).min(num_steps.saturating_sub(1));
                let prev_active_row = prev_active_step / PAGE_SIZE;
                if prev_active_row != active_row {
                    effects_dirty |= rt
                        .set_reactive(
                            "SEQ",
                            &track_playhead_page_field(t),
                            Value::Number(active_row as f64),
                        )
                        .effects_dirty;
                    effects_dirty |= rt
                        .set_reactive(
                            "SEQ",
                            &track_playhead_row_field(t, prev_active_row),
                            Value::Number(-1.0),
                        )
                        .effects_dirty;
                }
                if prev_active_step != active_step {
                    effects_dirty |= rt
                        .set_reactive(
                            "SEQ",
                            &track_playhead_active_field(t, prev_active_step),
                            Value::Bool(false),
                        )
                        .effects_dirty;
                    effects_dirty |= rt
                        .set_reactive(
                            "SEQ",
                            &track_playhead_active_field(t, active_step),
                            Value::Bool(true),
                        )
                        .effects_dirty;
                    effects_dirty |= rt
                        .set_reactive(
                            "SEQ",
                            &track_playhead_row_field(t, active_row),
                            Value::Number(active_col as f64),
                        )
                        .effects_dirty;
                }
            }
        } else {
            effects_dirty |= rt
                .set_reactive(
                    "SEQ",
                    &track_playhead_page_field(t),
                    Value::Number(active_row as f64),
                )
                .effects_dirty;
            effects_dirty |= rt
                .set_reactive(
                    "SEQ",
                    &track_playhead_active_field(t, active_step),
                    Value::Bool(true),
                )
                .effects_dirty;
            effects_dirty |= rt
                .set_reactive(
                    "SEQ",
                    &track_playhead_row_field(t, active_row),
                    Value::Number(active_col as f64),
                )
                .effects_dirty;
        }
        current.push(playhead);
    }

    if snapshot_changed {
        *previous = current;
    }

    effects_dirty
}

pub(crate) fn sync_all_track_sequencer_state(
    rt: &mut Runtime,
    state: &Arc<SequencerState>,
    app: &ui::App,
    current_track_idx: usize,
    selected_steps: &Arc<Mutex<HashSet<usize>>>,
) {
    sync_all_track_sequencer_state_inner(rt, state, app, current_track_idx, selected_steps, None);
}

pub(crate) fn sync_all_track_sequencer_state_profiled(
    rt: &mut Runtime,
    state: &Arc<SequencerState>,
    app: &ui::App,
    current_track_idx: usize,
    selected_steps: &Arc<Mutex<HashSet<usize>>>,
) -> AllTrackSequencerSyncProfile {
    let mut profile = AllTrackSequencerSyncProfile::default();
    sync_all_track_sequencer_state_inner(
        rt,
        state,
        app,
        current_track_idx,
        selected_steps,
        Some(&mut profile),
    );
    profile
}

fn sync_all_track_sequencer_state_inner(
    rt: &mut Runtime,
    state: &Arc<SequencerState>,
    app: &ui::App,
    current_track_idx: usize,
    selected_steps: &Arc<Mutex<HashSet<usize>>>,
    mut profile: Option<&mut AllTrackSequencerSyncProfile>,
) {
    let total_started = profile.as_ref().map(|_| Instant::now());
    let started = profile.as_ref().map(|_| Instant::now());
    rt.set_reactive(
        "SEQ",
        "track-steps",
        build_all_track_steps_value(state, app),
    );
    if let Some(profile) = profile.as_deref_mut() {
        profile.track_steps = started.expect("profile timer").elapsed();
    }

    let started = profile.as_ref().map(|_| Instant::now());
    rt.set_reactive(
        "SEQ",
        "track-num-steps",
        build_all_track_num_steps_value(state, app),
    );
    if let Some(profile) = profile.as_deref_mut() {
        profile.track_num_steps = started.expect("profile timer").elapsed();
    }

    let started = profile.as_ref().map(|_| Instant::now());
    rt.set_reactive(
        "SEQ",
        "track-timebases",
        build_all_track_timebase_labels_value(state, app, current_track_idx, selected_steps),
    );
    if let Some(profile) = profile.as_deref_mut() {
        profile.track_timebases = started.expect("profile timer").elapsed();
    }

    let started = profile.as_ref().map(|_| Instant::now());
    rt.set_reactive(
        "SEQ",
        "track-duration-spans",
        build_all_track_duration_spans_value(state, app),
    );
    if let Some(profile) = profile.as_deref_mut() {
        profile.track_duration_spans = started.expect("profile timer").elapsed();
    }

    let started = profile.as_ref().map(|_| Instant::now());
    rt.set_reactive(
        "SEQ",
        "track-step-has-plocks",
        build_all_track_step_has_plocks(state, app),
    );
    if let Some(profile) = profile.as_deref_mut() {
        profile.track_step_has_plocks = started.expect("profile timer").elapsed();
    }

    let started = profile.as_ref().map(|_| Instant::now());
    rt.set_reactive(
        "SEQ",
        "track-playheads",
        build_all_track_playheads_value(state, app),
    );
    if let Some(profile) = profile.as_deref_mut() {
        profile.track_playheads = started.expect("profile timer").elapsed();
    }

    let started = profile.as_ref().map(|_| Instant::now());
    rt.set_reactive(
        "SEQ",
        "track-velocities",
        build_all_track_param_lists_value(state, app, StepParam::Velocity),
    );
    if let Some(profile) = profile.as_deref_mut() {
        profile.track_velocities = started.expect("profile timer").elapsed();
    }

    let started = profile.as_ref().map(|_| Instant::now());
    rt.set_reactive(
        "SEQ",
        "track-durations",
        build_all_track_param_lists_value(state, app, StepParam::Duration),
    );
    if let Some(profile) = profile.as_deref_mut() {
        profile.track_durations = started.expect("profile timer").elapsed();
    }

    let started = profile.as_ref().map(|_| Instant::now());
    rt.set_reactive(
        "SEQ",
        "track-auxas",
        build_all_track_param_lists_value(state, app, StepParam::AuxA),
    );
    if let Some(profile) = profile.as_deref_mut() {
        profile.track_auxas = started.expect("profile timer").elapsed();
    }

    let started = profile.as_ref().map(|_| Instant::now());
    rt.set_reactive(
        "SEQ",
        "track-transposes",
        build_all_track_param_lists_value(state, app, StepParam::Transpose),
    );
    if let Some(profile) = profile.as_deref_mut() {
        profile.track_transposes = started.expect("profile timer").elapsed();
    }

    let started = profile.as_ref().map(|_| Instant::now());
    rt.set_reactive(
        "SEQ",
        "track-pans",
        build_all_track_param_lists_value(state, app, StepParam::Pan),
    );
    if let Some(profile) = profile.as_deref_mut() {
        profile.track_pans = started.expect("profile timer").elapsed();
    }

    let started = profile.as_ref().map(|_| Instant::now());
    rt.set_reactive(
        "SEQ",
        "track-syncs",
        build_all_track_param_lists_value(state, app, StepParam::Sync),
    );
    if let Some(profile) = profile.as_deref_mut() {
        profile.track_syncs = started.expect("profile timer").elapsed();
    }

    let started = profile.as_ref().map(|_| Instant::now());
    rt.set_reactive(
        "SEQ",
        "track-delays",
        build_all_track_param_lists_value(state, app, StepParam::Delay),
    );
    if let Some(profile) = profile.as_deref_mut() {
        profile.track_delays = started.expect("profile timer").elapsed();
    }

    if let Some(profile) = profile.as_deref_mut() {
        profile.step_bindings = sync_all_track_step_binding_fields_profiled(
            rt,
            state,
            app,
            current_track_idx,
            selected_steps,
        );
    } else {
        sync_all_track_step_binding_fields(rt, state, app, current_track_idx, selected_steps);
    }

    let started = profile.as_ref().map(|_| Instant::now());
    sync_all_track_playhead_fields(rt, state, app);
    if let Some(profile) = profile.as_deref_mut() {
        profile.playhead_fields = started.expect("profile timer").elapsed();
        profile.elapsed = total_started.expect("profile timer").elapsed();
    }
}

/// Build a Lisp Value::List of floats for a given step param on a given track.
pub(crate) fn build_param_list(
    state: &Arc<SequencerState>,
    track: usize,
    param: StepParam,
) -> Value {
    let items: Vec<Rc<RefCell<Value>>> = (0..MAX_STEPS)
        .map(|s| {
            let val = state.pattern.step_data[track].get(s, param);
            Rc::new(RefCell::new(Value::Number(val as f64)))
        })
        .collect();
    Value::List(items)
}

pub(crate) fn sync_step_param_lists(rt: &mut Runtime, state: &Arc<SequencerState>, track: usize) {
    rt.set_reactive(
        "SEQ",
        "velocities",
        build_param_list(state, track, StepParam::Velocity),
    );
    rt.set_reactive(
        "SEQ",
        "durations",
        build_param_list(state, track, StepParam::Duration),
    );
    rt.set_reactive(
        "SEQ",
        "transposes",
        build_param_list(state, track, StepParam::Transpose),
    );
    rt.set_reactive(
        "SEQ",
        "auxas",
        build_param_list(state, track, StepParam::AuxA),
    );
    rt.set_reactive(
        "SEQ",
        "pans",
        build_param_list(state, track, StepParam::Pan),
    );
    rt.set_reactive(
        "SEQ",
        "syncs",
        build_param_list(state, track, StepParam::Sync),
    );
    rt.set_reactive(
        "SEQ",
        "delays",
        build_param_list(state, track, StepParam::Delay),
    );
    rt.set_reactive(
        "SEQ",
        "track-velocities",
        build_all_active_track_param_lists_value(state, StepParam::Velocity),
    );
    rt.set_reactive(
        "SEQ",
        "track-durations",
        build_all_active_track_param_lists_value(state, StepParam::Duration),
    );
    rt.set_reactive(
        "SEQ",
        "track-transposes",
        build_all_active_track_param_lists_value(state, StepParam::Transpose),
    );
    rt.set_reactive(
        "SEQ",
        "track-auxas",
        build_all_active_track_param_lists_value(state, StepParam::AuxA),
    );
    rt.set_reactive(
        "SEQ",
        "track-pans",
        build_all_active_track_param_lists_value(state, StepParam::Pan),
    );
    rt.set_reactive(
        "SEQ",
        "track-syncs",
        build_all_active_track_param_lists_value(state, StepParam::Sync),
    );
    rt.set_reactive(
        "SEQ",
        "track-delays",
        build_all_active_track_param_lists_value(state, StepParam::Delay),
    );
}

pub(crate) fn build_accumulator_names(app: &ui::App) -> Vec<String> {
    let mut names = BUILTIN_ACCUMULATOR_NAMES
        .iter()
        .map(|name| (*name).to_string())
        .collect::<Vec<_>>();
    if let Some(runtime) = app.editor.scratch_runtime.as_ref() {
        names.extend(runtime.accumulator_names());
    }
    names
}

pub(crate) fn build_accumulator_options(app: &ui::App) -> Value {
    let items = build_accumulator_names(app)
        .into_iter()
        .map(|name| Rc::new(RefCell::new(Value::String(name))))
        .collect();
    Value::List(items)
}

pub(crate) fn build_accum_mode_options() -> Value {
    let items = ACCUM_MODE_LABELS
        .iter()
        .map(|label| Rc::new(RefCell::new(Value::String((*label).to_string()))))
        .collect();
    Value::List(items)
}

pub(crate) fn build_fts_options() -> Value {
    let items = FTS_SCALE_NAMES
        .iter()
        .map(|scale| Rc::new(RefCell::new(Value::String((*scale).to_string()))))
        .collect();
    Value::List(items)
}

pub(crate) fn builtin_accumulator_default_limit(idx: usize) -> f32 {
    match idx {
        1 => 48.0,
        2 => 1.0,
        _ => 0.0,
    }
}

pub(crate) fn accum_mode_label(mode: u32) -> &'static str {
    ACCUM_MODE_LABELS
        .get(mode as usize)
        .copied()
        .unwrap_or(ACCUM_MODE_LABELS[0])
}

pub(crate) fn selected_accumulator_name(app: &ui::App, track: usize) -> String {
    let tp = &app.state.pattern.track_params[track];
    if let Some(name) = tp.script_accumulator_name() {
        return name;
    }
    build_accumulator_names(app)
        .get(tp.get_accumulator_idx())
        .cloned()
        .unwrap_or_else(|| "Off".to_string())
}

/// Build a Lisp Value::List of bools for record-armed state per track.
pub(crate) fn build_record_armed_value(armed: &[bool]) -> Value {
    let items: Vec<Rc<RefCell<Value>>> = armed
        .iter()
        .map(|a| Rc::new(RefCell::new(Value::Bool(*a))))
        .collect();
    Value::List(items)
}

/// Build a Lisp Value::List of track name strings.
pub(crate) fn build_track_names(names: &[String]) -> Value {
    let items: Vec<Rc<RefCell<Value>>> = names
        .iter()
        .map(|name| Rc::new(RefCell::new(Value::String(name.clone()))))
        .collect();
    Value::List(items)
}

pub(crate) fn build_track_colors(app: &ui::App) -> Value {
    let items: Vec<Rc<RefCell<Value>>> = (0..app.tracks.len())
        .map(|track| {
            let color = app
                .track_colors
                .get(track)
                .copied()
                .unwrap_or_else(|| sequencer::track_color::TrackColor::palette_color(track))
                .clamped();
            Rc::new(RefCell::new(Value::List(vec![
                Rc::new(RefCell::new(Value::Number(color.r as f64))),
                Rc::new(RefCell::new(Value::Number(color.g as f64))),
                Rc::new(RefCell::new(Value::Number(color.b as f64))),
            ])))
        })
        .collect();
    Value::List(items)
}

pub(crate) fn build_track_collapsed_from_slice(collapsed: &[bool], track_count: usize) -> Value {
    Value::List(
        (0..track_count)
            .map(|track| {
                Rc::new(RefCell::new(Value::Bool(
                    collapsed.get(track).copied().unwrap_or(false),
                )))
            })
            .collect(),
    )
}

pub(crate) fn build_track_collapsed(app: &ui::App) -> Value {
    build_track_collapsed_from_slice(&app.track_collapsed, app.tracks.len())
}

pub(crate) fn build_track_ids(app: &ui::App) -> Value {
    let items: Vec<Rc<RefCell<Value>>> = app
        .graph
        .track_node_ids
        .iter()
        .map(|ids| Rc::new(RefCell::new(Value::Number(ids.pan_id as f64))))
        .collect();
    Value::List(items)
}

pub(crate) fn build_track_instrument_types(app: &ui::App) -> Value {
    let items: Vec<Rc<RefCell<Value>>> = app
        .graph
        .track_instrument_types
        .iter()
        .map(|instrument_type| {
            let label = match instrument_type {
                sequencer::sequencer::InstrumentType::Sampler => "sampler",
                sequencer::sequencer::InstrumentType::Custom => "custom",
                sequencer::sequencer::InstrumentType::Modulator => "modulator",
            };
            Rc::new(RefCell::new(Value::String(label.to_string())))
        })
        .collect();
    Value::List(items)
}

pub(crate) fn build_track_mod_output_available(app: &ui::App) -> Value {
    let items: Vec<Rc<RefCell<Value>>> = (0..app.graph.track_instrument_types.len())
        .map(|track| {
            Rc::new(RefCell::new(Value::Bool(
                app.graph.track_exposes_mod_output(track),
            )))
        })
        .collect();
    Value::List(items)
}

pub(crate) fn build_track_instrument_run_modes(app: &ui::App) -> Value {
    let items: Vec<Rc<RefCell<Value>>> = app
        .graph
        .track_instrument_run_modes
        .iter()
        .map(|run_mode| {
            let label = match run_mode {
                sequencer::sequencer::CustomInstrumentRunMode::Instrument => "instrument",
                sequencer::sequencer::CustomInstrumentRunMode::FreePatch => "free_patch",
            };
            Rc::new(RefCell::new(Value::String(label.to_string())))
        })
        .collect();
    Value::List(items)
}

pub(crate) fn sync_track_name_state(
    rt: &mut Runtime,
    track_names: &mut Vec<String>,
    app: &ui::App,
) {
    rt.set_reactive("SEQ", "track-ids", build_track_ids(app));
    rt.set_reactive(
        "SEQ",
        "track-instrument-types",
        build_track_instrument_types(app),
    );
    rt.set_reactive(
        "SEQ",
        "track-mod-output-available",
        build_track_mod_output_available(app),
    );
    rt.set_reactive(
        "SEQ",
        "track-instrument-run-modes",
        build_track_instrument_run_modes(app),
    );
    if *track_names == app.tracks {
        return;
    }
    *track_names = app.tracks.clone();
    rt.set_reactive("SEQ", "num-tracks", Value::Number(track_names.len() as f64));
    rt.set_reactive("SEQ", "track-names", build_track_names(track_names));
    rt.set_reactive("SEQ", "track-colors", build_track_colors(app));
    rt.set_reactive("SEQ", "track-collapsed", build_track_collapsed(app));
}

/// Build a Lisp Value::List of per-track volumes (0.0–1.0).
pub(crate) fn build_track_volumes(state: &Arc<SequencerState>) -> Value {
    let count = state.active_track_count();
    let items: Vec<Rc<RefCell<Value>>> = (0..count)
        .map(|t| {
            let vol = state.pattern.track_params[t].get_volume();
            Rc::new(RefCell::new(Value::Number(vol as f64)))
        })
        .collect();
    Value::List(items)
}

pub(crate) fn build_track_pans(state: &Arc<SequencerState>) -> Value {
    let count = state.active_track_count();
    let items: Vec<Rc<RefCell<Value>>> = (0..count)
        .map(|t| {
            Rc::new(RefCell::new(Value::Number(
                state.pattern.track_params[t].get_pan() as f64,
            )))
        })
        .collect();
    Value::List(items)
}

pub(crate) fn track_volume_field(track: usize) -> String {
    format!("track-{track}-volume")
}

pub(crate) fn track_pan_field(track: usize) -> String {
    format!("track-{track}-pan")
}

pub(crate) fn sync_track_volume_binding_field(
    rt: &mut Runtime,
    state: &Arc<SequencerState>,
    track: usize,
) {
    if let Some(tp) = state.pattern.track_params.get(track) {
        rt.set_reactive(
            "SEQ",
            &track_volume_field(track),
            Value::Number(tp.get_volume() as f64),
        );
    }
}

pub(crate) fn sync_track_pan_binding_field(
    rt: &mut Runtime,
    state: &Arc<SequencerState>,
    track: usize,
) {
    if let Some(tp) = state.pattern.track_params.get(track) {
        rt.set_reactive(
            "SEQ",
            &track_pan_field(track),
            Value::Number(tp.get_pan() as f64),
        );
    }
}

pub(crate) fn sync_track_volume_pan_binding_fields(rt: &mut Runtime, state: &Arc<SequencerState>) {
    for track in 0..state.active_track_count() {
        sync_track_volume_binding_field(rt, state, track);
        sync_track_pan_binding_field(rt, state, track);
    }
}

pub(crate) fn build_track_outputs(app: &ui::App, state: &Arc<SequencerState>) -> Value {
    let count = state.active_track_count();
    let items: Vec<Rc<RefCell<Value>>> = (0..count)
        .map(|t| {
            Rc::new(RefCell::new(build_track_output_label(
                app,
                &state.pattern.track_params[t],
            )))
        })
        .collect();
    Value::List(items)
}

pub(crate) fn build_all_track_bus_sends(app: &ui::App, state: &Arc<SequencerState>) -> Value {
    let count = state.active_track_count();
    let items: Vec<Rc<RefCell<Value>>> = (0..count)
        .map(|t| {
            Rc::new(RefCell::new(build_track_bus_sends(
                app,
                &state.pattern.track_params[t],
            )))
        })
        .collect();
    Value::List(items)
}

pub(crate) fn track_bus_send_field(track: usize, bus_idx: usize) -> String {
    format!("track-{track}-bus-{bus_idx}-send")
}

pub(crate) fn current_track_bus_send_field(bus_idx: usize) -> String {
    format!("tp-bus-{bus_idx}-send")
}

pub(crate) fn track_bus_send_amount(
    app: &ui::App,
    state: &Arc<SequencerState>,
    track: usize,
    bus_idx: usize,
) -> Option<f32> {
    let bus = app.buses.get(bus_idx)?;
    if bus.id == sequencer::sequencer::BusId::MIX {
        return None;
    }
    let tp = state.pattern.track_params.get(track)?;
    Some(
        tp.sends()
            .iter()
            .find(|send| send.destination == bus.id)
            .map(|send| send.amount)
            .unwrap_or(0.0),
    )
}

pub(crate) fn sync_track_bus_send_binding_field(
    rt: &mut Runtime,
    app: &ui::App,
    state: &Arc<SequencerState>,
    track: usize,
    bus_idx: usize,
) {
    if let Some(amount) = track_bus_send_amount(app, state, track, bus_idx) {
        rt.set_reactive(
            "SEQ",
            &track_bus_send_field(track, bus_idx),
            Value::Number(amount as f64),
        );
    }
}

pub(crate) fn sync_track_bus_send_binding_fields(
    rt: &mut Runtime,
    app: &ui::App,
    state: &Arc<SequencerState>,
) {
    for track in 0..state.active_track_count() {
        for (bus_idx, bus) in app.buses.iter().enumerate() {
            if bus.id != sequencer::sequencer::BusId::MIX {
                sync_track_bus_send_binding_field(rt, app, state, track, bus_idx);
            }
        }
    }
}

pub(crate) fn sync_current_track_bus_send_binding_field(
    rt: &mut Runtime,
    app: &ui::App,
    state: &Arc<SequencerState>,
    track: usize,
    bus_idx: usize,
) {
    if let Some(amount) = track_bus_send_amount(app, state, track, bus_idx) {
        rt.set_reactive(
            "SEQ",
            &current_track_bus_send_field(bus_idx),
            Value::Number(amount as f64),
        );
    }
}

pub(crate) fn sync_current_track_bus_send_binding_fields(
    rt: &mut Runtime,
    app: &ui::App,
    state: &Arc<SequencerState>,
    track: usize,
) {
    for (bus_idx, bus) in app.buses.iter().enumerate() {
        if bus.id != sequencer::sequencer::BusId::MIX {
            sync_current_track_bus_send_binding_field(rt, app, state, track, bus_idx);
        }
    }
}

pub(crate) fn build_mod_routes(state: &Arc<SequencerState>) -> Value {
    let routes = state.current_mod_connections();
    Value::List(
        routes
            .into_iter()
            .map(|connection| {
                let mut map = std::collections::HashMap::new();
                map.insert(
                    "source".to_string(),
                    Rc::new(RefCell::new(Value::Number(connection.source_track as f64))),
                );
                map.insert(
                    "dest".to_string(),
                    Rc::new(RefCell::new(Value::Number(connection.dest_track as f64))),
                );
                map.insert(
                    "input".to_string(),
                    Rc::new(RefCell::new(Value::Number(connection.dest_input as f64))),
                );
                Rc::new(RefCell::new(Value::Map(map)))
            })
            .collect(),
    )
}

pub(crate) fn build_track_mutes(state: &Arc<SequencerState>) -> Value {
    let count = state.active_track_count();
    let items: Vec<Rc<RefCell<Value>>> = (0..count)
        .map(|t| {
            Rc::new(RefCell::new(Value::Bool(
                state.pattern.track_params[t].is_muted(),
            )))
        })
        .collect();
    Value::List(items)
}

pub(crate) fn build_track_solos(state: &Arc<SequencerState>) -> Value {
    let count = state.active_track_count();
    let items: Vec<Rc<RefCell<Value>>> = (0..count)
        .map(|t| {
            Rc::new(RefCell::new(Value::Bool(
                state.pattern.track_params[t].is_solo(),
            )))
        })
        .collect();
    Value::List(items)
}

pub(crate) fn build_track_muted_by_solo(state: &Arc<SequencerState>) -> Value {
    let count = state.active_track_count();
    let has_solo = (0..count).any(|t| state.pattern.track_params[t].is_solo());
    let items: Vec<Rc<RefCell<Value>>> = (0..count)
        .map(|t| {
            Rc::new(RefCell::new(Value::Bool(
                has_solo && !state.pattern.track_params[t].is_solo(),
            )))
        })
        .collect();
    Value::List(items)
}

pub(crate) fn sync_track_mixer_state(rt: &mut Runtime, app: &ui::App, state: &Arc<SequencerState>) {
    rt.set_reactive("SEQ", "track-colors", build_track_colors(app));
    rt.set_reactive("SEQ", "track-collapsed", build_track_collapsed(app));
    rt.set_reactive(
        "SEQ",
        "track-pattern-cells",
        build_track_pattern_cells_value(state, app.tracks.len()),
    );
    sync_track_pattern_cell_state_fields(rt, state, app.tracks.len());
    rt.set_reactive(
        "SEQ",
        "track-instrument-types",
        build_track_instrument_types(app),
    );
    rt.set_reactive(
        "SEQ",
        "track-mod-output-available",
        build_track_mod_output_available(app),
    );
    rt.set_reactive(
        "SEQ",
        "track-instrument-run-modes",
        build_track_instrument_run_modes(app),
    );
    rt.set_reactive("SEQ", "track-volumes", build_track_volumes(state));
    rt.set_reactive("SEQ", "track-mixer-pans", build_track_pans(state));
    sync_track_volume_pan_binding_fields(rt, state);
    rt.set_reactive("SEQ", "track-outputs", build_track_outputs(app, state));
    rt.set_reactive(
        "SEQ",
        "track-bus-sends",
        build_all_track_bus_sends(app, state),
    );
    sync_track_bus_send_binding_fields(rt, app, state);
    rt.set_reactive("SEQ", "mod-routes", build_mod_routes(state));
    rt.set_reactive("SEQ", "track-mutes", build_track_mutes(state));
    rt.set_reactive("SEQ", "track-solos", build_track_solos(state));
    rt.set_reactive(
        "SEQ",
        "track-muted-by-solo",
        build_track_muted_by_solo(state),
    );
}

pub(crate) fn sync_bus_mixer_control_state(rt: &mut Runtime, app: &ui::App) {
    let names: Vec<String> = app.buses.iter().map(|bus| bus.name.clone()).collect();
    let volumes: Vec<Rc<RefCell<Value>>> = app
        .buses
        .iter()
        .map(|bus| Rc::new(RefCell::new(Value::Number(bus.volume as f64))))
        .collect();
    let mutes: Vec<Rc<RefCell<Value>>> = app
        .buses
        .iter()
        .map(|bus| Rc::new(RefCell::new(Value::Bool(bus.mute))))
        .collect();
    let solos: Vec<Rc<RefCell<Value>>> = app
        .buses
        .iter()
        .map(|bus| Rc::new(RefCell::new(Value::Bool(bus.solo))))
        .collect();
    rt.set_reactive("SEQ", "bus-names", build_track_names(&names));
    rt.set_reactive("SEQ", "bus-volumes", Value::List(volumes));
    rt.set_reactive("SEQ", "bus-mutes", Value::List(mutes));
    rt.set_reactive("SEQ", "bus-solos", Value::List(solos));
}

pub(crate) fn sync_bus_mixer_state(rt: &mut Runtime, app: &ui::App) {
    sync_bus_mixer_control_state(rt, app);
    rt.set_reactive("SEQ", "bus-effects", build_bus_effects_value(app));
    rt.set_reactive("SEQ", "bus-steps", build_bus_steps_value(app));
    rt.set_reactive(
        "SEQ",
        "bus-velocities",
        build_bus_param_lists(app, "velocity"),
    );
    rt.set_reactive(
        "SEQ",
        "bus-durations",
        build_bus_param_lists(app, "duration"),
    );
    rt.set_reactive("SEQ", "bus-syncs", build_bus_param_lists(app, "sync"));
    rt.set_reactive("SEQ", "bus-num-steps", build_bus_num_steps_value(app));
    rt.set_reactive("SEQ", "bus-timebases", build_bus_timebase_value(app));
    rt.set_reactive("SEQ", "bus-swings", build_bus_swing_value(app));
    rt.set_reactive(
        "SEQ",
        "bus-swing-resolutions",
        build_bus_swing_resolution_value(app),
    );
    rt.set_reactive("SEQ", "bus-step-has-plocks", build_bus_step_has_plocks(app));
    rt.set_reactive("SEQ", "bus-playheads", build_bus_playheads_value(app));
}

pub(crate) fn sync_track_mixer_empty_state(rt: &mut Runtime) {
    rt.set_reactive("SEQ", "track-volumes", Value::List(vec![]));
    rt.set_reactive("SEQ", "track-mixer-pans", Value::List(vec![]));
    rt.set_reactive("SEQ", "track-outputs", Value::List(vec![]));
    rt.set_reactive("SEQ", "track-colors", Value::List(vec![]));
    rt.set_reactive("SEQ", "track-collapsed", Value::List(vec![]));
    rt.set_reactive("SEQ", "track-pattern-cells", Value::List(vec![]));
    rt.set_reactive("SEQ", "track-instrument-types", Value::List(vec![]));
    rt.set_reactive("SEQ", "track-mod-output-available", Value::List(vec![]));
    rt.set_reactive("SEQ", "track-bus-sends", Value::List(vec![]));
    rt.set_reactive("SEQ", "track-mutes", Value::List(vec![]));
    rt.set_reactive("SEQ", "track-solos", Value::List(vec![]));
    rt.set_reactive("SEQ", "track-muted-by-solo", Value::List(vec![]));
    rt.set_reactive("SEQ", "bus-names", Value::List(vec![]));
    rt.set_reactive("SEQ", "bus-volumes", Value::List(vec![]));
    rt.set_reactive("SEQ", "bus-mutes", Value::List(vec![]));
    rt.set_reactive("SEQ", "bus-solos", Value::List(vec![]));
    rt.set_reactive("SEQ", "bus-steps", Value::List(vec![]));
    rt.set_reactive("SEQ", "bus-velocities", Value::List(vec![]));
    rt.set_reactive("SEQ", "bus-durations", Value::List(vec![]));
    rt.set_reactive("SEQ", "bus-syncs", Value::List(vec![]));
    rt.set_reactive("SEQ", "bus-num-steps", Value::List(vec![]));
    rt.set_reactive("SEQ", "bus-timebases", Value::List(vec![]));
    rt.set_reactive("SEQ", "bus-swings", Value::List(vec![]));
    rt.set_reactive("SEQ", "bus-swing-resolutions", Value::List(vec![]));
    rt.set_reactive("SEQ", "bus-step-has-plocks", Value::List(vec![]));
    rt.set_reactive("SEQ", "bus-playheads", Value::List(vec![]));
}

pub(crate) fn build_bus_playheads_value(app: &ui::App) -> Value {
    Value::List(
        bus_playhead_snapshot(app)
            .into_iter()
            .map(|step| Rc::new(RefCell::new(Value::Number(step as f64))))
            .collect(),
    )
}

pub(crate) fn bus_playhead_snapshot(app: &ui::App) -> Vec<usize> {
    let playheads = app.graph.bus_gate_playheads.lock().unwrap();
    app.buses
        .iter()
        .map(|bus| {
            playheads
                .iter()
                .find(|(id, _)| *id == bus.id)
                .map(|(_, step)| *step)
                .unwrap_or(0)
        })
        .collect()
}

pub(crate) fn build_bus_steps_value(app: &ui::App) -> Value {
    Value::List(
        app.buses
            .iter()
            .map(|bus| {
                Rc::new(RefCell::new(Value::List(
                    bus.gate_sequence
                        .steps
                        .iter()
                        .map(|active| Rc::new(RefCell::new(Value::Bool(*active))))
                        .collect(),
                )))
            })
            .collect(),
    )
}

pub(crate) fn build_bus_param_lists(app: &ui::App, param: &str) -> Value {
    Value::List(
        app.buses
            .iter()
            .map(|bus| {
                let values = match param {
                    "duration" => &bus.gate_sequence.durations,
                    "sync" => &bus.gate_sequence.syncs,
                    _ => &bus.gate_sequence.velocities,
                };
                Rc::new(RefCell::new(Value::List(
                    values
                        .iter()
                        .map(|value| Rc::new(RefCell::new(Value::Number(*value as f64))))
                        .collect(),
                )))
            })
            .collect(),
    )
}

pub(crate) fn build_bus_num_steps_value(app: &ui::App) -> Value {
    Value::List(
        app.buses
            .iter()
            .map(|bus| {
                Rc::new(RefCell::new(Value::Number(
                    bus.gate_sequence.num_steps as f64,
                )))
            })
            .collect(),
    )
}

pub(crate) fn build_bus_timebase_value(app: &ui::App) -> Value {
    Value::List(
        app.buses
            .iter()
            .map(|bus| {
                Rc::new(RefCell::new(Value::String(
                    bus.gate_sequence.timebase.label().to_string(),
                )))
            })
            .collect(),
    )
}

pub(crate) fn build_bus_swing_value(app: &ui::App) -> Value {
    Value::List(
        app.buses
            .iter()
            .map(|bus| Rc::new(RefCell::new(Value::Number(bus.gate_sequence.swing as f64))))
            .collect(),
    )
}

pub(crate) fn build_bus_swing_resolution_value(app: &ui::App) -> Value {
    Value::List(
        app.buses
            .iter()
            .map(|bus| {
                Rc::new(RefCell::new(Value::String(
                    bus.gate_sequence.swing_resolution.label().to_string(),
                )))
            })
            .collect(),
    )
}

pub(crate) fn build_bus_step_has_plocks(app: &ui::App) -> Value {
    Value::List(
        app.buses
            .iter()
            .map(|bus| {
                Rc::new(RefCell::new(Value::List(
                    (0..MAX_STEPS)
                        .map(|step| {
                            let effect_has_plock = bus.effect_slots.iter().any(|slot| {
                                slot.plocks
                                    .get(step)
                                    .map(|params| params.iter().any(Option::is_some))
                                    .unwrap_or(false)
                            });
                            Rc::new(RefCell::new(Value::Bool(
                                bus.gate_sequence.has_step_plock(step) || effect_has_plock,
                            )))
                        })
                        .collect(),
                )))
            })
            .collect(),
    )
}

pub(crate) fn push_panner_bool(
    lg_raw: *mut sequencer::audiograph::LiveGraph,
    pan_id: i32,
    param_idx: u64,
    value: bool,
) {
    if pan_id < 0 {
        return;
    }
    unsafe {
        sequencer::audiograph::params_push_wrapper(
            lg_raw,
            sequencer::audiograph::ParamMsg {
                idx: param_idx,
                logical_id: pan_id as u64,
                fvalue: if value { 1.0 } else { 0.0 },
            },
        );
    }
}

pub(crate) fn push_solo_mutes(
    lg_raw: *mut sequencer::audiograph::LiveGraph,
    state: &Arc<SequencerState>,
    pan_ids: &[i32],
) {
    let count = state.active_track_count();
    let has_solo = (0..count).any(|track| state.pattern.track_params[track].is_solo());
    for track in 0..count {
        let muted_by_solo = has_solo && !state.pattern.track_params[track].is_solo();
        if let Some(&pan_id) = pan_ids.get(track) {
            push_panner_bool(
                lg_raw,
                pan_id,
                sequencer::stereo_panner::STEREO_PANNER_PARAM_MUTED_BY_SOLO,
                muted_by_solo,
            );
        }
    }
}

fn read_panner_peak_levels(lg: sequencer::audiograph::LiveGraphPtr, node_ids: &[i32]) -> Vec<f64> {
    node_ids
        .iter()
        .map(|&node_id| read_panner_peak_level(lg, node_id))
        .collect()
}

fn read_panner_peak_level(lg: sequencer::audiograph::LiveGraphPtr, node_id: i32) -> f64 {
    const PANNER_STATE_LEN: usize = sequencer::stereo_panner::STEREO_PANNER_STATE_SIZE;
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
    let peak = state[sequencer::stereo_panner::STATE_PEAK_L]
        .max(state[sequencer::stereo_panner::STATE_PEAK_R]);
    meter_display_level(peak)
}

pub(crate) fn read_track_peak_levels(
    lg: sequencer::audiograph::LiveGraphPtr,
    pan_ids: &[i32],
) -> Vec<f64> {
    read_panner_peak_levels(lg, pan_ids)
}

pub(crate) fn read_bus_peak_levels(
    lg: sequencer::audiograph::LiveGraphPtr,
    bus_nodes: &[ui::BusNodeIds],
) -> Vec<f64> {
    bus_nodes
        .iter()
        .map(|bus| {
            read_panner_peak_level(lg, bus.merge_id)
                .max(read_panner_peak_level(lg, bus.gate_id))
                .max(read_panner_peak_level(lg, bus.volume_id))
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
    app: &ui::App,
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

fn read_modulator_display_value(
    lg: sequencer::audiograph::LiveGraphPtr,
    node_id: i32,
) -> (f64, f64) {
    const STATE_LEN: usize = sequencer::track_modulator::MODULATOR_ENVELOPE_STATE_SIZE;
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
    (
        quantize_modulator_unit_value(state[sequencer::track_modulator::STATE_DISPLAY_PHASE]),
        quantize_modulator_unit_value(
            state[sequencer::track_modulator::PARAM_PULSE_LEVEL as usize],
        ),
    )
}

fn quantize_modulator_unit_value(value: f32) -> f64 {
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

pub(crate) fn sync_neural_visualization_fields(
    rt: &mut Runtime,
    state: &Arc<SequencerState>,
) -> bool {
    let mut effects_dirty = rt
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
    effects_dirty |= rt
        .set_reactive(
            "SEQ",
            "graph-visualizations",
            build_graph_visualizations_value(state),
        )
        .effects_dirty;
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

pub(crate) fn sync_track_topology_state(
    rt: &mut Runtime,
    app: &ui::App,
    state: &Arc<SequencerState>,
    track_names: &mut Vec<String>,
    current_track_idx: usize,
    selected_steps: &Arc<Mutex<HashSet<usize>>>,
    piano_roll_selection: &Arc<Mutex<HashSet<u64>>>,
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
        rt.set_reactive("SEQ", "track-steps", Value::List(vec![]));
        rt.set_reactive("SEQ", "track-num-steps", Value::List(vec![]));
        rt.set_reactive("SEQ", "track-timebases", Value::List(vec![]));
        rt.set_reactive("SEQ", "track-duration-spans", Value::List(vec![]));
        rt.set_reactive("SEQ", "track-playheads", Value::List(vec![]));
        rt.set_reactive("SEQ", "track-step-has-plocks", Value::List(vec![]));
        rt.set_reactive("SEQ", "track-velocities", Value::List(vec![]));
        rt.set_reactive("SEQ", "track-durations", Value::List(vec![]));
        rt.set_reactive("SEQ", "track-auxas", Value::List(vec![]));
        rt.set_reactive("SEQ", "track-transposes", Value::List(vec![]));
        rt.set_reactive("SEQ", "track-pans", Value::List(vec![]));
        rt.set_reactive("SEQ", "track-syncs", Value::List(vec![]));
        rt.set_reactive("SEQ", "track-delays", Value::List(vec![]));
        rt.set_reactive("SEQ", "track-ids", Value::List(vec![]));
        return;
    }

    sync_all_track_sequencer_state(rt, state, app, current_track_idx, selected_steps);

    sync_playhead_fields(
        rt,
        state.transport.track_playheads[current_track_idx].load(Ordering::Relaxed) as usize,
        state.pattern.track_params[current_track_idx].get_num_steps(),
    );
    rt.set_reactive("SEQ", "steps", build_steps_value(state, current_track_idx));
    sync_piano_roll_state(rt, state, current_track_idx, piano_roll_selection);
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
    sync_fx_param_binding_fields(rt, app, state, current_track_idx, selected_steps);
    *accumulator_names.lock().unwrap() = build_accumulator_names(app);
    sync_track_params(rt, app, state, current_track_idx, selected_steps);
    rt.set_reactive(
        "SEQ",
        "step-has-plocks",
        build_step_has_plocks(state, current_track_idx, &app.graph.effect_descriptors),
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

fn graph_visualization_value(snapshot: &sequencer::graph::GraphVisualizationSnapshot) -> Value {
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
    Value::Map(map)
}

fn value_cell(value: Value) -> Rc<RefCell<Value>> {
    Rc::new(RefCell::new(value))
}

fn graph_dense_edge_matrix_value(
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

fn graph_edge_value(edge: sequencer::graph::GraphVisualizationEdge) -> Value {
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

fn graph_optional_event_value(event: Option<sequencer::graph::GraphVisualizationEvent>) -> Value {
    event.map(graph_event_value).unwrap_or(Value::Nil)
}

fn graph_event_value(event: sequencer::graph::GraphVisualizationEvent) -> Value {
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

fn neural_column_matrix_value(values: impl Iterator<Item = f64>) -> Value {
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

fn neural_energy_display_value(value: f32) -> f64 {
    let value = value.clamp(0.0, 4.0) as f64;
    (value * 100.0).round() / 100.0
}

fn graph_energy_display_value(value: f64) -> f64 {
    let value = value.clamp(0.0, 4.0);
    (value * 100.0).round() / 100.0
}

fn graph_weight_display_value(value: f64) -> f64 {
    (value * 100.0).round() / 100.0
}

fn neural_trigger_display_value(value: f32) -> f64 {
    value.clamp(0.0, 1.0) as f64
}

fn neural_dampening_display_value(value: f32) -> f64 {
    let value = value.clamp(0.0, 1.0) as f64;
    (value * 100.0).round() / 100.0
}

fn neural_network_value(network: &sequencer::neural::ProjectNeuralNetwork) -> Value {
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

fn neural_neuron_value(idx: usize, neuron: &sequencer::neural::ProjectNeuron) -> Value {
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

/// Build a Lisp Value::List of effect slot maps for a track.
/// Each slot is a map: {:name "Filter" :params ({:name "cutoff" :value 1000 :min 20 :max 20000} ...)}
pub(crate) fn build_effects_value(
    state: &Arc<SequencerState>,
    track: usize,
    descriptors: &[Vec<sequencer::effects::EffectDescriptor>],
    selected: &Arc<Mutex<HashSet<usize>>>,
) -> Value {
    use sequencer::effects::{ParamKind, SyncDivision};
    use std::collections::HashMap;
    let Some(track_descs) = descriptors.get(track) else {
        return Value::List(vec![]);
    };
    let chain = &state.pattern.effect_chains[track];
    let sel = selected.lock().unwrap();
    // If steps are selected, show p-lock value from first selected step
    let plock_step = sel.iter().copied().min();

    struct UiModMetadata {
        source_param_idx: Option<usize>,
        depth_param_idx: usize,
        source_slot: f32,
        source_value_field: Option<String>,
        depth_value: f32,
        depth_value_field: Option<String>,
        depth_min: f32,
        depth_max: f32,
        depth_unit: Option<String>,
    }

    fn is_mod_param(name: &str) -> bool {
        name.starts_with("mod ")
    }

    fn is_generated_host_mod_param(name: &str) -> bool {
        name.starts_with("__host_mod__")
    }

    fn is_hidden_dgen_mod_param(name: &str) -> bool {
        name.starts_with("__dgen_mod_active__")
    }

    fn is_source_param(node_param_idx: u32) -> bool {
        sequencer::voice_modulator::is_source_param(node_param_idx)
    }

    fn rename_source_param(name: &str) -> String {
        sequencer::voice_modulator::source_param_display_name(name)
    }

    fn insert_mod_metadata(
        pmap: &mut HashMap<String, Rc<RefCell<Value>>>,
        targets: &[UiModMetadata],
    ) {
        pmap.insert(
            "modulatable".to_string(),
            Rc::new(RefCell::new(Value::Bool(true))),
        );
        let target_values = targets
            .iter()
            .map(|meta| {
                let mut target = HashMap::new();
                if let Some(source_param_idx) = meta.source_param_idx {
                    target.insert(
                        "source-idx".to_string(),
                        Rc::new(RefCell::new(Value::Number(source_param_idx as f64))),
                    );
                }
                target.insert(
                    "depth-idx".to_string(),
                    Rc::new(RefCell::new(Value::Number(meta.depth_param_idx as f64))),
                );
                target.insert(
                    "source-slot".to_string(),
                    Rc::new(RefCell::new(Value::Number(meta.source_slot as f64))),
                );
                if let Some(field) = &meta.source_value_field {
                    target.insert(
                        "source-value-field".to_string(),
                        Rc::new(RefCell::new(Value::String(field.clone()))),
                    );
                }
                target.insert(
                    "depth".to_string(),
                    Rc::new(RefCell::new(Value::Number(meta.depth_value as f64))),
                );
                if let Some(field) = &meta.depth_value_field {
                    target.insert(
                        "depth-value-field".to_string(),
                        Rc::new(RefCell::new(Value::String(field.clone()))),
                    );
                }
                target.insert(
                    "depth-min".to_string(),
                    Rc::new(RefCell::new(Value::Number(meta.depth_min as f64))),
                );
                target.insert(
                    "depth-max".to_string(),
                    Rc::new(RefCell::new(Value::Number(meta.depth_max as f64))),
                );
                if let Some(unit) = &meta.depth_unit {
                    target.insert(
                        "depth-unit".to_string(),
                        Rc::new(RefCell::new(Value::String(unit.clone()))),
                    );
                }
                Rc::new(RefCell::new(Value::Map(target)))
            })
            .collect();
        pmap.insert(
            "mod-targets".to_string(),
            Rc::new(RefCell::new(Value::List(target_values))),
        );
    }

    let slots: Vec<Rc<RefCell<Value>>> = track_descs
        .iter()
        .enumerate()
        .map(|(slot_idx, desc)| {
            let mut slot_map: HashMap<String, Rc<RefCell<Value>>> = HashMap::new();

            slot_map.insert(
                "name".to_string(),
                Rc::new(RefCell::new(Value::String(desc.name.clone()))),
            );

            slot_map.insert(
                "slot-idx".to_string(),
                Rc::new(RefCell::new(Value::Number(slot_idx as f64))),
            );
            slot_map.insert(
                "builtin".to_string(),
                Rc::new(RefCell::new(Value::Bool(
                    sequencer::effects::EffectDescriptor::builtin_insert(&desc.name).is_some()
                        || sequencer::conv_reverb::is_dgen_builtin(&desc.name),
                ))),
            );

            let slot = chain.get(slot_idx);

            // Convolution Reverb: surface the current IR's display name for the
            // panel label (keyed by the live node id).
            if sequencer::conv_reverb::is_dgen_builtin(&desc.name) {
                let node_id = slot
                    .map(|s| s.node_id.load(Ordering::Relaxed) as i32)
                    .unwrap_or(0);
                let ir_name = sequencer::conv_reverb::ir_name_for(node_id)
                    .unwrap_or_else(|| "No IR".to_string());
                slot_map.insert(
                    "ir-name".to_string(),
                    Rc::new(RefCell::new(Value::String(ir_name))),
                );
            }
            let mut modulation_targets: HashMap<usize, Vec<UiModMetadata>> = HashMap::new();
            for target in desc
                .instrument_modulation_targets
                .iter()
                .filter_map(|target| {
                    let depth_desc = desc.params.get(target.depth_param_idx)?;
                    let source_default = if let Some(source_param_idx) = target.source_param_idx {
                        if let Some(slot) = slot {
                            if source_param_idx < slot.num_params.load(Ordering::Relaxed) as usize {
                                slot.defaults.get(source_param_idx)
                            } else {
                                desc.params.get(source_param_idx)?.default
                            }
                        } else {
                            desc.params.get(source_param_idx)?.default
                        }
                    } else {
                        target.modulator_slot as f32
                    };
                    let depth_default = if let Some(slot) = slot {
                        if target.depth_param_idx < slot.num_params.load(Ordering::Relaxed) as usize
                        {
                            slot.defaults.get(target.depth_param_idx)
                        } else {
                            depth_desc.default
                        }
                    } else {
                        depth_desc.default
                    };
                    let source_current = target
                        .source_param_idx
                        .and_then(|source_param_idx| {
                            plock_step.and_then(|step| {
                                slot.and_then(|slot| slot.plocks.get(step, source_param_idx))
                            })
                        })
                        .unwrap_or(source_default);
                    let depth_current = plock_step
                        .and_then(|step| {
                            slot.and_then(|slot| slot.plocks.get(step, target.depth_param_idx))
                        })
                        .unwrap_or(depth_default);
                    Some((
                        target.base_param_idx,
                        UiModMetadata {
                            source_param_idx: target.source_param_idx,
                            depth_param_idx: target.depth_param_idx,
                            source_slot: target
                                .source_param_idx
                                .and_then(|source_param_idx| {
                                    desc.params.get(source_param_idx).map(|source_desc| {
                                        source_desc.stored_to_user(source_current)
                                    })
                                })
                                .unwrap_or(source_current),
                            source_value_field: target.source_param_idx.map(|source_param_idx| {
                                let source_desc = &desc.params[source_param_idx];
                                track_effect_param_value_field(
                                    track,
                                    slot_idx,
                                    source_param_idx,
                                    &source_desc.name,
                                )
                            }),
                            depth_value: depth_desc.stored_to_user(depth_current),
                            depth_value_field: Some(track_effect_param_value_field(
                                track,
                                slot_idx,
                                target.depth_param_idx,
                                &depth_desc.name,
                            )),
                            depth_min: target.depth_min,
                            depth_max: target.depth_max,
                            depth_unit: target.depth_unit.clone(),
                        },
                    ))
                })
            {
                modulation_targets
                    .entry(target.0)
                    .or_default()
                    .push(target.1);
            }

            let params: Vec<Rc<RefCell<Value>>> = desc
                .params
                .iter()
                .enumerate()
                .filter_map(|(param_idx, pdesc)| {
                    if (is_source_param(pdesc.node_param_idx)
                        && !matches!(
                            pdesc.host_control,
                            Some(sequencer::effects::HostControl::FxSidechain { .. })
                        ))
                        || is_mod_param(&pdesc.name)
                        || is_generated_host_mod_param(&pdesc.name)
                        || is_hidden_dgen_mod_param(&pdesc.name)
                    {
                        return None;
                    }
                    let delay_synced = if desc.name == "Delay" {
                        chain
                            .get(slot_idx)
                            .map(|s| s.defaults.get(1) > 0.5)
                            .unwrap_or(false)
                    } else {
                        false
                    };
                    let default_val = chain
                        .get(slot_idx)
                        .map(|s| {
                            if param_idx < s.num_params.load(Ordering::Relaxed) as usize {
                                s.defaults.get(param_idx)
                            } else {
                                pdesc.default
                            }
                        })
                        .unwrap_or(pdesc.default);
                    // Show p-lock value if steps are selected, fall back to default
                    let current_val = plock_step
                        .and_then(|step| chain.get(slot_idx)?.plocks.get(step, param_idx))
                        .unwrap_or(default_val);

                    let mut pmap: HashMap<String, Rc<RefCell<Value>>> = HashMap::new();
                    pmap.insert(
                        "name".to_string(),
                        Rc::new(RefCell::new(Value::String(pdesc.name.clone()))),
                    );
                    pmap.insert(
                        "idx".to_string(),
                        Rc::new(RefCell::new(Value::Number(param_idx as f64))),
                    );
                    pmap.insert(
                        "value".to_string(),
                        Rc::new(RefCell::new(Value::Number(current_val as f64))),
                    );
                    pmap.insert(
                        "min".to_string(),
                        Rc::new(RefCell::new(Value::Number(pdesc.min as f64))),
                    );
                    pmap.insert(
                        "max".to_string(),
                        Rc::new(RefCell::new(Value::Number(pdesc.max as f64))),
                    );
                    match &pdesc.kind {
                        ParamKind::Boolean => {
                            pmap.insert(
                                "boolean".to_string(),
                                Rc::new(RefCell::new(Value::Bool(true))),
                            );
                            if param_supports_value_binding(pdesc) {
                                insert_string_prop(
                                    &mut pmap,
                                    "value-field",
                                    track_effect_param_value_field(
                                        track,
                                        slot_idx,
                                        param_idx,
                                        &pdesc.name,
                                    ),
                                );
                            }
                        }
                        ParamKind::Enum { labels } => {
                            let selected = labels
                                .get(current_val.round() as usize)
                                .cloned()
                                .unwrap_or_default();
                            let option_values = labels
                                .iter()
                                .cloned()
                                .map(|label| Rc::new(RefCell::new(Value::String(label))))
                                .collect();
                            pmap.insert(
                                "text-value".to_string(),
                                Rc::new(RefCell::new(Value::String(selected))),
                            );
                            pmap.insert(
                                "options".to_string(),
                                Rc::new(RefCell::new(Value::List(option_values))),
                            );
                            if param_supports_value_binding(pdesc) {
                                insert_string_prop(
                                    &mut pmap,
                                    "value-field",
                                    track_effect_param_value_field(
                                        track,
                                        slot_idx,
                                        param_idx,
                                        &pdesc.name,
                                    ),
                                );
                            }
                        }
                        ParamKind::Continuous { .. } => {
                            if desc.name == "Delay" && param_idx == 2 && delay_synced {
                                let labels: Vec<String> = SyncDivision::ALL
                                    .iter()
                                    .map(|d| d.label().to_string())
                                    .collect();
                                let selected_idx = (current_val.round() as usize)
                                    .min(labels.len().saturating_sub(1));
                                let selected =
                                    labels.get(selected_idx).cloned().unwrap_or_default();
                                let option_values = labels
                                    .into_iter()
                                    .map(|label| Rc::new(RefCell::new(Value::String(label))))
                                    .collect();
                                pmap.insert(
                                    "text-value".to_string(),
                                    Rc::new(RefCell::new(Value::String(selected))),
                                );
                                pmap.insert(
                                    "options".to_string(),
                                    Rc::new(RefCell::new(Value::List(option_values))),
                                );
                                pmap.insert(
                                    "min".to_string(),
                                    Rc::new(RefCell::new(Value::Number(0.0))),
                                );
                                pmap.insert(
                                    "max".to_string(),
                                    Rc::new(RefCell::new(Value::Number(
                                        (SyncDivision::ALL.len() - 1) as f64,
                                    ))),
                                );
                            }
                            if param_supports_value_binding(pdesc) {
                                insert_string_prop(
                                    &mut pmap,
                                    "value-field",
                                    track_effect_param_value_field(
                                        track,
                                        slot_idx,
                                        param_idx,
                                        &pdesc.name,
                                    ),
                                );
                            }
                        }
                    }
                    if let Some(targets) = modulation_targets.get(&param_idx) {
                        insert_mod_metadata(&mut pmap, targets);
                    }
                    insert_param_ui_metadata(&mut pmap, pdesc.ui_metadata.as_ref());
                    Some(Rc::new(RefCell::new(Value::Map(pmap))))
                })
                .collect();

            let source_actual =
                selected_voice_mod_source_indices_for_optional_slot(desc, slot, plock_step);
            let mut source_sections: Vec<Rc<RefCell<Value>>> = Vec::new();
            let mut source_names: Vec<Rc<RefCell<Value>>> = Vec::new();
            for slot_number in 1..=sequencer::voice_modulator::SLOT_COUNT {
                let section_name =
                    sequencer::voice_modulator::modulator_slot_label(slot_number, "");
                let mut section_params: Vec<Rc<RefCell<Value>>> = Vec::new();
                let mut source_param: Option<Rc<RefCell<Value>>> = None;
                for &param_idx in &source_actual {
                    let Some(pdesc) = desc.params.get(param_idx) else {
                        continue;
                    };
                    if sequencer::voice_modulator::slot_from_param_name(&pdesc.name)
                        != Some(slot_number)
                    {
                        continue;
                    }
                    let default_val = slot
                        .map(|slot| {
                            if param_idx < slot.num_params.load(Ordering::Relaxed) as usize {
                                slot.defaults.get(param_idx)
                            } else {
                                pdesc.default
                            }
                        })
                        .unwrap_or(pdesc.default);
                    let current_val = plock_step
                        .and_then(|step| slot.and_then(|slot| slot.plocks.get(step, param_idx)))
                        .unwrap_or(default_val);
                    let mut pmap: HashMap<String, Rc<RefCell<Value>>> = HashMap::new();
                    pmap.insert(
                        "name".to_string(),
                        Rc::new(RefCell::new(Value::String(rename_source_param(
                            &pdesc.name,
                        )))),
                    );
                    pmap.insert(
                        "idx".to_string(),
                        Rc::new(RefCell::new(Value::Number(param_idx as f64))),
                    );
                    pmap.insert(
                        "value".to_string(),
                        Rc::new(RefCell::new(Value::Number(
                            pdesc.stored_to_user(current_val) as f64,
                        ))),
                    );
                    pmap.insert(
                        "min".to_string(),
                        Rc::new(RefCell::new(Value::Number(
                            pdesc.stored_to_user(pdesc.min) as f64
                        ))),
                    );
                    pmap.insert(
                        "max".to_string(),
                        Rc::new(RefCell::new(Value::Number(
                            pdesc.stored_to_user(pdesc.max) as f64
                        ))),
                    );
                    match &pdesc.kind {
                        ParamKind::Boolean => {
                            pmap.insert(
                                "boolean".to_string(),
                                Rc::new(RefCell::new(Value::Bool(true))),
                            );
                        }
                        ParamKind::Enum { labels } => {
                            let selected = labels
                                .get(current_val.round() as usize)
                                .cloned()
                                .unwrap_or_default();
                            let option_values = labels
                                .iter()
                                .cloned()
                                .map(|label| Rc::new(RefCell::new(Value::String(label))))
                                .collect();
                            pmap.insert(
                                "text-value".to_string(),
                                Rc::new(RefCell::new(Value::String(selected))),
                            );
                            pmap.insert(
                                "options".to_string(),
                                Rc::new(RefCell::new(Value::List(option_values))),
                            );
                        }
                        ParamKind::Continuous { .. } => {}
                    }
                    insert_string_prop(
                        &mut pmap,
                        "value-field",
                        track_effect_param_value_field(track, slot_idx, param_idx, &pdesc.name),
                    );
                    let param_value = Rc::new(RefCell::new(Value::Map(pmap)));
                    if sequencer::voice_modulator::source_type_name_from_param_name(&pdesc.name)
                        == Some("source")
                    {
                        source_param = Some(param_value);
                    } else {
                        section_params.push(param_value);
                    }
                }
                source_names.push(Rc::new(RefCell::new(Value::String(section_name.clone()))));
                let mut section_map: HashMap<String, Rc<RefCell<Value>>> = HashMap::new();
                section_map.insert(
                    "name".to_string(),
                    Rc::new(RefCell::new(Value::String(section_name))),
                );
                section_map.insert(
                    "slot".to_string(),
                    Rc::new(RefCell::new(Value::Number(slot_number as f64))),
                );
                if let Some(source_param) = source_param {
                    section_map.insert("source-param".to_string(), source_param);
                }
                section_map.insert(
                    "params".to_string(),
                    Rc::new(RefCell::new(Value::List(section_params))),
                );
                source_sections.push(Rc::new(RefCell::new(Value::Map(section_map))));
            }

            slot_map.insert(
                "params".to_string(),
                Rc::new(RefCell::new(Value::List(params))),
            );
            slot_map.insert(
                "modulators".to_string(),
                Rc::new(RefCell::new(Value::List(
                    desc.instrument_modulators
                        .iter()
                        .map(|modulator| {
                            let mut map: HashMap<String, Rc<RefCell<Value>>> = HashMap::new();
                            map.insert(
                                "slot".to_string(),
                                Rc::new(RefCell::new(Value::Number(modulator.slot as f64))),
                            );
                            map.insert(
                                "label".to_string(),
                                Rc::new(RefCell::new(Value::String(modulator.label.clone()))),
                            );
                            Rc::new(RefCell::new(Value::Map(map)))
                        })
                        .collect(),
                ))),
            );
            slot_map.insert(
                "source-names".to_string(),
                Rc::new(RefCell::new(Value::List(source_names))),
            );
            slot_map.insert(
                "sources".to_string(),
                Rc::new(RefCell::new(Value::List(source_sections))),
            );

            Rc::new(RefCell::new(Value::Map(slot_map)))
        })
        .collect();

    Value::List(slots)
}

pub(crate) fn build_bus_effects_value(app: &ui::App) -> Value {
    build_bus_effects_value_for_selection(app, None)
}

pub(crate) fn build_bus_effects_value_for_selection(
    app: &ui::App,
    selected: Option<&Arc<Mutex<HashSet<usize>>>>,
) -> Value {
    use sequencer::effects::ParamKind;
    use std::collections::HashMap;

    let plock_step = selected.and_then(|selected| selected.lock().unwrap().iter().copied().min());

    let buses: Vec<Rc<RefCell<Value>>> = app
        .buses
        .iter()
        .enumerate()
        .map(|(bus_idx, bus)| {
            let slots: Vec<Rc<RefCell<Value>>> = bus
                .effect_descriptors
                .iter()
                .enumerate()
                .map(|(slot_idx, desc)| {
                    let mut slot_map: HashMap<String, Rc<RefCell<Value>>> = HashMap::new();
                    slot_map.insert(
                        "name".to_string(),
                        Rc::new(RefCell::new(Value::String(desc.name.clone()))),
                    );
                    slot_map.insert(
                        "slot-idx".to_string(),
                        Rc::new(RefCell::new(Value::Number(slot_idx as f64))),
                    );
                    slot_map.insert(
                        "bus-idx".to_string(),
                        Rc::new(RefCell::new(Value::Number(bus_idx as f64))),
                    );
                    slot_map.insert(
                        "bus-fx".to_string(),
                        Rc::new(RefCell::new(Value::Bool(true))),
                    );
                    slot_map.insert(
                        "builtin".to_string(),
                        Rc::new(RefCell::new(Value::Bool(
                            sequencer::effects::EffectDescriptor::builtin_insert(&desc.name)
                                .is_some()
                                || sequencer::conv_reverb::is_dgen_builtin(&desc.name),
                        ))),
                    );
                    // Convolution Reverb: surface the current IR name for the label.
                    if sequencer::conv_reverb::is_dgen_builtin(&desc.name) {
                        let node_id = bus
                            .effect_slots
                            .get(slot_idx)
                            .map(|s| s.node_id as i32)
                            .unwrap_or(0);
                        let ir_name = sequencer::conv_reverb::ir_name_for(node_id)
                            .unwrap_or_else(|| "No IR".to_string());
                        slot_map.insert(
                            "ir-name".to_string(),
                            Rc::new(RefCell::new(Value::String(ir_name))),
                        );
                    }

                    let params: Vec<Rc<RefCell<Value>>> = desc
                        .params
                        .iter()
                        .enumerate()
                        .map(|(param_idx, pdesc)| {
                            let current_val = bus
                                .effect_slots
                                .get(slot_idx)
                                .and_then(|slot| {
                                    plock_step
                                        .and_then(|step| {
                                            slot.plocks
                                                .get(step)
                                                .and_then(|step_plocks| step_plocks.get(param_idx))
                                                .copied()
                                                .flatten()
                                        })
                                        .or_else(|| slot.defaults.get(param_idx).copied())
                                })
                                .unwrap_or(pdesc.default);
                            let mut pmap: HashMap<String, Rc<RefCell<Value>>> = HashMap::new();
                            pmap.insert(
                                "name".to_string(),
                                Rc::new(RefCell::new(Value::String(pdesc.name.clone()))),
                            );
                            pmap.insert(
                                "idx".to_string(),
                                Rc::new(RefCell::new(Value::Number(param_idx as f64))),
                            );
                            pmap.insert(
                                "value".to_string(),
                                Rc::new(RefCell::new(Value::Number(current_val as f64))),
                            );
                            pmap.insert(
                                "min".to_string(),
                                Rc::new(RefCell::new(Value::Number(pdesc.min as f64))),
                            );
                            pmap.insert(
                                "max".to_string(),
                                Rc::new(RefCell::new(Value::Number(pdesc.max as f64))),
                            );
                            match &pdesc.kind {
                                ParamKind::Boolean => {
                                    pmap.insert(
                                        "boolean".to_string(),
                                        Rc::new(RefCell::new(Value::Bool(true))),
                                    );
                                    if param_supports_value_binding(pdesc) {
                                        insert_string_prop(
                                            &mut pmap,
                                            "value-field",
                                            bus_effect_param_value_field(
                                                bus_idx,
                                                slot_idx,
                                                param_idx,
                                                &pdesc.name,
                                            ),
                                        );
                                    }
                                }
                                ParamKind::Enum { labels } => {
                                    let selected = labels
                                        .get(current_val.round() as usize)
                                        .cloned()
                                        .unwrap_or_default();
                                    let option_values = labels
                                        .iter()
                                        .cloned()
                                        .map(|label| Rc::new(RefCell::new(Value::String(label))))
                                        .collect();
                                    pmap.insert(
                                        "text-value".to_string(),
                                        Rc::new(RefCell::new(Value::String(selected))),
                                    );
                                    pmap.insert(
                                        "options".to_string(),
                                        Rc::new(RefCell::new(Value::List(option_values))),
                                    );
                                    if param_supports_value_binding(pdesc) {
                                        insert_string_prop(
                                            &mut pmap,
                                            "value-field",
                                            bus_effect_param_value_field(
                                                bus_idx,
                                                slot_idx,
                                                param_idx,
                                                &pdesc.name,
                                            ),
                                        );
                                    }
                                }
                                ParamKind::Continuous { .. } => {
                                    if param_supports_value_binding(pdesc) {
                                        insert_string_prop(
                                            &mut pmap,
                                            "value-field",
                                            bus_effect_param_value_field(
                                                bus_idx,
                                                slot_idx,
                                                param_idx,
                                                &pdesc.name,
                                            ),
                                        );
                                    }
                                }
                            }
                            Rc::new(RefCell::new(Value::Map(pmap)))
                        })
                        .collect();
                    slot_map.insert(
                        "params".to_string(),
                        Rc::new(RefCell::new(Value::List(params))),
                    );
                    Rc::new(RefCell::new(Value::Map(slot_map)))
                })
                .collect();
            Rc::new(RefCell::new(Value::List(slots)))
        })
        .collect();

    Value::List(buses)
}

pub(crate) fn build_midi_effects_value(
    state: &Arc<SequencerState>,
    track: usize,
    selected: &Arc<Mutex<HashSet<usize>>>,
) -> Value {
    use sequencer::effects::{EffectDescriptor, ParamKind};
    use std::collections::HashMap;

    let descriptors = sequencer::lisp_host::load_midi_fx_descriptors();
    let descriptor_for = |name: &str| -> Option<EffectDescriptor> {
        descriptors
            .iter()
            .find(|desc| desc.name.eq_ignore_ascii_case(name))
            .cloned()
    };
    let Some(track_params) = state.pattern.track_params.get(track) else {
        return Value::List(vec![]);
    };
    let chain = track_params.midi_fx_chain();
    let sel = selected.lock().unwrap();
    let plock_step = sel.iter().copied().min();

    let slots: Vec<Rc<RefCell<Value>>> = chain
        .iter()
        .enumerate()
        .filter_map(|(slot_idx, name)| {
            let desc = descriptor_for(name)?;
            let slot = state
                .pattern
                .midi_fx_slots
                .get(track)
                .and_then(|slots| slots.get(slot_idx));
            let mut slot_map: HashMap<String, Rc<RefCell<Value>>> = HashMap::new();
            slot_map.insert(
                "name".to_string(),
                Rc::new(RefCell::new(Value::String(desc.name.clone()))),
            );
            slot_map.insert(
                "slot-idx".to_string(),
                Rc::new(RefCell::new(Value::Number(slot_idx as f64))),
            );
            slot_map.insert(
                "midi-fx".to_string(),
                Rc::new(RefCell::new(Value::Bool(true))),
            );

            let params: Vec<Rc<RefCell<Value>>> = desc
                .params
                .iter()
                .enumerate()
                .map(|(param_idx, pdesc)| {
                    let default_val = slot
                        .map(|s| {
                            if param_idx < s.num_params.load(Ordering::Relaxed) as usize {
                                s.defaults.get(param_idx)
                            } else {
                                pdesc.default
                            }
                        })
                        .unwrap_or(pdesc.default);
                    let current_val = plock_step
                        .and_then(|step| slot.and_then(|s| s.plocks.get(step, param_idx)))
                        .unwrap_or(default_val);
                    let mut pmap: HashMap<String, Rc<RefCell<Value>>> = HashMap::new();
                    pmap.insert(
                        "name".to_string(),
                        Rc::new(RefCell::new(Value::String(pdesc.name.clone()))),
                    );
                    pmap.insert(
                        "idx".to_string(),
                        Rc::new(RefCell::new(Value::Number(param_idx as f64))),
                    );
                    pmap.insert(
                        "value".to_string(),
                        Rc::new(RefCell::new(Value::Number(current_val as f64))),
                    );
                    pmap.insert(
                        "min".to_string(),
                        Rc::new(RefCell::new(Value::Number(pdesc.min as f64))),
                    );
                    pmap.insert(
                        "max".to_string(),
                        Rc::new(RefCell::new(Value::Number(pdesc.max as f64))),
                    );
                    match &pdesc.kind {
                        ParamKind::Boolean => {
                            pmap.insert(
                                "boolean".to_string(),
                                Rc::new(RefCell::new(Value::Bool(true))),
                            );
                            if param_supports_value_binding(pdesc) {
                                insert_string_prop(
                                    &mut pmap,
                                    "value-field",
                                    midi_fx_param_value_field(
                                        track,
                                        slot_idx,
                                        param_idx,
                                        &pdesc.name,
                                    ),
                                );
                            }
                        }
                        ParamKind::Enum { labels } => {
                            let selected = labels
                                .get(current_val.round() as usize)
                                .cloned()
                                .unwrap_or_default();
                            let option_values = labels
                                .iter()
                                .cloned()
                                .map(|label| Rc::new(RefCell::new(Value::String(label))))
                                .collect();
                            pmap.insert(
                                "text-value".to_string(),
                                Rc::new(RefCell::new(Value::String(selected))),
                            );
                            pmap.insert(
                                "options".to_string(),
                                Rc::new(RefCell::new(Value::List(option_values))),
                            );
                            if param_supports_value_binding(pdesc) {
                                insert_string_prop(
                                    &mut pmap,
                                    "value-field",
                                    midi_fx_param_value_field(
                                        track,
                                        slot_idx,
                                        param_idx,
                                        &pdesc.name,
                                    ),
                                );
                            }
                        }
                        ParamKind::Continuous { .. } => {
                            if param_supports_value_binding(pdesc) {
                                insert_string_prop(
                                    &mut pmap,
                                    "value-field",
                                    midi_fx_param_value_field(
                                        track,
                                        slot_idx,
                                        param_idx,
                                        &pdesc.name,
                                    ),
                                );
                            }
                        }
                    }
                    Rc::new(RefCell::new(Value::Map(pmap)))
                })
                .collect();
            slot_map.insert(
                "params".to_string(),
                Rc::new(RefCell::new(Value::List(params))),
            );
            Some(Rc::new(RefCell::new(Value::Map(slot_map))))
        })
        .collect();

    Value::List(slots)
}

pub(crate) fn build_sampler_panel_value(
    app: &ui::App,
    track: usize,
    selected: &Arc<Mutex<HashSet<usize>>>,
) -> Value {
    use std::collections::HashMap;

    fn is_mod_param(name: &str) -> bool {
        name.starts_with("mod ")
    }

    fn is_generated_host_mod_param(name: &str) -> bool {
        name.starts_with("__host_mod__")
    }

    fn is_hidden_dgen_mod_param(name: &str) -> bool {
        name.starts_with("__dgen_mod_active__")
    }

    fn is_source_param(node_param_idx: u32) -> bool {
        sequencer::voice_modulator::is_source_param(node_param_idx)
    }

    fn rename_source_param(name: &str) -> String {
        sequencer::voice_modulator::source_param_display_name(name)
    }

    app.publish_sampler_analysis_runtime(track);

    let sel = selected.lock().unwrap();
    let plock_step = sel.iter().copied().min();
    let slot = &app.state.pattern.instrument_slots[track];
    let desc = app
        .graph
        .instrument_descriptors
        .get(track)
        .cloned()
        .unwrap_or_else(sequencer::effects::EffectDescriptor::builtin_sampler);

    // Look up the pre-registered SampleBuffer and pass its Value map directly
    // to the Lisp side, so the waveform widget can use it without re-loading.
    let sampler_path = app.sampler_path_for_track(track);
    let registered_sample = sampler_path.as_ref().and_then(|p| {
        let key = p.display().to_string();
        eseqlisp::audio::sample::get_registered_sample(&key).or_else(|| {
            match eseqlisp::audio::sample::SampleBuffer::load_wav(p) {
                Ok(sample) => {
                    sample.register();
                    eseqlisp::audio::sample::get_registered_sample(&key)
                }
                Err(e) => {
                    eprintln!("waveform: failed to register sample {}: {e}", p.display());
                    None
                }
            }
        })
    });
    let buffer_value = registered_sample.as_ref().map(|s| s.to_value());
    let sample_duration = registered_sample
        .as_ref()
        .map(|s| s.duration_seconds)
        .unwrap_or(1.0);

    struct UiModMetadata {
        source_param_idx: Option<usize>,
        depth_param_idx: usize,
        source_slot: f32,
        source_value_field: Option<String>,
        depth_value: f32,
        depth_value_field: Option<String>,
        depth_min: f32,
        depth_max: f32,
        depth_unit: Option<String>,
    }

    fn insert_mod_metadata(
        pmap: &mut HashMap<String, Rc<RefCell<Value>>>,
        targets: &[UiModMetadata],
    ) {
        pmap.insert(
            "modulatable".to_string(),
            Rc::new(RefCell::new(Value::Bool(true))),
        );
        let target_values = targets
            .iter()
            .map(|meta| {
                let mut target = HashMap::new();
                if let Some(source_param_idx) = meta.source_param_idx {
                    target.insert(
                        "source-idx".to_string(),
                        Rc::new(RefCell::new(Value::Number(source_param_idx as f64))),
                    );
                }
                target.insert(
                    "depth-idx".to_string(),
                    Rc::new(RefCell::new(Value::Number(meta.depth_param_idx as f64))),
                );
                target.insert(
                    "source-slot".to_string(),
                    Rc::new(RefCell::new(Value::Number(meta.source_slot as f64))),
                );
                if let Some(field) = &meta.source_value_field {
                    target.insert(
                        "source-value-field".to_string(),
                        Rc::new(RefCell::new(Value::String(field.clone()))),
                    );
                }
                target.insert(
                    "depth".to_string(),
                    Rc::new(RefCell::new(Value::Number(meta.depth_value as f64))),
                );
                if let Some(field) = &meta.depth_value_field {
                    target.insert(
                        "depth-value-field".to_string(),
                        Rc::new(RefCell::new(Value::String(field.clone()))),
                    );
                }
                target.insert(
                    "depth-min".to_string(),
                    Rc::new(RefCell::new(Value::Number(meta.depth_min as f64))),
                );
                target.insert(
                    "depth-max".to_string(),
                    Rc::new(RefCell::new(Value::Number(meta.depth_max as f64))),
                );
                if let Some(unit) = &meta.depth_unit {
                    target.insert(
                        "depth-unit".to_string(),
                        Rc::new(RefCell::new(Value::String(unit.clone()))),
                    );
                }
                Rc::new(RefCell::new(Value::Map(target)))
            })
            .collect();
        pmap.insert(
            "mod-targets".to_string(),
            Rc::new(RefCell::new(Value::List(target_values))),
        );
    }

    let mut modulation_targets: HashMap<usize, Vec<UiModMetadata>> = HashMap::new();
    for target in desc
        .instrument_modulation_targets
        .iter()
        .filter_map(|target| {
            let depth_desc = desc.params.get(target.depth_param_idx)?;
            let source_default = if let Some(source_param_idx) = target.source_param_idx {
                if source_param_idx < slot.num_params.load(Ordering::Relaxed) as usize {
                    slot.defaults.get(source_param_idx)
                } else {
                    desc.params.get(source_param_idx)?.default
                }
            } else {
                target.modulator_slot as f32
            };
            let depth_default =
                if target.depth_param_idx < slot.num_params.load(Ordering::Relaxed) as usize {
                    slot.defaults.get(target.depth_param_idx)
                } else {
                    depth_desc.default
                };
            let source_current = target
                .source_param_idx
                .and_then(|source_param_idx| {
                    plock_step.and_then(|step| slot.plocks.get(step, source_param_idx))
                })
                .unwrap_or(source_default);
            let depth_current = plock_step
                .and_then(|step| slot.plocks.get(step, target.depth_param_idx))
                .unwrap_or(depth_default);
            let (depth_min, depth_max) = sampler_modulation_depth_display_range(depth_desc, target);
            Some((
                target.base_param_idx,
                UiModMetadata {
                    source_param_idx: target.source_param_idx,
                    depth_param_idx: target.depth_param_idx,
                    source_slot: target
                        .source_param_idx
                        .and_then(|source_param_idx| {
                            desc.params
                                .get(source_param_idx)
                                .map(|source_desc| source_desc.stored_to_user(source_current))
                        })
                        .unwrap_or(source_current),
                    source_value_field: target.source_param_idx.map(|source_param_idx| {
                        let source_desc = &desc.params[source_param_idx];
                        instrument_param_value_field(track, source_param_idx, &source_desc.name)
                    }),
                    depth_value: depth_desc.stored_to_user(depth_current),
                    depth_value_field: Some(instrument_param_value_field(
                        track,
                        target.depth_param_idx,
                        &depth_desc.name,
                    )),
                    depth_min,
                    depth_max,
                    depth_unit: target.depth_unit.clone(),
                },
            ))
        })
    {
        modulation_targets
            .entry(target.0)
            .or_default()
            .push(target.1);
    }

    let mut synth_params: Vec<Rc<RefCell<Value>>> = Vec::new();
    let mut mod_params: Vec<Rc<RefCell<Value>>> = Vec::new();
    let mut source_params_by_slot: HashMap<usize, Vec<Rc<RefCell<Value>>>> = HashMap::new();
    let mut source_type_param_by_slot: HashMap<usize, Rc<RefCell<Value>>> = HashMap::new();
    let visible_source_indices: std::collections::HashSet<usize> =
        selected_voice_mod_source_indices(&desc, slot, plock_step)
            .into_iter()
            .collect();
    let base_note = f32::from_bits(
        app.state.pattern.instrument_base_note_offsets[track].load(Ordering::Relaxed),
    );
    {
        let mut pmap: HashMap<String, Rc<RefCell<Value>>> = HashMap::new();
        pmap.insert(
            "name".to_string(),
            Rc::new(RefCell::new(Value::String("base".to_string()))),
        );
        pmap.insert(
            "control".to_string(),
            Rc::new(RefCell::new(Value::String("base-note".to_string()))),
        );
        pmap.insert(
            "value".to_string(),
            Rc::new(RefCell::new(Value::Number(base_note as f64))),
        );
        pmap.insert(
            "min".to_string(),
            Rc::new(RefCell::new(Value::Number(-48.0))),
        );
        pmap.insert(
            "max".to_string(),
            Rc::new(RefCell::new(Value::Number(48.0))),
        );
        insert_string_prop(
            &mut pmap,
            "value-field",
            instrument_base_note_value_field(track),
        );
        synth_params.push(Rc::new(RefCell::new(Value::Map(pmap))));
    }
    for (param_idx, pdesc) in desc.params.iter().enumerate() {
        let default_val = if param_idx < slot.num_params.load(Ordering::Relaxed) as usize {
            slot.defaults.get(param_idx)
        } else {
            pdesc.default
        };
        let current_val = plock_step
            .and_then(|step| slot.plocks.get(step, param_idx))
            .unwrap_or(default_val);
        let mut pmap: HashMap<String, Rc<RefCell<Value>>> = HashMap::new();
        pmap.insert(
            "name".to_string(),
            Rc::new(RefCell::new(Value::String(pdesc.name.clone()))),
        );
        pmap.insert(
            "idx".to_string(),
            Rc::new(RefCell::new(Value::Number(param_idx as f64))),
        );
        pmap.insert(
            "control".to_string(),
            Rc::new(RefCell::new(Value::String("param".to_string()))),
        );
        pmap.insert(
            "value".to_string(),
            Rc::new(RefCell::new(Value::Number(
                pdesc.stored_to_user(current_val) as f64,
            ))),
        );
        pmap.insert(
            "min".to_string(),
            Rc::new(RefCell::new(Value::Number(
                pdesc.stored_to_user(pdesc.min) as f64
            ))),
        );
        pmap.insert(
            "max".to_string(),
            Rc::new(RefCell::new(Value::Number(
                pdesc.stored_to_user(pdesc.max) as f64
            ))),
        );
        match &pdesc.kind {
            sequencer::effects::ParamKind::Boolean => {
                pmap.insert(
                    "boolean".to_string(),
                    Rc::new(RefCell::new(Value::Bool(true))),
                );
                if param_supports_value_binding(pdesc) {
                    insert_string_prop(
                        &mut pmap,
                        "value-field",
                        instrument_param_value_field(track, param_idx, &pdesc.name),
                    );
                }
            }
            sequencer::effects::ParamKind::Enum { labels } => {
                let selected = labels
                    .get(current_val.round() as usize)
                    .cloned()
                    .unwrap_or_default();
                let option_values = labels
                    .iter()
                    .cloned()
                    .map(|label| Rc::new(RefCell::new(Value::String(label))))
                    .collect();
                pmap.insert(
                    "text-value".to_string(),
                    Rc::new(RefCell::new(Value::String(selected))),
                );
                pmap.insert(
                    "options".to_string(),
                    Rc::new(RefCell::new(Value::List(option_values))),
                );
                if param_supports_value_binding(pdesc) {
                    insert_string_prop(
                        &mut pmap,
                        "value-field",
                        instrument_param_value_field(track, param_idx, &pdesc.name),
                    );
                }
            }
            sequencer::effects::ParamKind::Continuous { .. } => {
                if param_supports_value_binding(pdesc) {
                    insert_string_prop(
                        &mut pmap,
                        "value-field",
                        instrument_param_value_field(track, param_idx, &pdesc.name),
                    );
                }
            }
        }
        if is_generated_host_mod_param(&pdesc.name) || is_hidden_dgen_mod_param(&pdesc.name) {
            continue;
        }
        if is_source_param(pdesc.node_param_idx) {
            if let Some(Value::String(name)) = pmap.get("name").map(|v| v.borrow().clone()) {
                pmap.insert(
                    "name".to_string(),
                    Rc::new(RefCell::new(Value::String(rename_source_param(&name)))),
                );
            }
            if let Some(slot_number) = sequencer::voice_modulator::slot_from_param_name(&pdesc.name)
            {
                let param_value = Rc::new(RefCell::new(Value::Map(pmap)));
                if sequencer::voice_modulator::source_type_name_from_param_name(&pdesc.name)
                    == Some("source")
                {
                    source_type_param_by_slot.insert(slot_number, param_value);
                } else if visible_source_indices.contains(&param_idx) {
                    source_params_by_slot
                        .entry(slot_number)
                        .or_default()
                        .push(param_value);
                }
            }
        } else if is_mod_param(&pdesc.name) {
            if let Some(Value::String(name)) = pmap.get("name").map(|v| v.borrow().clone()) {
                pmap.insert(
                    "name".to_string(),
                    Rc::new(RefCell::new(Value::String(
                        name.strip_prefix("mod ").unwrap_or(&name).to_string(),
                    ))),
                );
            }
            mod_params.push(Rc::new(RefCell::new(Value::Map(pmap))));
        } else {
            if let Some(targets) = modulation_targets.get(&param_idx) {
                insert_mod_metadata(&mut pmap, targets);
            }
            synth_params.push(Rc::new(RefCell::new(Value::Map(pmap))));
        }
    }

    let mut source_sections: Vec<Rc<RefCell<Value>>> = Vec::new();
    let mut source_names: Vec<Rc<RefCell<Value>>> = Vec::new();
    for slot_number in 1..=sequencer::voice_modulator::SLOT_COUNT {
        let section_name = sequencer::voice_modulator::modulator_slot_label(slot_number, "");
        let params = source_params_by_slot
            .remove(&slot_number)
            .unwrap_or_default();
        let source_param = source_type_param_by_slot.remove(&slot_number);
        source_names.push(Rc::new(RefCell::new(Value::String(section_name.clone()))));
        let mut section_map: HashMap<String, Rc<RefCell<Value>>> = HashMap::new();
        section_map.insert(
            "name".to_string(),
            Rc::new(RefCell::new(Value::String(section_name))),
        );
        section_map.insert(
            "slot".to_string(),
            Rc::new(RefCell::new(Value::Number(slot_number as f64))),
        );
        if let Some(source_param) = source_param {
            section_map.insert("source-param".to_string(), source_param);
        }
        section_map.insert(
            "params".to_string(),
            Rc::new(RefCell::new(Value::List(params))),
        );
        source_sections.push(Rc::new(RefCell::new(Value::Map(section_map))));
    }

    let mut panel_map: HashMap<String, Rc<RefCell<Value>>> = HashMap::new();
    panel_map.insert(
        "type".to_string(),
        Rc::new(RefCell::new(Value::String("sampler".to_string()))),
    );
    panel_map.insert(
        "track".to_string(),
        Rc::new(RefCell::new(Value::Number(track as f64))),
    );
    if let Some(buf_val) = buffer_value {
        panel_map.insert("buffer".to_string(), Rc::new(RefCell::new(buf_val)));
    }
    let buffer_id = app.graph.track_buffer_ids.get(track).copied().unwrap_or(-1);
    let analysis_entry = app.sample_analysis.cache().get(buffer_id);
    let mut analysis_status = "none".to_string();
    let mut analysis_message = String::new();
    let mut onset_values: Vec<Rc<RefCell<Value>>> = Vec::new();
    if let Some(entry) = analysis_entry {
        match entry.as_ref() {
            sequencer::analysis::AnalysisEntry::Pending => {
                analysis_status = "pending".to_string();
                analysis_message = "Analyzing...".to_string();
            }
            sequencer::analysis::AnalysisEntry::Ready(result) => {
                analysis_status = "ready".to_string();
                analysis_message = format!("{:.1} BPM", result.bpm);
                panel_map.insert(
                    "analysis-bpm".to_string(),
                    Rc::new(RefCell::new(Value::Number(result.bpm as f64))),
                );
                panel_map.insert(
                    "analysis-confidence".to_string(),
                    Rc::new(RefCell::new(Value::Number(result.bpm_confidence as f64))),
                );
                if let Some(frame) = result.downbeat_frame {
                    let seconds = frame as f64 / app.graph.sample_rate.max(1) as f64;
                    panel_map.insert(
                        "downbeat-time".to_string(),
                        Rc::new(RefCell::new(Value::Number(seconds))),
                    );
                }
                onset_values = result
                    .onsets_frames
                    .iter()
                    .map(|frame| {
                        Rc::new(RefCell::new(Value::Number(
                            *frame as f64 / app.graph.sample_rate.max(1) as f64,
                        )))
                    })
                    .collect();
            }
            sequencer::analysis::AnalysisEntry::Failed(error) => {
                analysis_status = "failed".to_string();
                analysis_message = error.clone();
            }
        }
    }
    panel_map.insert(
        "analysis-status".to_string(),
        Rc::new(RefCell::new(Value::String(analysis_status))),
    );
    panel_map.insert(
        "analysis-message".to_string(),
        Rc::new(RefCell::new(Value::String(analysis_message))),
    );
    panel_map.insert(
        "onsets".to_string(),
        Rc::new(RefCell::new(Value::List(onset_values))),
    );
    panel_map.insert(
        "params".to_string(),
        Rc::new(RefCell::new(Value::List(synth_params.clone()))),
    );
    panel_map.insert(
        "synth".to_string(),
        Rc::new(RefCell::new(Value::List(synth_params))),
    );
    panel_map.insert(
        "mod".to_string(),
        Rc::new(RefCell::new(Value::List(mod_params))),
    );
    panel_map.insert(
        "modulators".to_string(),
        Rc::new(RefCell::new(Value::List(
            desc.instrument_modulators
                .iter()
                .map(|modulator| {
                    let mut map: HashMap<String, Rc<RefCell<Value>>> = HashMap::new();
                    map.insert(
                        "slot".to_string(),
                        Rc::new(RefCell::new(Value::Number(modulator.slot as f64))),
                    );
                    map.insert(
                        "label".to_string(),
                        Rc::new(RefCell::new(Value::String(modulator.label.clone()))),
                    );
                    Rc::new(RefCell::new(Value::Map(map)))
                })
                .collect(),
        ))),
    );
    panel_map.insert(
        "source-names".to_string(),
        Rc::new(RefCell::new(Value::List(source_names))),
    );
    panel_map.insert(
        "sources".to_string(),
        Rc::new(RefCell::new(Value::List(source_sections))),
    );
    // Start/end as seconds for the waveform selection overlay.
    // Raw stored values are 0.0-1.0 normalized; multiply by duration.
    let start_raw = plock_step
        .and_then(|step| slot.plocks.get(step, 2))
        .unwrap_or_else(|| slot.defaults.get(2));
    let end_raw = plock_step
        .and_then(|step| slot.plocks.get(step, 3))
        .unwrap_or_else(|| slot.defaults.get(3));
    panel_map.insert(
        "start-time".to_string(),
        Rc::new(RefCell::new(Value::Number(
            (start_raw as f64) * sample_duration,
        ))),
    );
    insert_string_prop(
        &mut panel_map,
        "start-time-field",
        sampler_selection_time_field(track, "start"),
    );
    panel_map.insert(
        "end-time".to_string(),
        Rc::new(RefCell::new(Value::Number(
            (end_raw as f64) * sample_duration,
        ))),
    );
    insert_string_prop(
        &mut panel_map,
        "end-time-field",
        sampler_selection_time_field(track, "end"),
    );
    panel_map.insert(
        "duration".to_string(),
        Rc::new(RefCell::new(Value::Number(sample_duration))),
    );

    Value::List(vec![Rc::new(RefCell::new(Value::Map(panel_map)))])
}

pub(crate) fn build_instrument_panel_value(
    app: &ui::App,
    track: usize,
    selected: &Arc<Mutex<HashSet<usize>>>,
) -> Value {
    use std::collections::HashMap;

    if app.is_sampler_track(track) {
        return build_sampler_panel_value(app, track, selected);
    }
    let Some(desc) = app.graph.instrument_descriptors.get(track) else {
        return Value::List(vec![]);
    };
    if desc.params.is_empty() {
        return Value::List(vec![]);
    }

    let sel = selected.lock().unwrap();
    let plock_step = sel.iter().copied().min();
    let slot = &app.state.pattern.instrument_slots[track];
    let base_note_default = f32::from_bits(
        app.state.pattern.instrument_base_note_offsets[track].load(Ordering::Relaxed),
    );
    let base_note_current = base_note_default;

    struct UiModMetadata {
        source_param_idx: Option<usize>,
        depth_param_idx: usize,
        source_slot: f32,
        source_value_field: Option<String>,
        depth_value: f32,
        depth_value_field: Option<String>,
        depth_min: f32,
        depth_max: f32,
        depth_unit: Option<String>,
    }

    fn push_param(
        out: &mut Vec<Rc<RefCell<Value>>>,
        name: String,
        control: &str,
        idx: Option<usize>,
        value: f32,
        min: f32,
        max: f32,
        options: Option<&Vec<String>>,
        value_field: Option<String>,
        mod_targets: Option<&Vec<UiModMetadata>>,
        ui_metadata: Option<&sequencer::effects::ParamUiMetadata>,
    ) {
        let is_boolean_name = name == "enabled" || name == "sync";
        let mut pmap: HashMap<String, Rc<RefCell<Value>>> = HashMap::new();
        pmap.insert(
            "name".to_string(),
            Rc::new(RefCell::new(Value::String(name))),
        );
        pmap.insert(
            "control".to_string(),
            Rc::new(RefCell::new(Value::String(control.to_string()))),
        );
        if let Some(idx) = idx {
            pmap.insert(
                "idx".to_string(),
                Rc::new(RefCell::new(Value::Number(idx as f64))),
            );
        }
        pmap.insert(
            "value".to_string(),
            Rc::new(RefCell::new(Value::Number(value as f64))),
        );
        pmap.insert(
            "min".to_string(),
            Rc::new(RefCell::new(Value::Number(min as f64))),
        );
        pmap.insert(
            "max".to_string(),
            Rc::new(RefCell::new(Value::Number(max as f64))),
        );
        if let Some(labels) = options {
            let selected = labels
                .get(value.round() as usize)
                .cloned()
                .unwrap_or_default();
            let option_values = labels
                .iter()
                .cloned()
                .map(|label| Rc::new(RefCell::new(Value::String(label))))
                .collect();
            pmap.insert(
                "text-value".to_string(),
                Rc::new(RefCell::new(Value::String(selected))),
            );
            pmap.insert(
                "options".to_string(),
                Rc::new(RefCell::new(Value::List(option_values))),
            );
        }
        if options.is_none() && is_boolean_name {
            pmap.insert(
                "boolean".to_string(),
                Rc::new(RefCell::new(Value::Bool(true))),
            );
        }
        if let Some(value_field) = value_field {
            insert_string_prop(&mut pmap, "value-field", value_field);
        }
        if let Some(targets) = mod_targets {
            pmap.insert(
                "modulatable".to_string(),
                Rc::new(RefCell::new(Value::Bool(true))),
            );
            let target_values = targets
                .iter()
                .map(|meta| {
                    let mut target = HashMap::new();
                    if let Some(source_param_idx) = meta.source_param_idx {
                        target.insert(
                            "source-idx".to_string(),
                            Rc::new(RefCell::new(Value::Number(source_param_idx as f64))),
                        );
                    }
                    target.insert(
                        "depth-idx".to_string(),
                        Rc::new(RefCell::new(Value::Number(meta.depth_param_idx as f64))),
                    );
                    target.insert(
                        "source-slot".to_string(),
                        Rc::new(RefCell::new(Value::Number(meta.source_slot as f64))),
                    );
                    if let Some(field) = &meta.source_value_field {
                        target.insert(
                            "source-value-field".to_string(),
                            Rc::new(RefCell::new(Value::String(field.clone()))),
                        );
                    }
                    target.insert(
                        "depth".to_string(),
                        Rc::new(RefCell::new(Value::Number(meta.depth_value as f64))),
                    );
                    if let Some(field) = &meta.depth_value_field {
                        target.insert(
                            "depth-value-field".to_string(),
                            Rc::new(RefCell::new(Value::String(field.clone()))),
                        );
                    }
                    target.insert(
                        "depth-min".to_string(),
                        Rc::new(RefCell::new(Value::Number(meta.depth_min as f64))),
                    );
                    target.insert(
                        "depth-max".to_string(),
                        Rc::new(RefCell::new(Value::Number(meta.depth_max as f64))),
                    );
                    if let Some(unit) = &meta.depth_unit {
                        target.insert(
                            "depth-unit".to_string(),
                            Rc::new(RefCell::new(Value::String(unit.clone()))),
                        );
                    }
                    Rc::new(RefCell::new(Value::Map(target)))
                })
                .collect();
            pmap.insert(
                "mod-targets".to_string(),
                Rc::new(RefCell::new(Value::List(target_values))),
            );
        }
        insert_param_ui_metadata(&mut pmap, ui_metadata);
        out.push(Rc::new(RefCell::new(Value::Map(pmap))));
    }

    fn is_mod_param(name: &str) -> bool {
        name.starts_with("mod ")
    }

    fn is_generated_host_mod_param(name: &str) -> bool {
        name.starts_with("__host_mod__")
    }

    fn is_hidden_dgen_mod_param(name: &str) -> bool {
        name.starts_with("__dgen_mod_active__")
    }

    fn is_source_param(node_param_idx: u32) -> bool {
        sequencer::voice_modulator::is_source_param(node_param_idx)
    }

    fn rename_source_param(name: &str) -> String {
        sequencer::voice_modulator::source_param_display_name(name)
    }

    let source_actual = selected_voice_mod_source_indices(desc, slot, plock_step);

    let mut synth_params: Vec<Rc<RefCell<Value>>> = Vec::new();
    let mut mod_params: Vec<Rc<RefCell<Value>>> = Vec::new();
    let mut modulation_targets: HashMap<usize, Vec<UiModMetadata>> = HashMap::new();
    for target in desc
        .instrument_modulation_targets
        .iter()
        .filter_map(|target| {
            let depth_desc = desc.params.get(target.depth_param_idx)?;
            let source_default = if let Some(source_param_idx) = target.source_param_idx {
                if source_param_idx < slot.num_params.load(Ordering::Relaxed) as usize {
                    slot.defaults.get(source_param_idx)
                } else {
                    desc.params.get(source_param_idx)?.default
                }
            } else {
                target.modulator_slot as f32
            };
            let depth_default =
                if target.depth_param_idx < slot.num_params.load(Ordering::Relaxed) as usize {
                    slot.defaults.get(target.depth_param_idx)
                } else {
                    depth_desc.default
                };
            let source_current = target
                .source_param_idx
                .and_then(|source_param_idx| {
                    plock_step.and_then(|step| slot.plocks.get(step, source_param_idx))
                })
                .unwrap_or(source_default);
            let depth_current = plock_step
                .and_then(|step| slot.plocks.get(step, target.depth_param_idx))
                .unwrap_or(depth_default);
            let (depth_min, depth_max) = instrument_modulation_depth_display_range(target);
            Some((
                target.base_param_idx,
                UiModMetadata {
                    source_param_idx: target.source_param_idx,
                    depth_param_idx: target.depth_param_idx,
                    source_slot: target
                        .source_param_idx
                        .and_then(|source_param_idx| {
                            desc.params
                                .get(source_param_idx)
                                .map(|source_desc| source_desc.stored_to_user(source_current))
                        })
                        .unwrap_or(source_current),
                    source_value_field: target.source_param_idx.map(|source_param_idx| {
                        let source_desc = &desc.params[source_param_idx];
                        instrument_param_value_field(track, source_param_idx, &source_desc.name)
                    }),
                    depth_value: depth_desc.stored_to_user(depth_current),
                    depth_value_field: Some(instrument_param_value_field(
                        track,
                        target.depth_param_idx,
                        &depth_desc.name,
                    )),
                    depth_min,
                    depth_max,
                    depth_unit: target.depth_unit.clone(),
                },
            ))
        })
    {
        modulation_targets
            .entry(target.0)
            .or_default()
            .push(target.1);
    }
    push_param(
        &mut synth_params,
        "base_note".to_string(),
        "base-note",
        None,
        base_note_current,
        -48.0,
        48.0,
        None,
        Some(instrument_base_note_value_field(track)),
        None,
        None,
    );

    for (param_idx, pdesc) in desc.params.iter().enumerate() {
        let default_val = if param_idx < slot.num_params.load(Ordering::Relaxed) as usize {
            slot.defaults.get(param_idx)
        } else {
            pdesc.default
        };
        let current_val = plock_step
            .and_then(|step| slot.plocks.get(step, param_idx))
            .unwrap_or(default_val);
        let options = match &pdesc.kind {
            sequencer::effects::ParamKind::Enum { labels } => Some(labels),
            _ => None,
        };
        if is_source_param(pdesc.node_param_idx)
            || is_generated_host_mod_param(&pdesc.name)
            || is_hidden_dgen_mod_param(&pdesc.name)
        {
            continue;
        }
        if is_mod_param(&pdesc.name) {
            let mod_name = pdesc
                .name
                .strip_prefix("mod ")
                .unwrap_or(&pdesc.name)
                .to_string();
            push_param(
                &mut mod_params,
                mod_name,
                "param",
                Some(param_idx),
                pdesc.stored_to_user(current_val),
                pdesc.stored_to_user(pdesc.min),
                pdesc.stored_to_user(pdesc.max),
                options,
                Some(instrument_param_value_field(track, param_idx, &pdesc.name)),
                None,
                None,
            );
        } else {
            push_param(
                &mut synth_params,
                pdesc.name.clone(),
                "param",
                Some(param_idx),
                pdesc.stored_to_user(current_val),
                pdesc.stored_to_user(pdesc.min),
                pdesc.stored_to_user(pdesc.max),
                options,
                Some(instrument_param_value_field(track, param_idx, &pdesc.name)),
                modulation_targets.get(&param_idx),
                pdesc.ui_metadata.as_ref(),
            );
        }
    }

    let mut source_sections: Vec<Rc<RefCell<Value>>> = Vec::new();
    let mut source_names: Vec<Rc<RefCell<Value>>> = Vec::new();
    for slot_number in 1..=sequencer::voice_modulator::SLOT_COUNT {
        let section_name = sequencer::voice_modulator::modulator_slot_label(slot_number, "");
        let mut params: Vec<Rc<RefCell<Value>>> = Vec::new();
        let mut source_param: Option<Rc<RefCell<Value>>> = None;
        for &param_idx in &source_actual {
            let Some(pdesc) = desc.params.get(param_idx) else {
                continue;
            };
            if sequencer::voice_modulator::slot_from_param_name(&pdesc.name) != Some(slot_number) {
                continue;
            }
            let default_val = if param_idx < slot.num_params.load(Ordering::Relaxed) as usize {
                slot.defaults.get(param_idx)
            } else {
                pdesc.default
            };
            let current_val = plock_step
                .and_then(|step| slot.plocks.get(step, param_idx))
                .unwrap_or(default_val);
            let options = match &pdesc.kind {
                sequencer::effects::ParamKind::Enum { labels } => Some(labels),
                _ => None,
            };
            push_param(
                &mut params,
                rename_source_param(&pdesc.name),
                "param",
                Some(param_idx),
                pdesc.stored_to_user(current_val),
                pdesc.stored_to_user(pdesc.min),
                pdesc.stored_to_user(pdesc.max),
                options,
                Some(instrument_param_value_field(track, param_idx, &pdesc.name)),
                None,
                None,
            );
            if sequencer::voice_modulator::source_type_name_from_param_name(&pdesc.name)
                == Some("source")
            {
                source_param = params.pop();
            }
        }
        source_names.push(Rc::new(RefCell::new(Value::String(section_name.clone()))));
        let mut section_map: HashMap<String, Rc<RefCell<Value>>> = HashMap::new();
        section_map.insert(
            "name".to_string(),
            Rc::new(RefCell::new(Value::String(section_name))),
        );
        section_map.insert(
            "slot".to_string(),
            Rc::new(RefCell::new(Value::Number(slot_number as f64))),
        );
        if let Some(source_param) = source_param {
            section_map.insert("source-param".to_string(), source_param);
        }
        section_map.insert(
            "params".to_string(),
            Rc::new(RefCell::new(Value::List(params))),
        );
        source_sections.push(Rc::new(RefCell::new(Value::Map(section_map))));
    }

    let mut panel_map: HashMap<String, Rc<RefCell<Value>>> = HashMap::new();
    let instrument_type = app
        .graph
        .track_instrument_types
        .get(track)
        .copied()
        .unwrap_or(sequencer::sequencer::InstrumentType::Custom);
    let instrument_name = current_custom_instrument_name(app, track).unwrap_or_else(|| {
        if instrument_type == sequencer::sequencer::InstrumentType::Modulator {
            "Modulator".to_string()
        } else {
            "Instrument".to_string()
        }
    });
    let instrument_type_name = match instrument_type {
        sequencer::sequencer::InstrumentType::Sampler => "sampler",
        sequencer::sequencer::InstrumentType::Custom => "custom",
        sequencer::sequencer::InstrumentType::Modulator => "modulator",
    };
    panel_map.insert(
        "type".to_string(),
        Rc::new(RefCell::new(Value::String(
            instrument_type_name.to_string(),
        ))),
    );
    panel_map.insert(
        "phase-field".to_string(),
        Rc::new(RefCell::new(Value::String(modulator_phase_field(track)))),
    );
    panel_map.insert(
        "level-field".to_string(),
        Rc::new(RefCell::new(Value::String(modulator_level_field(track)))),
    );
    panel_map.insert(
        "name".to_string(),
        Rc::new(RefCell::new(Value::String(instrument_name.clone()))),
    );
    panel_map.insert(
        "display-name".to_string(),
        Rc::new(RefCell::new(Value::String(instrument_display_name(
            &instrument_name,
        )))),
    );
    panel_map.insert(
        "synth".to_string(),
        Rc::new(RefCell::new(Value::List(synth_params))),
    );
    panel_map.insert(
        "mod".to_string(),
        Rc::new(RefCell::new(Value::List(mod_params))),
    );
    panel_map.insert(
        "modulators".to_string(),
        Rc::new(RefCell::new(Value::List(
            desc.instrument_modulators
                .iter()
                .map(|modulator| {
                    let mut map: HashMap<String, Rc<RefCell<Value>>> = HashMap::new();
                    map.insert(
                        "slot".to_string(),
                        Rc::new(RefCell::new(Value::Number(modulator.slot as f64))),
                    );
                    map.insert(
                        "label".to_string(),
                        Rc::new(RefCell::new(Value::String(modulator.label.clone()))),
                    );
                    Rc::new(RefCell::new(Value::Map(map)))
                })
                .collect(),
        ))),
    );
    panel_map.insert(
        "source-names".to_string(),
        Rc::new(RefCell::new(Value::List(source_names))),
    );
    panel_map.insert(
        "sources".to_string(),
        Rc::new(RefCell::new(Value::List(source_sections))),
    );

    Value::List(vec![Rc::new(RefCell::new(Value::Map(panel_map)))])
}

/// Build a Lisp Value::List of bools indicating which steps are selected.
pub(crate) fn build_selection_value(selected: &Arc<Mutex<HashSet<usize>>>) -> Value {
    let set = selected.lock().unwrap();
    build_selection_value_from_set(&set)
}

/// Build a Lisp Value::List of bools from an already-held selection snapshot.
pub(crate) fn build_selection_value_from_set(set: &HashSet<usize>) -> Value {
    let items: Vec<Rc<RefCell<Value>>> = (0..MAX_STEPS)
        .map(|s| Rc::new(RefCell::new(Value::Bool(set.contains(&s)))))
        .collect();
    Value::List(items)
}

/// Build list of available effect names from the effects/ directory.
pub(crate) fn build_available_effects() -> Value {
    let names = sequencer::lisp_host::list_saved_effects();
    let items: Vec<Rc<RefCell<Value>>> = names
        .into_iter()
        .map(|n| Rc::new(RefCell::new(Value::String(n))))
        .collect();
    Value::List(items)
}

pub(crate) fn build_available_builtin_effects() -> Value {
    let mut items: Vec<Rc<RefCell<Value>>> =
        sequencer::effects::EffectDescriptor::builtin_insert_names()
            .iter()
            .map(|name| Rc::new(RefCell::new(Value::String((*name).to_string()))))
            .collect();
    // dgenlisp-backed builtins (added through the builtin path, DSP is dgenlisp)
    items.push(Rc::new(RefCell::new(Value::String(
        sequencer::conv_reverb::NAME.to_string(),
    ))));
    Value::List(items)
}

pub(crate) fn build_available_midi_effects() -> Value {
    let mut names: Vec<String> = sequencer::lisp_host::load_midi_fx_descriptors()
        .into_iter()
        .map(|desc| desc.name)
        .collect();
    names.sort();
    let items: Vec<Rc<RefCell<Value>>> = names
        .into_iter()
        .map(|name| Rc::new(RefCell::new(Value::String(name))))
        .collect();
    Value::List(items)
}

pub(crate) fn midi_fx_option_index(fx_name: &str, param_idx: usize, label: &str) -> Option<usize> {
    sequencer::lisp_host::load_midi_fx_descriptor(fx_name)
        .and_then(|desc| desc.params.get(param_idx).cloned())
        .and_then(|param| match param.kind {
            sequencer::effects::ParamKind::Enum { labels } => {
                labels.iter().position(|item| item == label)
            }
            _ => None,
        })
}

const METER_FLOOR_DBFS: f32 = -60.0;

pub(crate) fn master_meter_level(peak: f32) -> f64 {
    if peak <= 0.0 || !peak.is_finite() {
        0.0
    } else {
        let db = 20.0 * peak.log10();
        ((db - METER_FLOOR_DBFS) / -METER_FLOOR_DBFS).clamp(0.0, 1.0) as f64
    }
}

pub(crate) fn quantize_meter_level(level: f64) -> f64 {
    ((level.clamp(0.0, 1.0) * METER_LEVEL_STEPS).round()) / METER_LEVEL_STEPS
}

pub(crate) fn meter_display_level(peak: f32) -> f64 {
    quantize_meter_level(master_meter_level(peak))
}

pub(crate) fn sync_project_state(rt: &mut Runtime, app: &ui::App) {
    rt.set_reactive(
        "SEQ",
        "current-project-name",
        Value::String(app.current_project_name.clone().unwrap_or_default()),
    );
}

pub(crate) const PROJECT_SCRATCH_BUFFER_NAME: &str = "*scratch*";

fn project_scratch_source_path() -> PathBuf {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    manifest_dir
        .parent()
        .and_then(|crates_dir| crates_dir.parent())
        .unwrap_or(&manifest_dir)
        .join(".eseqlisp-scratch")
}

pub(crate) fn push_project_scratch_to_named_buffer(editor: &mut Editor, app: &ui::App) {
    let scratch_text = app.editor.scratch_buffer.clone();
    let scratch_cursor = app.editor.scratch_cursor;

    let id = editor.upsert_scratch_buffer(PROJECT_SCRATCH_BUFFER_NAME, &scratch_text);
    let scratch_path = project_scratch_source_path();
    if let Some(buffer) = editor.buffers.iter_mut().find(|buffer| buffer.id == id) {
        buffer.path = Some(scratch_path);
    }

    if editor.active_buffer().name == PROJECT_SCRATCH_BUFFER_NAME {
        let buffer = editor.active_buffer_mut();
        let row = scratch_cursor.0.min(buffer.lines.len().saturating_sub(1));
        let col = scratch_cursor.1.min(buffer.lines[row].len());
        buffer.cursor = (row, col);
    }
}

pub(crate) fn evaluate_project_scratch_on_ui_runtime(
    editor: &mut Editor,
    app: &ui::App,
) -> Result<(), String> {
    let scratch_text = app.editor.scratch_buffer.clone();
    if scratch_text.trim().is_empty() {
        return Ok(());
    }

    let overlays = editor.snapshot_file_backed_sources();
    let report = editor.runtime_mut().eval_source_transactional(
        Some(project_scratch_source_path()),
        &scratch_text,
        overlays,
    );
    let result = if report.success {
        Ok(())
    } else {
        Err(report.failure_message())
    };
    editor.process_lisp_reload_report(report);
    if let Some(status) = editor.runtime_mut().take_status_message() {
        editor.show_transient_message(status);
    }
    result
}

pub(crate) fn pull_named_scratch_buffer_into_project(editor: &Editor, app: &mut ui::App) {
    let buffer = editor.active_buffer();
    if buffer.name != PROJECT_SCRATCH_BUFFER_NAME {
        return;
    }

    let text = buffer.text();
    let cursor = buffer.cursor;
    if app.editor.scratch_buffer != text || app.editor.scratch_cursor != cursor {
        app.editor.scratch_buffer = text.clone();
        app.editor.scratch_cursor = cursor;
        app.state.set_scratch_source(text);
        app.editor.scratch_runtime = None;
    }
}

pub(crate) fn current_custom_instrument_name(app: &ui::App, track: usize) -> Option<String> {
    if app.tracks.is_empty() || app.is_sampler_track(track) {
        None
    } else if let Some(Some(engine_id)) = app.graph.track_engine_ids.get(track) {
        app.editor
            .engine_registry
            .get(*engine_id)
            .map(|engine| engine.name.clone())
    } else {
        app.tracks.get(track).cloned()
    }
}

pub(crate) fn sync_sidebar_browser(rt: &mut Runtime, app: &ui::App, track: usize) {
    if app.is_sampler_track(track) {
        let selected_sample = app
            .sampler_path_for_track(track)
            .map(|path| path.to_string_lossy().to_string())
            .unwrap_or_default();
        rt.set_reactive("SEQ", "sidebar-kind", Value::String("sampler".to_string()));
        rt.set_reactive(
            "SEQ",
            "sidebar-instrument-name",
            Value::String(String::new()),
        );
        rt.set_reactive(
            "SEQ",
            "sidebar-instrument-display-name",
            Value::String(String::new()),
        );
        rt.set_reactive("SEQ", "sidebar-loaded-preset", Value::String(String::new()));
        rt.set_reactive("SEQ", "sidebar-track-index", Value::Number(track as f64));
        rt.set_reactive(
            "SEQ",
            "sidebar-selected-sample",
            Value::String(selected_sample),
        );
        rt.set_reactive("SEQ", "sidebar-presets", Value::List(vec![]));
        rt.set_reactive("SEQ", "sidebar-preset-tree", Value::List(vec![]));
        return;
    }

    let instrument_name = current_custom_instrument_name(app, track).unwrap_or_default();
    let loaded_preset = app
        .state
        .pattern
        .track_sound_state
        .lock()
        .unwrap()
        .get(track)
        .and_then(|meta| meta.loaded_preset.clone())
        .unwrap_or_default();
    let preset_items = visible_preset_items_for_track(app, track);

    rt.set_reactive(
        "SEQ",
        "sidebar-kind",
        Value::String("instrument".to_string()),
    );
    rt.set_reactive(
        "SEQ",
        "sidebar-instrument-name",
        Value::String(instrument_name.clone()),
    );
    rt.set_reactive(
        "SEQ",
        "sidebar-instrument-display-name",
        Value::String(instrument_display_name(&instrument_name)),
    );
    rt.set_reactive(
        "SEQ",
        "sidebar-loaded-preset",
        Value::String(loaded_preset.clone()),
    );
    rt.set_reactive("SEQ", "sidebar-track-index", Value::Number(track as f64));
    rt.set_reactive(
        "SEQ",
        "sidebar-selected-sample",
        Value::String(String::new()),
    );
    rt.set_reactive("SEQ", "sidebar-presets", build_string_list(&preset_items));
    rt.set_reactive(
        "SEQ",
        "sidebar-preset-tree",
        build_flat_tree_items(&preset_items),
    );
}

pub(crate) fn load_instrument_preset_into_track(
    app: &mut ui::App,
    track: usize,
    preset_name: &str,
) -> Result<(), String> {
    let instrument_name = current_custom_instrument_name(app, track)
        .ok_or_else(|| "Current track is not a custom instrument".to_string())?;
    let presets = sequencer::lisp_host::load_instrument_presets(&instrument_name)
        .map_err(|e| e.to_string())?;
    let preset = presets
        .into_iter()
        .find(|preset| preset.name == preset_name)
        .ok_or_else(|| format!("Preset '{preset_name}' not found"))?;
    let desc = app
        .graph
        .instrument_descriptors
        .get(track)
        .cloned()
        .ok_or_else(|| "Instrument descriptor unavailable".to_string())?;

    {
        let slot = &app.state.pattern.instrument_slots[track];
        for (param_idx, param) in desc.params.iter().enumerate() {
            let value = preset
                .params
                .get(&param.name)
                .copied()
                .unwrap_or(param.default);
            let clamped = param.clamp(value);
            slot.defaults.set(param_idx, clamped);
            app.send_instrument_param(track, param_idx, clamped);
        }
    }

    app.state.pattern.instrument_base_note_offsets[track]
        .store(preset.base_note_offset.to_bits(), Ordering::Relaxed);
    app.state.schedule_mod_resync();
    app.state.publish_scheduler_snapshot();
    let engine_id = app.graph.track_engine_ids.get(track).and_then(|id| *id);
    if let Some(meta) = app
        .state
        .pattern
        .track_sound_state
        .lock()
        .unwrap()
        .get_mut(track)
    {
        meta.engine_id = engine_id;
        meta.loaded_preset = Some(preset.name.clone());
        meta.dirty = false;
    }
    Ok(())
}

/// Extract the :path string from a host-command payload dict.
pub(crate) fn extract_path_from_payload(payload: &Value) -> Option<String> {
    extract_string_from_payload(payload, "path")
}

pub(crate) fn extract_string_from_payload(payload: &Value, key: &str) -> Option<String> {
    if let Value::Map(map) = payload {
        if let Some(cell) = map.get(key) {
            if let Value::String(s) | Value::Keyword(s) | Value::Symbol(s) = &*cell.borrow() {
                return Some(s.clone());
            }
        }
    }
    None
}

pub(crate) fn extract_usize_from_payload(payload: &Value, key: &str) -> Option<usize> {
    if let Value::Map(map) = payload {
        if let Some(cell) = map.get(key) {
            if let Value::Number(n) = &*cell.borrow() {
                return (*n >= 0.0).then_some(*n as usize);
            }
        }
    }
    None
}

pub(crate) fn extract_bool_from_payload(payload: &Value, key: &str) -> bool {
    if let Value::Map(map) = payload {
        if let Some(cell) = map.get(key) {
            return matches!(&*cell.borrow(), Value::Bool(true));
        }
    }
    false
}

/// Push individual tp-* reactive fields for the current track.
pub(crate) fn sync_track_params(
    rt: &mut Runtime,
    app: &ui::App,
    state: &Arc<SequencerState>,
    track: usize,
    selected: &Arc<Mutex<HashSet<usize>>>,
) {
    let tp = &state.pattern.track_params[track];
    let selected_step = selected_plock_step(selected);
    let display_step = displayed_plock_step(state, track, selected_step);
    rt.set_reactive("SEQ", "tp-attack", Value::Number(tp.get_attack_ms() as f64));
    rt.set_reactive(
        "SEQ",
        "tp-release",
        Value::Number(tp.get_release_ms() as f64),
    );
    let swing = display_step
        .and_then(|step| state.pattern.swing_plocks[track].get(step))
        .unwrap_or_else(|| tp.get_swing());
    rt.set_reactive("SEQ", "tp-swing", Value::Number(swing as f64));
    rt.set_reactive("SEQ", "tp-send", Value::Number(tp.get_send() as f64));
    rt.set_reactive("SEQ", "tp-output", build_track_output_label(app, tp));
    rt.set_reactive(
        "SEQ",
        "track-output-options",
        build_track_output_options(app),
    );
    rt.set_reactive("SEQ", "tp-bus-sends", build_track_bus_sends(app, tp));
    sync_current_track_bus_send_binding_fields(rt, app, state, track);
    rt.set_reactive(
        "SEQ",
        "tp-num-steps",
        Value::Number(tp.get_num_steps() as f64),
    );
    rt.set_reactive("SEQ", "tp-gate", Value::Bool(tp.is_gate_on()));
    rt.set_reactive("SEQ", "tp-poly", Value::Bool(tp.is_polyphonic()));
    rt.set_reactive(
        "SEQ",
        "tp-max-polyphony",
        Value::Number(tp.get_max_polyphony() as f64),
    );
    // Resolve timebase through the same display overlay used by parameter controls.
    let timebase_label = display_step
        .and_then(|step| state.pattern.timebase_plocks[track].get(step))
        .unwrap_or_else(|| tp.get_timebase())
        .label()
        .to_string();
    rt.set_reactive("SEQ", "tp-timebase", Value::String(timebase_label));
    let swing_resolution = display_step
        .and_then(|step| state.pattern.swing_resolution_plocks[track].get(step))
        .unwrap_or_else(|| tp.get_swing_resolution());
    rt.set_reactive(
        "SEQ",
        "tp-swing-resolution",
        Value::String(swing_resolution.label().to_string()),
    );
    rt.set_reactive(
        "SEQ",
        "tp-fts",
        Value::String(
            FTS_SCALE_NAMES
                .get(tp.get_fts_scale())
                .copied()
                .unwrap_or("Off")
                .to_string(),
        ),
    );
    rt.set_reactive(
        "SEQ",
        "tp-accumulator",
        Value::String(selected_accumulator_name(app, track)),
    );
    rt.set_reactive(
        "SEQ",
        "tp-accum-limit",
        Value::Number(tp.get_accum_limit() as f64),
    );
    rt.set_reactive(
        "SEQ",
        "tp-accum-mode",
        Value::String(accum_mode_label(tp.get_accum_mode()).to_string()),
    );
    rt.set_reactive("SEQ", "accumulator-options", build_accumulator_options(app));
    rt.set_reactive("SEQ", "fts-options", build_fts_options());
    rt.set_reactive("SEQ", "accum-mode-options", build_accum_mode_options());
    rt.set_reactive(
        "SEQ",
        "track-plocks",
        build_track_plocks_value(app, state, track, selected),
    );
}

pub(crate) fn sync_track_params_with_neural_selection(
    rt: &mut Runtime,
    app: &ui::App,
    state: &Arc<SequencerState>,
    track: usize,
    selected: &Arc<Mutex<HashSet<usize>>>,
    selected_neural_neurons: Option<
        &std::collections::BTreeSet<sequencer::lisp_host::SelectedNeuralNeuron>,
    >,
) {
    sync_track_params(rt, app, state, track, selected);
    rt.set_reactive(
        "SEQ",
        "track-plocks",
        build_track_plocks_value_with_neural_selection(
            app,
            state,
            track,
            selected,
            selected_neural_neurons,
        ),
    );
}

fn plock_entry(
    step: usize,
    target: &str,
    group: &str,
    name: &str,
    value: f32,
    min: f32,
    max: f32,
    slot_idx: Option<usize>,
    param_idx: Option<usize>,
    options: Option<Vec<String>>,
) -> Rc<RefCell<Value>> {
    plock_entry_with_label(
        &format!("S{}", step + 1),
        step,
        target,
        group,
        name,
        value,
        min,
        max,
        slot_idx,
        param_idx,
        options,
        None,
        None,
        None,
    )
}

fn plock_entry_with_label(
    label: &str,
    step: usize,
    target: &str,
    group: &str,
    name: &str,
    value: f32,
    min: f32,
    max: f32,
    slot_idx: Option<usize>,
    param_idx: Option<usize>,
    options: Option<Vec<String>>,
    source: Option<&str>,
    target_track: Option<usize>,
    network_id: Option<u64>,
) -> Rc<RefCell<Value>> {
    use std::collections::HashMap;

    let mut map: HashMap<String, Rc<RefCell<Value>>> = HashMap::new();
    map.insert(
        "label".to_string(),
        Rc::new(RefCell::new(Value::String(label.to_string()))),
    );
    map.insert(
        "step".to_string(),
        Rc::new(RefCell::new(Value::Number((step + 1) as f64))),
    );
    map.insert(
        "step-idx".to_string(),
        Rc::new(RefCell::new(Value::Number(step as f64))),
    );
    map.insert(
        "target".to_string(),
        Rc::new(RefCell::new(Value::String(target.to_string()))),
    );
    map.insert(
        "group".to_string(),
        Rc::new(RefCell::new(Value::String(group.to_string()))),
    );
    map.insert(
        "name".to_string(),
        Rc::new(RefCell::new(Value::String(name.to_string()))),
    );
    map.insert(
        "value".to_string(),
        Rc::new(RefCell::new(Value::Number(value as f64))),
    );
    map.insert(
        "min".to_string(),
        Rc::new(RefCell::new(Value::Number(min as f64))),
    );
    map.insert(
        "max".to_string(),
        Rc::new(RefCell::new(Value::Number(max as f64))),
    );
    if let Some(slot_idx) = slot_idx {
        map.insert(
            "slot-idx".to_string(),
            Rc::new(RefCell::new(Value::Number(slot_idx as f64))),
        );
    }
    if let Some(param_idx) = param_idx {
        map.insert(
            "param-idx".to_string(),
            Rc::new(RefCell::new(Value::Number(param_idx as f64))),
        );
    }
    if let Some(source) = source {
        map.insert(
            "source".to_string(),
            Rc::new(RefCell::new(Value::String(source.to_string()))),
        );
        if source == "neuron" {
            map.insert(
                "neuron-idx".to_string(),
                Rc::new(RefCell::new(Value::Number(step as f64))),
            );
        }
    }
    if let Some(target_track) = target_track {
        map.insert(
            "target-track".to_string(),
            Rc::new(RefCell::new(Value::Number(target_track as f64))),
        );
    }
    if let Some(network_id) = network_id {
        map.insert(
            "network-id".to_string(),
            Rc::new(RefCell::new(Value::Number(network_id as f64))),
        );
    }
    if let Some(options) = options {
        let selected = options
            .get(value.round().max(0.0) as usize)
            .cloned()
            .unwrap_or_default();
        map.insert(
            "text-value".to_string(),
            Rc::new(RefCell::new(Value::String(selected))),
        );
        map.insert(
            "options".to_string(),
            Rc::new(RefCell::new(Value::List(
                options
                    .into_iter()
                    .map(|label| Rc::new(RefCell::new(Value::String(label))))
                    .collect(),
            ))),
        );
    }
    Rc::new(RefCell::new(Value::Map(map)))
}

pub(crate) fn build_track_plocks_value_with_neural_selection(
    app: &ui::App,
    state: &Arc<SequencerState>,
    track: usize,
    selected: &Arc<Mutex<HashSet<usize>>>,
    selected_neural_neurons: Option<
        &std::collections::BTreeSet<sequencer::lisp_host::SelectedNeuralNeuron>,
    >,
) -> Value {
    let Some(selection) = selected_neural_neurons else {
        return build_track_plocks_value(app, state, track, selected);
    };
    let current_pattern = state.current_scene_index();
    if !selection
        .iter()
        .any(|selected| selected.pattern_idx == current_pattern)
    {
        return build_track_plocks_value(app, state, track, selected);
    }
    build_selected_neural_plocks_value(app, state, selection)
}

fn build_selected_neural_plocks_value(
    app: &ui::App,
    state: &Arc<SequencerState>,
    selection: &std::collections::BTreeSet<sequencer::lisp_host::SelectedNeuralNeuron>,
) -> Value {
    use sequencer::effects::ParamKind;

    let current_pattern = state.current_scene_index();
    let networks = state.current_neural_networks();
    let mut items = Vec::new();

    for selected in selection
        .iter()
        .filter(|selected| selected.pattern_idx == current_pattern)
    {
        let Some(network) = networks
            .iter()
            .find(|network| network.id == selected.network_id)
        else {
            continue;
        };
        let Some(neuron) = network.neurons.get(selected.neuron_idx) else {
            continue;
        };
        let label = format!("N{}", selected.neuron_idx + 1);

        for override_param in &neuron.output_overrides.instrument {
            let target_track = override_param.target_track;
            let Some(desc) = app.graph.instrument_descriptors.get(target_track) else {
                continue;
            };
            let Some(param) = desc.params.get(override_param.param_index) else {
                continue;
            };
            let Some(slot) = state.pattern.instrument_slots.get(target_track) else {
                continue;
            };
            if slot.param_node_id(override_param.param_index) != Some(override_param.param_id) {
                continue;
            }
            let options = match &param.kind {
                ParamKind::Enum { labels } => Some(labels.clone()),
                ParamKind::Boolean => Some(vec!["off".to_string(), "on".to_string()]),
                ParamKind::Continuous { .. } => None,
            };
            items.push(plock_entry_with_label(
                &label,
                selected.neuron_idx,
                "neural-instrument",
                &format!("T{} inst", target_track + 1),
                &param.name,
                param.stored_to_user(override_param.value),
                param.stored_to_user(param.min),
                param.stored_to_user(param.max),
                None,
                Some(override_param.param_index),
                options,
                Some("neuron"),
                Some(target_track),
                Some(network.id),
            ));
        }

        for override_param in &neuron.output_overrides.effects {
            let target_track = override_param.target_track;
            let Some(desc) = app
                .graph
                .effect_descriptors
                .get(target_track)
                .and_then(|descs| descs.get(override_param.slot_index))
            else {
                continue;
            };
            let Some(param) = desc.params.get(override_param.param_index) else {
                continue;
            };
            let Some(slot) = state
                .pattern
                .effect_chains
                .get(target_track)
                .and_then(|chain| chain.get(override_param.slot_index))
            else {
                continue;
            };
            if slot.param_node_id(override_param.param_index) != Some(override_param.param_id) {
                continue;
            }
            let options = match &param.kind {
                ParamKind::Enum { labels } => Some(labels.clone()),
                ParamKind::Boolean => Some(vec!["off".to_string(), "on".to_string()]),
                ParamKind::Continuous { .. } => None,
            };
            items.push(plock_entry_with_label(
                &label,
                selected.neuron_idx,
                "neural-effect",
                &format!("T{} {}", target_track + 1, desc.name),
                &param.name,
                override_param.value,
                param.min,
                param.max,
                Some(override_param.slot_index),
                Some(override_param.param_index),
                options,
                Some("neuron"),
                Some(target_track),
                Some(network.id),
            ));
        }
    }

    Value::List(items)
}

pub(crate) fn build_track_plocks_value(
    app: &ui::App,
    state: &Arc<SequencerState>,
    track: usize,
    selected: &Arc<Mutex<HashSet<usize>>>,
) -> Value {
    use sequencer::effects::ParamKind;

    let mut steps: Vec<usize> = selected.lock().unwrap().iter().copied().collect();
    steps.sort_unstable();
    let mut items = Vec::new();
    if steps.is_empty() {
        return Value::List(items);
    }

    let tp = &state.pattern.track_params[track];
    for step in steps {
        if let Some(timebase) = state.pattern.timebase_plocks[track].get(step) {
            items.push(plock_entry(
                step,
                "timebase",
                "track",
                "timebase",
                timebase as u32 as f32,
                0.0,
                (Timebase::ALL.len() - 1) as f32,
                None,
                None,
                Some(
                    Timebase::LABELS
                        .iter()
                        .map(|label| label.to_string())
                        .collect(),
                ),
            ));
        }
        if let Some(swing) = state.pattern.swing_plocks[track].get(step) {
            items.push(plock_entry(
                step, "swing", "track", "swing", swing, 50.0, 75.0, None, None, None,
            ));
        }
        if let Some(resolution) = state.pattern.swing_resolution_plocks[track].get(step) {
            items.push(plock_entry(
                step,
                "swing-resolution",
                "track",
                "swing res",
                resolution as u32 as f32,
                0.0,
                (SwingResolution::ALL.len() - 1) as f32,
                None,
                None,
                Some(
                    SwingResolution::LABELS
                        .iter()
                        .map(|label| label.to_string())
                        .collect(),
                ),
            ));
        }

        if let Some(desc) = app.graph.instrument_descriptors.get(track) {
            let slot = &state.pattern.instrument_slots[track];
            for (param_idx, param) in desc.params.iter().enumerate() {
                if let Some(value) = slot.plocks.get(step, param_idx) {
                    let options = match &param.kind {
                        ParamKind::Enum { labels } => Some(labels.clone()),
                        ParamKind::Boolean => Some(vec!["off".to_string(), "on".to_string()]),
                        ParamKind::Continuous { .. } => None,
                    };
                    items.push(plock_entry(
                        step,
                        "instrument",
                        "inst",
                        &param.name,
                        param.stored_to_user(value),
                        param.stored_to_user(param.min),
                        param.stored_to_user(param.max),
                        None,
                        Some(param_idx),
                        options,
                    ));
                }
            }
        }

        if let Some(descs) = app.graph.effect_descriptors.get(track) {
            for (slot_idx, desc) in descs.iter().enumerate() {
                let Some(slot) = state.pattern.effect_chains[track].get(slot_idx) else {
                    continue;
                };
                for (param_idx, param) in desc.params.iter().enumerate() {
                    if let Some(value) = slot.plocks.get(step, param_idx) {
                        let options = match &param.kind {
                            ParamKind::Enum { labels } => Some(labels.clone()),
                            ParamKind::Boolean => Some(vec!["off".to_string(), "on".to_string()]),
                            ParamKind::Continuous { .. } => None,
                        };
                        items.push(plock_entry(
                            step,
                            "effect",
                            &desc.name,
                            &param.name,
                            value,
                            param.min,
                            param.max,
                            Some(slot_idx),
                            Some(param_idx),
                            options,
                        ));
                    }
                }
            }
        }

        let midi_chain = tp.midi_fx_chain();
        for (slot_idx, slot) in state.pattern.midi_fx_slots[track].iter().enumerate() {
            let Some(desc) = midi_chain
                .get(slot_idx)
                .and_then(|name| sequencer::lisp_host::load_midi_fx_descriptor(name))
            else {
                continue;
            };
            for (param_idx, param) in desc.params.iter().enumerate() {
                if let Some(value) = slot.plocks.get(step, param_idx) {
                    let options = match &param.kind {
                        ParamKind::Enum { labels } => Some(labels.clone()),
                        ParamKind::Boolean => Some(vec!["off".to_string(), "on".to_string()]),
                        ParamKind::Continuous { .. } => None,
                    };
                    items.push(plock_entry(
                        step,
                        "midi-fx",
                        &desc.name,
                        &param.name,
                        value,
                        param.min,
                        param.max,
                        Some(slot_idx),
                        Some(param_idx),
                        options,
                    ));
                }
            }
        }
    }

    Value::List(items)
}

fn build_track_output_label(app: &ui::App, tp: &sequencer::sequencer::TrackParams) -> Value {
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
}

fn build_track_output_options(app: &ui::App) -> Value {
    let mut labels = vec![
        Rc::new(RefCell::new(Value::String("main".to_string()))),
        Rc::new(RefCell::new(Value::String("sends only".to_string()))),
    ];
    labels.extend(
        app.buses
            .iter()
            .filter(|bus| bus.id != sequencer::sequencer::BusId::MIX)
            .map(|bus| Rc::new(RefCell::new(Value::String(bus.name.clone())))),
    );
    Value::List(labels)
}

fn build_track_bus_sends(app: &ui::App, _tp: &sequencer::sequencer::TrackParams) -> Value {
    use std::collections::HashMap;

    let items = app
        .buses
        .iter()
        .enumerate()
        .filter(|(_, bus)| bus.id != sequencer::sequencer::BusId::MIX)
        .map(|(bus_idx, bus)| {
            let mut map = HashMap::new();
            map.insert(
                "bus-idx".to_string(),
                Rc::new(RefCell::new(Value::Number(bus_idx as f64))),
            );
            map.insert(
                "name".to_string(),
                Rc::new(RefCell::new(Value::String(bus.name.clone()))),
            );
            Rc::new(RefCell::new(Value::Map(map)))
        })
        .collect();
    Value::List(items)
}

/// Build a Lisp Value::Map of track parameters for the current track.
pub(crate) fn build_track_params(state: &Arc<SequencerState>, track: usize) -> Value {
    use std::collections::HashMap;
    let tp = &state.pattern.track_params[track];
    let mut map: HashMap<String, Rc<RefCell<Value>>> = HashMap::new();
    map.insert(
        "gate".into(),
        Rc::new(RefCell::new(Value::Bool(tp.is_gate_on()))),
    );
    map.insert(
        "attack".into(),
        Rc::new(RefCell::new(Value::Number(tp.get_attack_ms() as f64))),
    );
    map.insert(
        "release".into(),
        Rc::new(RefCell::new(Value::Number(tp.get_release_ms() as f64))),
    );
    map.insert(
        "swing".into(),
        Rc::new(RefCell::new(Value::Number(tp.get_swing() as f64))),
    );
    map.insert(
        "swing-resolution".into(),
        Rc::new(RefCell::new(Value::String(
            tp.get_swing_resolution().label().to_string(),
        ))),
    );
    map.insert(
        "num-steps".into(),
        Rc::new(RefCell::new(Value::Number(tp.get_num_steps() as f64))),
    );
    map.insert(
        "volume".into(),
        Rc::new(RefCell::new(Value::Number(tp.get_volume() as f64))),
    );
    map.insert(
        "pan".into(),
        Rc::new(RefCell::new(Value::Number(tp.get_pan() as f64))),
    );
    map.insert(
        "mute".into(),
        Rc::new(RefCell::new(Value::Bool(tp.is_muted()))),
    );
    map.insert(
        "solo".into(),
        Rc::new(RefCell::new(Value::Bool(tp.is_solo()))),
    );
    map.insert(
        "timebase".into(),
        Rc::new(RefCell::new(Value::String(
            tp.get_timebase().label().to_string(),
        ))),
    );
    map.insert(
        "send".into(),
        Rc::new(RefCell::new(Value::Number(tp.get_send() as f64))),
    );
    map.insert(
        "poly".into(),
        Rc::new(RefCell::new(Value::Bool(tp.is_polyphonic()))),
    );
    map.insert(
        "max-polyphony".into(),
        Rc::new(RefCell::new(Value::Number(tp.get_max_polyphony() as f64))),
    );
    Value::Map(map)
}

/// Build a Lisp Value::List of bools indicating which steps have any p-locks on the given track.
pub(crate) fn build_step_has_plocks(
    state: &Arc<SequencerState>,
    track: usize,
    descriptors: &[Vec<sequencer::effects::EffectDescriptor>],
) -> Value {
    let items: Vec<Rc<RefCell<Value>>> = (0..MAX_STEPS)
        .map(|step| {
            Rc::new(RefCell::new(Value::Bool(track_step_has_plock(
                state,
                track,
                descriptors,
                step,
            ))))
        })
        .collect();
    Value::List(items)
}

pub(crate) fn track_step_has_plock(
    state: &Arc<SequencerState>,
    track: usize,
    descriptors: &[Vec<sequencer::effects::EffectDescriptor>],
    step: usize,
) -> bool {
    let chain = &state.pattern.effect_chains[track];
    let midi_fx_slots = &state.pattern.midi_fx_slots[track];
    let num_slots = descriptors.get(track).map(|d| d.len()).unwrap_or(0);
    let instrument_slot = &state.pattern.instrument_slots[track];
    let instrument_num_params = instrument_slot.num_params.load(Ordering::Relaxed) as usize;
    let timebase_plocks = &state.pattern.timebase_plocks[track];
    let swing_plocks = &state.pattern.swing_plocks[track];
    let swing_resolution_plocks = &state.pattern.swing_resolution_plocks[track];
    let effect_has_plock = (0..num_slots).any(|slot_idx| {
        let Some(slot) = chain.get(slot_idx) else {
            return false;
        };
        let np = slot.num_params.load(Ordering::Relaxed) as usize;
        (0..np).any(|p| slot.plocks.get(step, p).is_some())
    });
    let instrument_has_plock =
        (0..instrument_num_params).any(|p| instrument_slot.plocks.get(step, p).is_some());
    let midi_fx_has_plock = midi_fx_slots.iter().any(|slot| {
        let np = slot.num_params.load(Ordering::Relaxed) as usize;
        (0..np).any(|p| slot.plocks.get(step, p).is_some())
    });

    effect_has_plock
        || midi_fx_has_plock
        || instrument_has_plock
        || timebase_plocks.has_plock(step)
        || swing_plocks.has_plock(step)
        || swing_resolution_plocks.has_plock(step)
}

#[cfg(test)]
mod tests {
    use super::*;
    use eseqlisp::parser::{ASTParser, Expression, Parser, ParserError, Token};
    use sequencer::sequencer::default_empty_effect_chain;
    use std::collections::HashMap;

    fn agent_test_string_list(values: &[&str]) -> Value {
        Value::List(
            values
                .iter()
                .map(|value| Rc::new(RefCell::new(Value::String((*value).to_string()))))
                .collect(),
        )
    }

    fn register_agent_test_natives(runtime: &mut Runtime) {
        runtime.register_native("agent/new", |_args, _ctx| Ok(Value::Number(1.0)));
        runtime.register_native("agent/send", |_args, _ctx| Ok(Value::Nil));
        runtime.register_native("agent/cancel", |_args, _ctx| Ok(Value::Nil));
        runtime.register_native("agent/discard", |_args, _ctx| Ok(Value::Nil));
        runtime.register_native("agent/finalize", |_args, _ctx| Ok(Value::Nil));
        runtime.register_native("agent/status", |_args, _ctx| {
            Ok(Value::Symbol("idle".to_string()))
        });
        runtime.register_native("agent/model", |_args, _ctx| {
            Ok(Value::String("gpt-5.5".to_string()))
        });
        runtime.register_native("agent/set-model", |_args, _ctx| Ok(Value::Nil));
        runtime.register_native("agent/models", |_args, _ctx| {
            Ok(agent_test_string_list(&["gpt-5.5", "gemini-2.5-pro"]))
        });
        runtime.register_native("agent/messages", |_args, _ctx| Ok(Value::List(vec![])));
        runtime.register_native("agent/draft-source", |_args, _ctx| Ok(Value::Nil));
        runtime.register_native("agent/artifact", |_args, _ctx| {
            let mut map = std::collections::HashMap::new();
            map.insert(
                "exists".to_string(),
                Rc::new(RefCell::new(Value::Bool(false))),
            );
            Ok(Value::Map(map))
        });
    }

    #[test]
    fn meter_level_maps_linear_audio_peak_to_dbfs_display_scale() {
        let epsilon = 0.0001;
        assert!((master_meter_level(1.0) - 1.0).abs() < epsilon);
        assert!((master_meter_level(10.0_f32.powf(-6.0 / 20.0)) - 0.9).abs() < epsilon);
        assert!((master_meter_level(10.0_f32.powf(-12.0 / 20.0)) - 0.8).abs() < epsilon);
        assert!((master_meter_level(0.01) - (20.0 / 60.0)).abs() < epsilon);
        assert_eq!(master_meter_level(0.001), 0.0);
        assert_eq!(master_meter_level(0.0), 0.0);
        assert_eq!(master_meter_level(f32::NAN), 0.0);
    }

    #[test]
    fn quantized_meter_display_keeps_audible_low_levels_visible() {
        let minus_forty_db = meter_display_level(0.01);
        assert!(
            minus_forty_db >= 0.31,
            "-40 dBFS should light several meter segments"
        );
        assert!(
            minus_forty_db <= 0.35,
            "-40 dBFS should remain near one third scale"
        );
        assert_eq!(meter_display_level(0.0005), 0.0);
    }

    #[test]
    fn neural_networks_reactive_value_reflects_current_pattern_model() {
        let state = Arc::new(SequencerState::new(
            2,
            vec![default_empty_effect_chain(), default_empty_effect_chain()],
        ));
        state
            .edit_current_neural_networks(|networks| {
                let mut network = sequencer::neural::ProjectNeuralNetwork {
                    id: 11,
                    name: "router".to_string(),
                    num_neurons: 2,
                    weights: vec![vec![0.0, 0.75], vec![0.0, 0.0]],
                    ..sequencer::neural::ProjectNeuralNetwork::default()
                };
                network.reset_interval_bars = 3.0;
                network.energy_decay = 0.5;
                network.max_poly = 5;
                network.max_poly_selection = sequencer::neural::NeuralMaxPolySelection::Random;
                network.neurons[1].route = Some(1);
                network.neurons[1].delay_steps = 4;
                network.neurons[1].quantize = Some(Timebase::Eighth as u8);
                networks.push(network);
                Ok(())
            })
            .unwrap();

        let Value::List(networks) = build_neural_networks_value(&state) else {
            panic!("expected neural network list");
        };
        assert_eq!(networks.len(), 1);
        let Value::Map(network) = &*networks[0].borrow() else {
            panic!("expected neural network map");
        };
        assert_eq!(
            network.get("name").map(|value| value.borrow().clone()),
            Some(Value::String("router".to_string()))
        );
        assert_eq!(
            network
                .get("reset-bars")
                .map(|value| value.borrow().clone()),
            Some(Value::Number(3.0))
        );
        assert_eq!(
            network
                .get("energy-decay")
                .map(|value| value.borrow().clone()),
            Some(Value::Number(0.5))
        );
        assert_eq!(
            network.get("max-poly").map(|value| value.borrow().clone()),
            Some(Value::Number(5.0))
        );
        assert_eq!(
            network
                .get("max-poly-selection")
                .map(|value| value.borrow().clone()),
            Some(Value::String("random".to_string()))
        );
        let Some(neurons) = network.get("neurons") else {
            panic!("expected neurons");
        };
        let Value::List(neurons) = &*neurons.borrow() else {
            panic!("expected neuron list");
        };
        let Value::Map(neuron) = &*neurons[1].borrow() else {
            panic!("expected neuron map");
        };
        assert_eq!(
            neuron.get("route").map(|value| value.borrow().clone()),
            Some(Value::Number(1.0))
        );
        assert_eq!(
            neuron.get("delay").map(|value| value.borrow().clone()),
            Some(Value::Number(4.0))
        );
        assert_eq!(
            neuron.get("quantize").map(|value| value.borrow().clone()),
            Some(Value::Keyword("8".to_string()))
        );
    }

    #[test]
    fn neural_dampening_matrix_value_reflects_runtime_snapshot() {
        let state = Arc::new(SequencerState::new(
            2,
            vec![default_empty_effect_chain(), default_empty_effect_chain()],
        ));
        let mut snapshot = sequencer::neural::NeuralVisualizationSnapshot::default();
        snapshot.active = true;
        snapshot.network_id = 7;
        snapshot.num_neurons = 2;
        snapshot.dampening[0][1] = 0.625;
        state.set_neural_visualization(snapshot);

        let Value::List(rows) = build_neural_dampening_matrix_value(&state) else {
            panic!("expected dampening matrix rows");
        };
        assert_eq!(rows.len(), 2);
        let Value::List(first_row) = &*rows[0].borrow() else {
            panic!("expected first dampening row");
        };
        assert_eq!(first_row.len(), 2);
        assert_eq!(*first_row[1].borrow(), Value::Number(0.63));
    }

    #[test]
    fn neural_energy_and_trigger_matrix_values_reflect_runtime_snapshot() {
        let state = Arc::new(SequencerState::new(
            2,
            vec![default_empty_effect_chain(), default_empty_effect_chain()],
        ));
        let mut snapshot = sequencer::neural::NeuralVisualizationSnapshot::default();
        snapshot.active = true;
        snapshot.network_id = 7;
        snapshot.num_neurons = 2;
        snapshot.energy[0] = 0.625;
        snapshot.energy[1] = 8.0;
        snapshot.trigger_activity[1] = 1.0;
        state.set_neural_visualization(snapshot);

        let Value::List(energy_rows) = build_neural_energy_matrix_value(&state) else {
            panic!("expected energy matrix rows");
        };
        assert_eq!(energy_rows.len(), 2);
        let Value::List(first_energy_row) = &*energy_rows[0].borrow() else {
            panic!("expected first energy row");
        };
        assert_eq!(first_energy_row.len(), 1);
        assert_eq!(*first_energy_row[0].borrow(), Value::Number(0.63));
        let Value::List(second_energy_row) = &*energy_rows[1].borrow() else {
            panic!("expected second energy row");
        };
        assert_eq!(*second_energy_row[0].borrow(), Value::Number(4.0));

        let Value::List(trigger_rows) = build_neural_trigger_matrix_value(&state) else {
            panic!("expected trigger matrix rows");
        };
        assert_eq!(trigger_rows.len(), 2);
        let Value::List(first_trigger_row) = &*trigger_rows[0].borrow() else {
            panic!("expected first trigger row");
        };
        let Value::List(second_trigger_row) = &*trigger_rows[1].borrow() else {
            panic!("expected second trigger row");
        };
        assert_eq!(*first_trigger_row[0].borrow(), Value::Number(0.0));
        assert_eq!(*second_trigger_row[0].borrow(), Value::Number(1.0));
    }

    #[test]
    fn graph_visualizations_value_reflects_runtime_snapshot() {
        let state = Arc::new(SequencerState::new(
            2,
            vec![default_empty_effect_chain(), default_empty_effect_chain()],
        ));
        state.set_graph_visualizations(vec![sequencer::graph::GraphVisualizationSnapshot {
            id: 9,
            name: "graph".to_string(),
            active: true,
            num_nodes: 2,
            energy: vec![0.125, 8.0],
            trigger_activity: vec![0.0, 1.0],
            node_events: vec![
                None,
                Some(sequencer::graph::GraphVisualizationEvent {
                    node_index: 1,
                    track: Some(3),
                    sample_time: 12_345,
                    beat: 1.5,
                    transpose: -7.25,
                    velocity: 0.625,
                }),
            ],
            edges: vec![sequencer::graph::GraphVisualizationEdge {
                from: 0,
                to: 1,
                weight: 0.333,
                dampening: 0.625,
                delay_steps: 3,
                distribution: sequencer::graph::EdgeDistribution::WeightedChoice,
            }],
        }]);

        let Value::List(graphs) = build_graph_visualizations_value(&state) else {
            panic!("expected graph visualization list");
        };
        assert_eq!(graphs.len(), 1);
        let Value::Map(graph) = &*graphs[0].borrow() else {
            panic!("expected graph map");
        };
        assert_eq!(
            graph.get("name").map(|value| value.borrow().clone()),
            Some(Value::String("graph".to_string()))
        );

        let Value::List(weight_rows) = &*graph.get("weight-matrix").unwrap().borrow() else {
            panic!("expected weight matrix");
        };
        let Value::List(weight_row_0) = &*weight_rows[0].borrow() else {
            panic!("expected weight matrix row");
        };
        assert_eq!(*weight_row_0[0].borrow(), Value::Number(0.0));
        assert_eq!(*weight_row_0[1].borrow(), Value::Number(0.33));

        let Value::List(dampening_rows) = &*graph.get("dampening-matrix").unwrap().borrow() else {
            panic!("expected dampening matrix");
        };
        let Value::List(dampening_row_0) = &*dampening_rows[0].borrow() else {
            panic!("expected dampening matrix row");
        };
        assert_eq!(*dampening_row_0[1].borrow(), Value::Number(0.63));

        let Value::List(energy_rows) = &*graph.get("energy-matrix").unwrap().borrow() else {
            panic!("expected energy matrix");
        };
        let Value::List(energy_row_1) = &*energy_rows[1].borrow() else {
            panic!("expected energy matrix row");
        };
        assert_eq!(*energy_row_1[0].borrow(), Value::Number(4.0));

        let Value::List(trigger_rows) = &*graph.get("trigger-matrix").unwrap().borrow() else {
            panic!("expected trigger matrix");
        };
        let Value::List(trigger_row_1) = &*trigger_rows[1].borrow() else {
            panic!("expected trigger matrix row");
        };
        assert_eq!(*trigger_row_1[0].borrow(), Value::Number(1.0));

        let Value::List(node_events) = &*graph.get("node-events").unwrap().borrow() else {
            panic!("expected node events");
        };
        assert_eq!(*node_events[0].borrow(), Value::Nil);
        let Value::Map(event) = &*node_events[1].borrow() else {
            panic!("expected event map");
        };
        assert_eq!(
            event.get("track").map(|value| value.borrow().clone()),
            Some(Value::Number(3.0))
        );
        assert_eq!(
            event.get("transpose").map(|value| value.borrow().clone()),
            Some(Value::Number(-7.25))
        );
        assert_eq!(
            event.get("velocity").map(|value| value.borrow().clone()),
            Some(Value::Number(0.625))
        );
    }

    #[test]
    fn sampler_scrub_mod_depth_range_uses_display_domain() {
        let desc = sequencer::effects::EffectDescriptor::builtin_sampler();
        let target = desc
            .instrument_modulation_targets
            .iter()
            .find(|target| {
                desc.params
                    .get(target.base_param_idx)
                    .map(|param| param.name == "scrub")
                    .unwrap_or(false)
            })
            .expect("sampler scrub should be modulatable");
        let depth_desc = desc
            .params
            .get(target.depth_param_idx)
            .expect("sampler scrub mod depth param should exist");

        let (min, max) = sampler_modulation_depth_display_range(depth_desc, target);

        assert_eq!((min, max), (-100.0, 100.0));
    }

    #[test]
    fn custom_instrument_mod_depth_range_uses_manifest_domain() {
        let depth_desc = sequencer::effects::ParamDescriptor {
            name: "mod depth".to_string(),
            min: -1.0,
            max: 1.0,
            default: 0.0,
            kind: sequencer::effects::ParamKind::Continuous {
                unit: Some("%".to_string()),
            },
            scaling: sequencer::effects::ParamScaling::Linear,
            node_param_idx: 0,
            node_param_span: 1,
            host_control: None,
            ui_metadata: None,
        };
        let target = sequencer::effects::InstrumentModulationTarget {
            base_param_idx: 0,
            source_param_idx: Some(1),
            modulator_slot: 0,
            depth_param_idx: 2,
            active_param_idx: None,
            depth_min: -1.0,
            depth_max: 1.0,
            depth_unit: Some("%".to_string()),
        };

        assert_eq!(
            sampler_modulation_depth_display_range(&depth_desc, &target),
            (-100.0, 100.0)
        );
        assert_eq!(
            instrument_modulation_depth_display_range(&target),
            (-1.0, 1.0)
        );
    }

    fn parse_expression_at(tokens: &[Token], pos: &mut usize) -> Result<(), ParserError> {
        match tokens.get(*pos) {
            Some(Token::LeftParen) => {
                *pos += 1;
                while let Some(token) = tokens.get(*pos) {
                    match token {
                        Token::RightParen => {
                            *pos += 1;
                            return Ok(());
                        }
                        _ => parse_expression_at(tokens, pos)?,
                    }
                }
                Err(ParserError::UnexpectedEOF)
            }
            Some(Token::Quote) => {
                *pos += 1;
                match tokens.get(*pos) {
                    Some(Token::Symbol(_)) => {
                        *pos += 1;
                        Ok(())
                    }
                    Some(Token::LeftParen) => parse_expression_at(tokens, pos),
                    Some(Token::Number(_))
                    | Some(Token::RightParen)
                    | Some(Token::Pipe)
                    | Some(Token::Quote)
                    | Some(Token::String(_))
                    | Some(Token::Keyword(_))
                    | Some(Token::Backtick)
                    | Some(Token::Comma) => Err(ParserError::InvalidQuote),
                    None => Err(ParserError::UnexpectedEOF),
                }
            }
            Some(Token::Pipe) => {
                *pos += 1;
                loop {
                    match tokens.get(*pos) {
                        Some(Token::Pipe) => {
                            *pos += 1;
                            break;
                        }
                        Some(Token::Symbol(_)) | Some(Token::LeftParen) => {
                            parse_expression_at(tokens, pos)?
                        }
                        Some(_) => return Err(ParserError::InvalidLambda),
                        None => return Err(ParserError::UnexpectedEOF),
                    }
                }
                parse_expression_at(tokens, pos)
            }
            Some(Token::Backtick) | Some(Token::Comma) => {
                *pos += 1;
                parse_expression_at(tokens, pos)
            }
            Some(Token::Number(_))
            | Some(Token::String(_))
            | Some(Token::Symbol(_))
            | Some(Token::Keyword(_)) => {
                *pos += 1;
                Ok(())
            }
            Some(Token::RightParen) => Err(ParserError::ExpectedLeftParen),
            None => Err(ParserError::UnexpectedEOF),
        }
    }

    #[test]
    fn metal_seq_grid_lisp_parses() {
        let src = std::fs::read_to_string("metal-seq-grid.lisp").expect("read metal-seq-grid.lisp");
        let tokens = Parser::new(src)
            .parse()
            .expect("tokenize metal-seq-grid.lisp");
        let mut pos = 0;
        while pos < tokens.len() {
            if let Err(err) = parse_expression_at(&tokens, &mut pos) {
                let start = pos.saturating_sub(8);
                let end = (pos + 8).min(tokens.len());
                panic!(
                    "parse metal-seq-grid.lisp at token {pos}: {err:?}\ncontext: {:?}",
                    &tokens[start..end]
                );
            }
        }
        ASTParser::new(tokens)
            .parse()
            .expect("parse metal-seq-grid.lisp");
    }

    #[test]
    fn metal_seq_browser_lisp_parses() {
        let src = std::fs::read_to_string("metal-seq-browser.lisp").expect("read browser lisp");
        let tokens = Parser::new(src)
            .parse()
            .expect("tokenize metal-seq-browser.lisp");
        let mut pos = 0;
        while pos < tokens.len() {
            if let Err(err) = parse_expression_at(&tokens, &mut pos) {
                let start = pos.saturating_sub(8);
                let end = (pos + 8).min(tokens.len());
                panic!(
                    "parse metal-seq-browser.lisp at token {pos}: {err:?}\ncontext: {:?}",
                    &tokens[start..end]
                );
            }
        }
        ASTParser::new(tokens)
            .parse()
            .expect("parse metal-seq-browser.lisp");
    }

    #[test]
    fn metal_seq_core_lisp_files_parse() {
        for path in [
            "mac-osx-dark.lisp",
            "ableton-mid.lisp",
            "mac-osx-graphite.lisp",
            "mac-osx-haze.lisp",
            "mac-osx-midnight.lisp",
            "black-ir-theme.lisp",
            "mac-osx-ember.lisp",
            "mac-osx-violet.lisp",
            "metal-seq-themes.lisp",
            "metal-seq-materials.lisp",
            "metal-seq-browser.lisp",
            "metal-seq-builtin-fx-ui.lisp",
            "metal-seq-fx/builtin/filter-core.lisp",
            "metal-seq-fx/builtin/str8-delay.lisp",
            "metal-seq-fx/builtin/filter-panel.lisp",
            "metal-seq-fx/builtin/dynamics.lisp",
            "metal-seq-fx/builtin/tape.lisp",
            "metal-seq-fx/builtin/dj-mixer.lisp",
            "metal-seq-fx/builtin/audio-fx.lisp",
            "metal-seq-fx.lisp",
            "metal-seq-fx/state.lisp",
            "metal-seq-fx/panel-frame.lisp",
            "metal-seq-fx/drag-drop.lisp",
            "metal-seq-fx/track-panels.lisp",
            "metal-seq-fx/panel-widgets.lisp",
            "metal-seq-fx/param-controls.lisp",
            "metal-seq-fx/param-grid.lisp",
            "metal-seq-fx/instrument-modulation.lisp",
            "metal-seq-fx/effect-modulation.lisp",
            "metal-seq-fx/instrument-sources.lisp",
            "metal-seq-fx/effect-panels.lisp",
            "metal-seq-fx/custom-ui-runtime.lisp",
            "metal-seq-fx/custom-ui-sections.lisp",
            "metal-seq-fx/custom-ui-controls.lisp",
            "metal-seq-fx/custom-ui-lego.lisp",
            "metal-seq-fx/custom-effect-ui.lisp",
            "metal-seq-fx/panel-bodies.lisp",
            "metal-seq-fx/sampler-panel.lisp",
            "metal-seq-fx/modulator-panel.lisp",
            "metal-seq-fx/instrument-panel.lisp",
            "metal-seq-fx/buffers.lisp",
            "metal-seq-piano-roll.lisp",
            "metal-seq-mixer-v2.lisp",
            "metal-seq-transport.lisp",
            "metal-seq-agent.lisp",
            "metal-seq-metal.lisp",
            "metal-seq-sequencer.lisp",
            "metal-seq-grid.lisp",
        ] {
            let src = std::fs::read_to_string(path).unwrap_or_else(|e| panic!("read {path}: {e}"));
            let tokens = Parser::new(src)
                .parse()
                .unwrap_or_else(|e| panic!("tokenize {path}: {e:?}"));
            ASTParser::new(tokens)
                .parse()
                .unwrap_or_else(|e| panic!("parse {path}: {e:?}"));
        }
    }

    fn load_step_gesture_source(runtime: &mut Runtime) {
        let src = std::fs::read_to_string("metal-seq-grid.lisp").expect("read metal-seq-grid.lisp");
        let start = src
            .find("(def selection-click?")
            .expect("step gesture source should define selection-click?");
        let end = src
            .find("(def seq-set-step-param-from-step")
            .expect("step gesture source should precede step param helpers");
        runtime
            .eval_str(&src[start..end])
            .expect("load step gesture source");
    }

    fn register_step_gesture_test_natives(
        runtime: &mut Runtime,
        steps: Arc<Mutex<Vec<bool>>>,
        selected: Arc<Mutex<Vec<bool>>>,
        toggles: Arc<Mutex<Vec<usize>>>,
        moves: Arc<Mutex<Vec<(usize, usize)>>>,
    ) {
        runtime.register_native("cool-off-follow", |_args, _ctx| Ok(Value::Nil));

        let selected_for_has_selection = selected.clone();
        runtime.register_native("seq-has-selection?", move |_args, _ctx| {
            Ok(Value::Bool(
                selected_for_has_selection
                    .lock()
                    .unwrap()
                    .iter()
                    .any(|selected| *selected),
            ))
        });

        let steps_for_active = steps.clone();
        runtime.register_native("seq-track-step-active?", move |args, _ctx| {
            let Some(Value::Number(step)) = args.get(1) else {
                return Err("seq-track-step-active?: expected track and step".into());
            };
            Ok(Value::Bool(
                steps_for_active.lock().unwrap()[*step as usize],
            ))
        });

        let steps_for_toggle = steps.clone();
        let toggles_for_toggle = toggles.clone();
        runtime.register_native("seq-toggle-step", move |args, _ctx| {
            let Some(Value::Number(step)) = args.first() else {
                return Err("seq-toggle-step: expected step".into());
            };
            let step = *step as usize;
            let mut steps = steps_for_toggle.lock().unwrap();
            steps[step] = !steps[step];
            toggles_for_toggle.lock().unwrap().push(step);
            Ok(Value::Bool(steps[step]))
        });

        let moves_for_drag = moves.clone();
        runtime.register_native("seq-move-step-drag", move |args, _ctx| {
            let (Some(Value::Number(start)), Some(Value::Number(target))) =
                (args.first(), args.get(1))
            else {
                return Err("seq-move-step-drag: expected start and target".into());
            };
            moves_for_drag
                .lock()
                .unwrap()
                .push((*start as usize, *target as usize));
            Ok(Value::Bool(true))
        });

        runtime.register_native("seq-select-step-range", |_args, _ctx| Ok(Value::Nil));
    }

    fn step_gesture_runtime(
        initial_steps: &[bool],
        selected_steps: &[bool],
    ) -> (
        Runtime,
        Arc<Mutex<Vec<bool>>>,
        Arc<Mutex<Vec<usize>>>,
        Arc<Mutex<Vec<(usize, usize)>>>,
    ) {
        let steps = Arc::new(Mutex::new(initial_steps.to_vec()));
        let selected = Arc::new(Mutex::new(selected_steps.to_vec()));
        let toggles = Arc::new(Mutex::new(Vec::new()));
        let moves = Arc::new(Mutex::new(Vec::new()));
        let mut runtime = Runtime::new();
        runtime
            .eval_str("(defstate cursor-step 0)")
            .expect("define cursor step");
        runtime
            .eval_str(&format!(
                "(def SEQ (dict :current-track 0 :selected-steps '{}))",
                bool_list_source(selected_steps)
            ))
            .expect("define SEQ test map");
        register_step_gesture_test_natives(
            &mut runtime,
            steps.clone(),
            selected,
            toggles.clone(),
            moves.clone(),
        );
        load_step_gesture_source(&mut runtime);
        (runtime, steps, toggles, moves)
    }

    fn track_qualified_step_gesture_runtime() -> (
        Runtime,
        Arc<Mutex<Vec<Vec<bool>>>>,
        Arc<Mutex<Vec<(usize, usize)>>>,
    ) {
        let steps = Arc::new(Mutex::new(vec![
            vec![false, true, false, false],
            vec![false, false, false, false],
        ]));
        let current_track = Arc::new(Mutex::new(0usize));
        let toggles = Arc::new(Mutex::new(Vec::new()));
        let mut runtime = Runtime::new();
        runtime
            .eval_str("(defstate cursor-step 0)")
            .expect("define cursor step");
        runtime
            .eval_str(
                "(def SEQ (dict :current-track 1 :selected-steps '(false false false false)))",
            )
            .expect("define stale SEQ test map");
        runtime.register_native("cool-off-follow", |_args, _ctx| Ok(Value::Nil));
        runtime.register_native("seq-has-selection?", |_args, _ctx| Ok(Value::Bool(false)));
        runtime.register_native("seq-select-step-range", |_args, _ctx| Ok(Value::Nil));
        runtime.register_native("seq-move-step-drag", |_args, _ctx| Ok(Value::Bool(true)));
        {
            let current_track = current_track.clone();
            runtime.register_native("seq-set-track", move |args, _ctx| {
                let Some(Value::Number(track)) = args.first() else {
                    return Err("seq-set-track: expected track".into());
                };
                *current_track.lock().unwrap() = *track as usize;
                Ok(Value::Number(*track))
            });
        }
        {
            let steps = steps.clone();
            runtime.register_native("seq-track-step-active?", move |args, _ctx| {
                let (Some(Value::Number(track)), Some(Value::Number(step))) =
                    (args.first(), args.get(1))
                else {
                    return Err("seq-track-step-active?: expected track and step".into());
                };
                Ok(Value::Bool(
                    steps.lock().unwrap()[*track as usize][*step as usize],
                ))
            });
        }
        {
            let steps = steps.clone();
            let current_track = current_track.clone();
            let toggles = toggles.clone();
            runtime.register_native("seq-toggle-step", move |args, _ctx| {
                let Some(Value::Number(step)) = args.first() else {
                    return Err("seq-toggle-step: expected step".into());
                };
                let track = *current_track.lock().unwrap();
                let step = *step as usize;
                let mut steps = steps.lock().unwrap();
                steps[track][step] = !steps[track][step];
                toggles.lock().unwrap().push((track, step));
                Ok(Value::Bool(steps[track][step]))
            });
        }
        load_step_gesture_source(&mut runtime);
        (runtime, steps, toggles)
    }

    fn bool_list_source(values: &[bool]) -> String {
        let items = values
            .iter()
            .map(|value| if *value { "true" } else { "false" })
            .collect::<Vec<_>>()
            .join(" ");
        format!("({items})")
    }

    #[test]
    fn empty_step_drag_paints_steps_on_without_starting_move_drag() {
        let (mut runtime, steps, toggles, moves) =
            step_gesture_runtime(&[false, false, false, false, false], &[false; 5]);

        runtime
            .eval_str("(step-pointer-down 1 (dict))")
            .expect("pointer down");
        runtime
            .eval_str("(step-select-drag-over 2 (dict))")
            .expect("drag over step 2");
        runtime
            .eval_str("(step-select-drag-over 3 (dict))")
            .expect("drag over step 3");
        runtime
            .eval_str("(step-pointer-up 3 (dict))")
            .expect("pointer up");

        assert_eq!(*steps.lock().unwrap(), vec![false, true, true, true, false]);
        assert_eq!(*toggles.lock().unwrap(), vec![1, 2, 3]);
        assert!(moves.lock().unwrap().is_empty());
    }

    #[test]
    fn active_step_drag_moves_without_toggling_clicked_step_off() {
        let (mut runtime, steps, toggles, moves) =
            step_gesture_runtime(&[false, false, true, false, false], &[false; 5]);

        runtime
            .eval_str("(step-pointer-down 2 (dict))")
            .expect("pointer down");
        runtime
            .eval_str("(step-select-drag-over 3 (dict))")
            .expect("drag over step 3");
        runtime
            .eval_str("(step-pointer-up 3 (dict))")
            .expect("pointer up");

        assert_eq!(
            *steps.lock().unwrap(),
            vec![false, false, true, false, false]
        );
        assert!(toggles.lock().unwrap().is_empty());
        assert_eq!(*moves.lock().unwrap(), vec![(2, 3)]);
    }

    #[test]
    fn selected_empty_step_drag_uses_move_drag_instead_of_painting() {
        let (mut runtime, steps, toggles, moves) = step_gesture_runtime(
            &[false, false, false, false, false],
            &[false, false, true, false, false],
        );

        runtime
            .eval_str("(step-pointer-down 2 (dict))")
            .expect("pointer down");
        runtime
            .eval_str("(step-select-drag-over 4 (dict))")
            .expect("drag over step 4");
        runtime
            .eval_str("(step-pointer-up 4 (dict))")
            .expect("pointer up");

        assert_eq!(
            *steps.lock().unwrap(),
            vec![false, false, false, false, false]
        );
        assert!(toggles.lock().unwrap().is_empty());
        assert_eq!(*moves.lock().unwrap(), vec![(2, 4)]);
    }

    #[test]
    fn track_qualified_step_click_ignores_stale_reactive_current_track() {
        let (mut runtime, steps, toggles) = track_qualified_step_gesture_runtime();

        runtime
            .eval_str("(seq-set-track 0)")
            .expect("host switches to clicked track before gesture handling");
        runtime
            .eval_str("(step-pointer-down-for-track 0 1 (dict) false)")
            .expect("pointer down on active step in clicked track");
        runtime
            .eval_str("(step-select-drag-over-for-track 0 1 (dict))")
            .expect("same-step drag jitter should not enter paint-on mode");

        assert!(
            toggles.lock().unwrap().is_empty(),
            "pointer-down and same-step jitter must not toggle an already-active step off through stale SEQ.current-track"
        );

        runtime
            .eval_str("(step-pointer-up 1 (dict))")
            .expect("pointer up on clicked step");

        assert_eq!(
            *toggles.lock().unwrap(),
            vec![(0, 1)],
            "a plain click on an active step in another track should toggle exactly once"
        );
        assert!(
            !steps.lock().unwrap()[0][1],
            "the clicked active step should end off"
        );
    }

    #[test]
    fn metal_seq_agent_lisp_creates_agent_buffer_tree() {
        let mut editor =
            eseqlisp::Editor::new(eseqlisp::Runtime::new(), eseqlisp::EditorConfig::default());
        register_agent_test_natives(editor.runtime_mut());
        editor.runtime_mut().register_reactive(
            "AGENT",
            vec![("generation", Value::Number(0.0))],
            false,
        );
        editor
            .runtime_mut()
            .eval_str(r#"(load "mac-osx-dark.lisp")"#)
            .expect("load theme");
        editor
            .runtime_mut()
            .eval_str(r#"(load "metal-seq-agent.lisp")"#)
            .expect("load agent lisp");
        editor.refresh_runtime_side_effects();
        let agent = editor
            .buffers
            .iter()
            .find(|buffer| buffer.name == "*agent*")
            .expect("agent lisp should create the *agent* buffer");
        assert!(
            agent.widget_tree.is_some(),
            "*agent* should own a widget tree instead of showing stale UI"
        );
        let artifacts = editor
            .buffers
            .iter()
            .find(|buffer| buffer.name == "*agent-artifacts*")
            .expect("agent lisp should create the *agent-artifacts* buffer");
        assert!(
            artifacts.widget_tree.is_some(),
            "*agent-artifacts* should own the artifact-side panel"
        );
    }

    #[test]
    fn metal_seq_agent_open_starts_general_conversation() {
        let calls = std::sync::Arc::new(std::sync::Mutex::new(Vec::<Vec<Value>>::new()));
        let mut editor =
            eseqlisp::Editor::new(eseqlisp::Runtime::new(), eseqlisp::EditorConfig::default());
        register_agent_test_natives(editor.runtime_mut());
        let captured_calls = calls.clone();
        editor
            .runtime_mut()
            .register_native("agent/new", move |args, _ctx| {
                captured_calls.lock().unwrap().push(args.to_vec());
                Ok(Value::Number(1.0))
            });
        editor.runtime_mut().register_reactive(
            "AGENT",
            vec![("generation", Value::Number(0.0))],
            false,
        );
        editor
            .runtime_mut()
            .eval_str(r#"(load "mac-osx-dark.lisp")"#)
            .expect("load theme");
        editor
            .runtime_mut()
            .eval_str(r#"(load "metal-seq-agent.lisp")"#)
            .expect("load agent lisp");

        editor
            .runtime_mut()
            .eval_str("(agent-open)")
            .expect("open agent panel");

        let calls = calls.lock().unwrap();
        assert_eq!(calls.len(), 1, "agent-open should create one conversation");
        assert!(
            matches!(
                calls[0].as_slice(),
                [Value::Keyword(kind), Value::Symbol(agent_kind)]
                    if kind == "kind" && agent_kind == "general"
            ),
            "agent-open should request a general conversation, got {:?}",
            calls[0]
        );
    }

    #[test]
    fn metal_seq_agent_composer_keeps_actions_below_full_width_input() {
        use eseqlisp::layout::LayoutNode;

        struct TestTextMeasurer;
        impl eseqlisp::layout::TextMeasurer for TestTextMeasurer {
            fn measure_text_px(&self, text: &str, _font_size: f32) -> f32 {
                text.chars().count() as f32 * 8.0
            }

            fn line_height_px(&self, _font_size: f32) -> f32 {
                16.0
            }
        }

        fn node_text(node: &LayoutNode) -> Option<&str> {
            match node.props.get("text") {
                Some(Value::String(text)) => Some(text.as_str()),
                _ => None,
            }
        }

        fn find_button_by_text<'a>(node: &'a LayoutNode, text: &str) -> Option<&'a LayoutNode> {
            if node.widget_type == "button" && node_text(node) == Some(text) {
                return Some(node);
            }
            node.children
                .iter()
                .find_map(|child| find_button_by_text(child, text))
        }

        let mut editor =
            eseqlisp::Editor::new(eseqlisp::Runtime::new(), eseqlisp::EditorConfig::default());
        editor.set_text_measurer(Box::new(TestTextMeasurer), 8.0, 16.0);
        editor.set_layout_viewport(92, 24);
        register_agent_test_natives(editor.runtime_mut());
        editor.runtime_mut().register_reactive(
            "AGENT",
            vec![("generation", Value::Number(0.0))],
            false,
        );
        editor
            .runtime_mut()
            .eval_str(r#"(load "mac-osx-dark.lisp")"#)
            .expect("load theme");
        editor
            .runtime_mut()
            .eval_str(r#"(load "metal-seq-agent.lisp")"#)
            .expect("load agent lisp");
        editor.refresh_runtime_side_effects();

        let agent_id = editor
            .buffers
            .iter()
            .find(|buffer| buffer.name == "*agent*")
            .expect("agent buffer")
            .id;
        editor.set_active_buffer(agent_id);
        editor.active_buffer_mut().view_mode = eseqlisp::editor::ViewMode::UiOnly;
        editor.refresh_runtime_side_effects();
        let layout = editor
            .runtime_mut()
            .current_layout
            .clone()
            .expect("agent layout");

        let input = find_layout_node_by_stable_key(&layout, "agent-prompt-input")
            .expect("agent prompt input");
        let actions = find_layout_node_by_stable_key(&layout, "agent-composer-actions")
            .expect("agent action row");
        let model_select = find_layout_node_by_stable_key(&layout, "agent-model-select")
            .expect("agent model selector");
        let submit = find_layout_node_by_stable_key(&layout, "agent-submit")
            .expect("agent submit affordance");

        assert!(
            input.rect.row + input.rect.height <= actions.rect.row,
            "agent composer actions should sit below prompt input; input={:?}, actions={:?}",
            input.rect,
            actions.rect
        );
        assert!(
            input.rect.width > model_select.rect.width + submit.rect.width,
            "agent prompt should have the wide row instead of sharing width with controls; input={:?}, model={:?}, submit={:?}",
            input.rect,
            model_select.rect,
            submit.rect
        );
        assert!(
            model_select.rect.col + model_select.rect.width <= submit.rect.col,
            "agent composer controls should not overlap; model={:?}, submit={:?}",
            model_select.rect,
            submit.rect
        );
    }

    #[test]
    fn metal_seq_agent_busy_state_uses_submit_button_as_cancel() {
        use eseqlisp::layout::LayoutNode;

        struct TestTextMeasurer;
        impl eseqlisp::layout::TextMeasurer for TestTextMeasurer {
            fn measure_text_px(&self, text: &str, _font_size: f32) -> f32 {
                text.chars().count() as f32 * 8.0
            }

            fn line_height_px(&self, _font_size: f32) -> f32 {
                16.0
            }
        }

        fn node_text(node: &LayoutNode) -> Option<&str> {
            match node.props.get("text") {
                Some(Value::String(text)) => Some(text.as_str()),
                _ => None,
            }
        }

        fn find_button_by_text<'a>(node: &'a LayoutNode, text: &str) -> Option<&'a LayoutNode> {
            if node.widget_type == "button" && node_text(node) == Some(text) {
                return Some(node);
            }
            node.children
                .iter()
                .find_map(|child| find_button_by_text(child, text))
        }

        fn find_widget_by_type<'a>(
            node: &'a LayoutNode,
            widget_type: &str,
        ) -> Option<&'a LayoutNode> {
            if node.widget_type == widget_type {
                return Some(node);
            }
            node.children
                .iter()
                .find_map(|child| find_widget_by_type(child, widget_type))
        }

        fn prop_number(node: &LayoutNode, prop: &str) -> Option<f64> {
            match node.props.get(prop)? {
                Value::Number(value) => Some(*value),
                _ => None,
            }
        }

        let mut editor =
            eseqlisp::Editor::new(eseqlisp::Runtime::new(), eseqlisp::EditorConfig::default());
        editor.set_text_measurer(Box::new(TestTextMeasurer), 8.0, 16.0);
        editor.set_layout_viewport(92, 24);
        register_agent_test_natives(editor.runtime_mut());
        let cancel_calls = std::sync::Arc::new(std::sync::Mutex::new(Vec::<i64>::new()));
        editor
            .runtime_mut()
            .register_native("agent/status", |_args, _ctx| {
                Ok(Value::Symbol("streaming".to_string()))
            });
        let captured_cancel_calls = cancel_calls.clone();
        editor
            .runtime_mut()
            .register_native("agent/cancel", move |args, _ctx| {
                let Some(Value::Number(conv_id)) = args.first() else {
                    return Err("agent/cancel: expected conv-id".to_string());
                };
                captured_cancel_calls.lock().unwrap().push(*conv_id as i64);
                Ok(Value::Nil)
            });
        editor.runtime_mut().register_reactive(
            "AGENT",
            vec![("generation", Value::Number(0.0))],
            false,
        );
        editor
            .runtime_mut()
            .eval_str(r#"(load "mac-osx-dark.lisp")"#)
            .expect("load theme");
        editor
            .runtime_mut()
            .eval_str(r#"(load "metal-seq-agent.lisp")"#)
            .expect("load agent lisp");
        editor
            .runtime_mut()
            .eval_str("(set! agent-current-conv 1)")
            .expect("select test conversation");
        editor
            .runtime_mut()
            .eval_str("(agent-submit-current)")
            .expect("busy submit should cancel the active request");
        assert_eq!(
            *cancel_calls.lock().unwrap(),
            vec![1],
            "busy submit affordance should route to agent/cancel"
        );
        editor.refresh_runtime_side_effects();

        let agent_id = editor
            .buffers
            .iter()
            .find(|buffer| buffer.name == "*agent*")
            .expect("agent buffer")
            .id;
        editor.set_active_buffer(agent_id);
        editor.active_buffer_mut().view_mode = eseqlisp::editor::ViewMode::UiOnly;
        editor.refresh_runtime_side_effects();
        let layout = editor
            .runtime_mut()
            .current_layout
            .clone()
            .expect("agent layout");

        assert!(
            find_button_by_text(&layout, "Cancel").is_none(),
            "busy agent composer should not render a separate Cancel button"
        );

        let submit = find_layout_node_by_stable_key(&layout, "agent-submit")
            .expect("agent submit affordance");
        assert!(
            submit.rect.width > 0.0 && submit.rect.height > 0.0,
            "agent submit/cancel affordance should remain visible while busy: {:?}",
            submit.rect
        );

        let icon = find_widget_by_type(&layout, "agent-submit-icon").expect("agent submit icon");
        assert_eq!(prop_number(icon, "canceling"), Some(1.0));
        assert_eq!(prop_number(icon, "active"), Some(1.0));
    }

    #[test]
    fn metal_seq_agent_transcript_uses_virtualized_message_stack() {
        use eseqlisp::layout::LayoutNode;

        struct TestTextMeasurer;
        impl eseqlisp::layout::TextMeasurer for TestTextMeasurer {
            fn measure_text_px(&self, text: &str, _font_size: f32) -> f32 {
                text.chars().count() as f32 * 8.0
            }

            fn line_height_px(&self, _font_size: f32) -> f32 {
                16.0
            }
        }

        fn find_label_containing<'a>(node: &'a LayoutNode, needle: &str) -> Option<&'a LayoutNode> {
            if node.widget_type == "label"
                && matches!(node.props.get("text"), Some(Value::String(text)) if text.contains(needle))
            {
                return Some(node);
            }
            node.children
                .iter()
                .find_map(|child| find_label_containing(child, needle))
        }

        let mut editor =
            eseqlisp::Editor::new(eseqlisp::Runtime::new(), eseqlisp::EditorConfig::default());
        editor.set_text_measurer(Box::new(TestTextMeasurer), 8.0, 16.0);
        editor.set_layout_viewport(92, 24);
        register_agent_test_natives(editor.runtime_mut());
        editor
            .runtime_mut()
            .register_native("agent/messages", |_args, _ctx| {
                let messages = (0..80)
                    .map(|index| {
                        map_value([
                            ("role", Value::Symbol("assistant".to_string())),
                            (
                                "display-text",
                                Value::String(format!(
                                    "message {index} with enough text to produce a measurable card"
                                )),
                            ),
                            ("has-code-blocks", Value::Bool(false)),
                        ])
                    })
                    .collect::<Vec<_>>();
                Ok(test_list(messages))
            });
        editor.runtime_mut().register_reactive(
            "AGENT",
            vec![("generation", Value::Number(0.0))],
            false,
        );
        editor
            .runtime_mut()
            .eval_str(r#"(load "mac-osx-dark.lisp")"#)
            .expect("load theme");
        editor
            .runtime_mut()
            .eval_str(r#"(load "metal-seq-agent.lisp")"#)
            .expect("load agent lisp");
        editor
            .runtime_mut()
            .eval_str("(set! agent-current-conv 1)")
            .expect("select test conversation");
        editor.refresh_runtime_side_effects();

        let agent_id = editor
            .buffers
            .iter()
            .find(|buffer| buffer.name == "*agent*")
            .expect("agent buffer")
            .id;
        editor.set_active_buffer(agent_id);
        editor.active_buffer_mut().view_mode = eseqlisp::editor::ViewMode::UiOnly;
        editor.refresh_runtime_side_effects();
        let layout = editor
            .runtime_mut()
            .current_layout
            .clone()
            .expect("agent layout");
        let stack = find_layout_node_by_stable_key(&layout, "agent-message-stack")
            .expect("virtualized message stack");

        assert_eq!(stack.widget_type, "virtual-v-stack");
        assert!(
            !stack.children.is_empty() && stack.children.len() < 80,
            "agent transcript should materialize only a visible message window, got {} children",
            stack.children.len()
        );
        let first_message = find_layout_node_by_stable_key(&layout, "agent-message-79")
            .expect("latest visible message card");
        assert!(
            first_message.rect.height > 0.0 && first_message.rect.width > 0.0,
            "visible message card should have a finite measured rect: {:?}",
            first_message.rect
        );
        let first_label =
            find_label_containing(&layout, "message 79").expect("latest visible message text");
        assert!(
            first_label.rect.height > 0.0 && first_label.rect.width > 0.0,
            "visible message text should have a finite measured rect: {:?}",
            first_label.rect
        );
    }

    #[test]
    fn metal_seq_builtin_fx_ui_lisp_loads() {
        let mut runtime = Runtime::new();
        let loaded = runtime
            .eval_str(r#"(load "metal-seq-builtin-fx-ui.lisp")"#)
            .expect("load builtin fx ui lisp")
            .expect("load should return a value");
        assert!(
            !matches!(&loaded, Value::String(s) if s.starts_with("load:")),
            "builtin fx ui load failed: {loaded:?}"
        );
        runtime
            .eval_str("builtin-audio-fx-ui")
            .expect("builtin-audio-fx-ui should be defined");
    }

    fn browser_editor_on_instrument_tab() -> eseqlisp::Editor {
        let src = std::fs::read_to_string("metal-seq-browser.lisp").expect("read browser lisp");
        let mut editor = eseqlisp::Editor::new(Runtime::new(), eseqlisp::EditorConfig::default());
        editor.runtime_mut().register_reactive(
            "SEQ",
            vec![
                ("num-tracks", Value::Number(0.0)),
                ("current-track", Value::Number(0.0)),
                ("sidebar-kind", Value::String("sampler".to_string())),
                ("sidebar-track-index", Value::Number(0.0)),
                ("sidebar-selected-sample", Value::String(String::new())),
                ("sidebar-presets", test_list(vec![])),
                ("sidebar-loaded-preset", Value::String(String::new())),
                ("sidebar-instrument-name", Value::String(String::new())),
                (
                    "sidebar-instrument-display-name",
                    Value::String(String::new()),
                ),
                ("current-project-name", Value::String(String::new())),
                ("editor-mode", Value::String(String::new())),
                ("editor-buffer-name", Value::String(String::new())),
                ("editor-canceling", Value::Bool(false)),
                ("editor-error", Value::String(String::new())),
                ("editor-active-macro-name", Value::String(String::new())),
                ("editor-active-macro-action", Value::String(String::new())),
                (
                    "editor-instrument-run-mode",
                    Value::String("instrument".to_string()),
                ),
            ],
            true,
        );
        editor
            .runtime_mut()
            .register_native("seq-filter-sample-tree", |_args, _ctx| {
                Ok(test_sample_tree())
            });
        editor
            .runtime_mut()
            .register_native(
                "seq-sample-browser",
                |_args, _ctx| Ok(test_sample_browser()),
            );
        editor
            .runtime_mut()
            .register_native("seq-sample-tags-for-path", |_args, _ctx| {
                Ok(test_list(
                    vec!["kick", "808"]
                        .into_iter()
                        .map(|tag| Value::String(tag.to_string()))
                        .collect(),
                ))
            });
        editor
            .runtime_mut()
            .register_native("seq-project-tree", |_args, _ctx| Ok(test_list(vec![])));
        editor
            .runtime_mut()
            .register_native("seq-preset-tree", |_args, _ctx| Ok(test_list(vec![])));
        editor
            .runtime_mut()
            .register_native("seq-audio-effect-tree", |_args, _ctx| Ok(test_list(vec![])));
        editor
            .runtime_mut()
            .register_native("seq-midi-effect-tree", |_args, _ctx| Ok(test_list(vec![])));
        editor
            .runtime_mut()
            .register_native("seq-saved-instrument-tree", |args, _ctx| {
                let query = match args.first() {
                    Some(Value::String(s)) => s.as_str(),
                    _ => "",
                };
                Ok(build_instrument_tree_value(query))
            });
        editor
            .runtime_mut()
            .eval_str("(defstate selected-bus -1)")
            .expect("define browser test selected bus state");
        editor
            .runtime_mut()
            .eval_str("(def seq-has-selected-bus? () (>= selected-bus 0))")
            .expect("define browser test selected bus predicate");
        editor
            .runtime_mut()
            .eval_str(&src)
            .expect("load browser lisp");
        editor
            .runtime_mut()
            .eval_str("(set! sbrowser-tab \"instruments\")")
            .expect("select instrument tab");
        editor.refresh_runtime_side_effects();
        if let Some(status) = editor.runtime_mut().take_status_message() {
            panic!("browser lisp status after refresh: {status}");
        }
        editor
    }

    fn browser_id(editor: &eseqlisp::Editor) -> eseqlisp::host::BufferId {
        editor
            .buffers
            .iter()
            .find(|buffer| buffer.name == "*samples*")
            .expect("browser lisp should create the *samples* buffer")
            .id
    }

    fn test_sample_tree() -> Value {
        test_list(vec![map_value([
            ("label", Value::String("drums".to_string())),
            (
                "children",
                test_list(vec![
                    map_value([
                        ("label", Value::String("kick.wav".to_string())),
                        ("path", Value::String("samples/drums/kick.wav".to_string())),
                    ]),
                    map_value([
                        ("label", Value::String("snare.wav".to_string())),
                        ("path", Value::String("samples/drums/snare.wav".to_string())),
                    ]),
                ]),
            ),
        ])])
    }

    fn test_sample_browser() -> Value {
        map_value([
            (
                "tags",
                test_list(vec![
                    map_value([
                        ("name", Value::String("kick".to_string())),
                        ("count", Value::Number(2.0)),
                        ("selected", Value::Bool(true)),
                    ]),
                    map_value([
                        ("name", Value::String("808".to_string())),
                        ("count", Value::Number(1.0)),
                        ("selected", Value::Bool(false)),
                    ]),
                ]),
            ),
            (
                "items",
                test_list(vec![
                    map_value([
                        ("label", Value::String("kick.wav".to_string())),
                        ("path", Value::String("samples/kick.wav".to_string())),
                    ]),
                    map_value([
                        ("label", Value::String("snare.wav".to_string())),
                        ("path", Value::String("samples/snare.wav".to_string())),
                    ]),
                ]),
            ),
        ])
    }

    fn find_layout_node_by_stable_key<'a>(
        node: &'a eseqlisp::layout::LayoutNode,
        key: &str,
    ) -> Option<&'a eseqlisp::layout::LayoutNode> {
        if node.stable_key.as_deref() == Some(key) {
            return Some(node);
        }
        node.children
            .iter()
            .find_map(|child| find_layout_node_by_stable_key(child, key))
    }

    fn layout_prop_bool(node: &eseqlisp::layout::LayoutNode, prop: &str) -> Option<bool> {
        match node.props.get(prop)? {
            Value::Bool(value) => Some(*value),
            Value::Number(value) => Some(*value != 0.0),
            Value::ReactiveRef { slot, .. } => {
                Some(eseqlisp::reactive::read_float_slot(slot) != 0.0)
            }
            _ => None,
        }
    }

    fn layout_prop_number(node: &eseqlisp::layout::LayoutNode, prop: &str) -> Option<f64> {
        match node.props.get(prop)? {
            Value::Number(value) => Some(*value),
            Value::ReactiveRef { slot, .. } => {
                Some(eseqlisp::reactive::read_float_slot(slot) as f64)
            }
            _ => None,
        }
    }

    fn layout_tree_has_bool_prop(
        node: &eseqlisp::layout::LayoutNode,
        prop: &str,
        expected: bool,
    ) -> bool {
        layout_prop_bool(node, prop) == Some(expected)
            || node
                .children
                .iter()
                .any(|child| layout_tree_has_bool_prop(child, prop, expected))
    }

    fn layout_tree_has_reactive_prop_field(
        node: &eseqlisp::layout::LayoutNode,
        prop: &str,
        namespace: &str,
        field: &str,
    ) -> bool {
        matches!(
            node.props.get(prop),
            Some(Value::ReactiveRef {
                namespace: actual_namespace,
                field: actual_field,
                ..
            }) if actual_namespace == namespace && actual_field == field
        ) || node
            .children
            .iter()
            .any(|child| layout_tree_has_reactive_prop_field(child, prop, namespace, field))
    }

    fn render_layout_cells(layout: &eseqlisp::layout::LayoutNode, cols: u16, rows: u16) -> String {
        let mut cell_buf = eseqlisp::widget_render::CellBuffer::new(cols, rows);
        eseqlisp::widget_render::render_widget_tree(layout, &mut cell_buf);
        cell_buf
            .cells
            .iter()
            .map(|row| {
                row.iter()
                    .map(|cell| cell.as_ref().map(|cell| cell.ch).unwrap_or(' '))
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn find_layout_text_containing<'a>(
        node: &'a eseqlisp::layout::LayoutNode,
        needle: &str,
    ) -> Option<&'a eseqlisp::layout::LayoutNode> {
        if matches!(node.props.get("text"), Some(Value::String(text)) if text.contains(needle)) {
            return Some(node);
        }
        if matches!(node.props.get("label"), Some(Value::String(text)) if text.contains(needle)) {
            return Some(node);
        }
        node.children
            .iter()
            .find_map(|child| find_layout_text_containing(child, needle))
    }

    #[test]
    fn metal_seq_browser_instrument_tab_builds_instrument_tree() {
        let editor = browser_editor_on_instrument_tab();
        let browser = editor
            .buffers
            .iter()
            .find(|buffer| buffer.name == "*samples*")
            .expect("browser lisp should create the *samples* buffer");
        let tree = browser.widget_tree.as_ref().expect("browser widget tree");
        assert!(
            value_contains_string(tree, "digitone") || value_contains_string(tree, "minimoog"),
            "instrument tab should render saved instruments"
        );
    }

    #[test]
    fn metal_seq_audio_effect_tree_excludes_new_effect_action() {
        let tree = build_audio_effect_tree("+ New Effect");
        assert!(
            !value_contains_string(&tree, "new-audio-effect"),
            "effect creation should be a sidebar button, not a tree item"
        );
        assert!(
            !value_contains_string(&tree, "+ New Effect"),
            "effect creation should be a sidebar button, not a tree label"
        );
    }

    #[test]
    fn metal_seq_browser_audio_fx_tab_renders_new_effect_button_outside_tree() {
        let mut editor = browser_editor_on_instrument_tab();
        editor
            .runtime_mut()
            .eval_str("(set! sbrowser-tab \"audio-fx\")")
            .expect("select audio fx tab");
        editor.runtime_mut().run_reactive_cycle();
        editor.refresh_runtime_side_effects();

        let browser = editor
            .buffers
            .iter()
            .find(|buffer| buffer.name == "*samples*")
            .expect("browser lisp should create the *samples* buffer");
        let tree = browser.widget_tree.as_ref().expect("browser widget tree");
        assert!(value_contains_string(tree, "+ New Effect"));
        assert!(
            value_contains_string(tree, "No audio effects found."),
            "test native returns an empty effect tree; the button must not come from tree items"
        );

        editor
            .runtime_mut()
            .eval_str("(sbrowser-enter-new-effect-editor)")
            .expect("enter new effect editor");
        let commands = editor.drain_host_commands();
        assert_eq!(commands.len(), 1);
        match &commands[0] {
            eseqlisp::host::HostCommand::Custom { name, payload } => {
                assert_eq!(name, "enter-new-effect-editor");
                assert_eq!(payload, &Value::Map(Default::default()));
            }
            other => panic!("expected enter-new-effect-editor host command, got {other:?}"),
        }
    }

    #[test]
    fn metal_seq_browser_new_instrument_editor_uses_finalize_copy() {
        let mut editor = browser_editor_on_instrument_tab();
        editor.runtime_mut().set_reactive(
            "SEQ",
            "editor-mode",
            Value::String("new-instrument".to_string()),
        );
        editor
            .runtime_mut()
            .set_reactive("SEQ", "current-track", Value::Number(2.0));
        editor.runtime_mut().run_reactive_cycle();
        editor.refresh_runtime_side_effects();
        let browser = editor
            .buffers
            .iter()
            .find(|buffer| buffer.name == "*samples*")
            .expect("browser lisp should create the *samples* buffer");
        let tree = browser
            .widget_tree
            .as_ref()
            .expect("browser widget tree")
            .clone();
        assert!(value_contains_string(&tree, "Draft patch"));
        assert!(value_contains_string(&tree, "track "));
        assert!(value_contains_string(&tree, "Mode"));
        assert!(value_contains_string(&tree, "Instrument"));
        assert!(value_contains_string(&tree, "Free Patch"));
        assert!(value_contains_string(&tree, "Save as"));
        assert!(value_contains_string(&tree, "Finalize"));
        assert!(
            !value_contains_string(&tree, "Save & Add"),
            "new instrument editor should use finalization copy"
        );

        let layout = editor
            .runtime_mut()
            .layout_snapshot_for_tree_with_viewport(&tree, Some((28.0, 13.0)))
            .expect("new instrument editor sidebar should lay out");
        for label in ["Mode", "Instrument", "Free Patch", "Finalize"] {
            let node = find_layout_text_containing(&layout, label)
                .unwrap_or_else(|| panic!("expected visible editor text: {label}"));
            assert!(
                node.rect.width.is_finite()
                    && node.rect.height.is_finite()
                    && node.rect.width > 0.0
                    && node.rect.height > 0.0,
                "editor text should have a finite nonzero rect for {label}: {:?}",
                node.rect
            );
        }
    }

    #[test]
    fn metal_seq_browser_new_effect_editor_uses_finalize_controls() {
        let mut editor = browser_editor_on_instrument_tab();
        editor
            .runtime_mut()
            .set_reactive("SEQ", "editor-active", Value::Bool(true));
        editor.runtime_mut().set_reactive(
            "SEQ",
            "editor-mode",
            Value::String("new-effect".to_string()),
        );
        editor
            .runtime_mut()
            .set_reactive("SEQ", "current-track", Value::Number(2.0));
        editor.runtime_mut().run_reactive_cycle();
        editor.refresh_runtime_side_effects();
        let browser = editor
            .buffers
            .iter()
            .find(|buffer| buffer.name == "*samples*")
            .expect("browser lisp should create the *samples* buffer");
        let tree = browser
            .widget_tree
            .as_ref()
            .expect("browser widget tree")
            .clone();
        assert!(value_contains_string(&tree, "New Effect"));
        assert!(value_contains_string(&tree, "Draft patch"));
        assert!(value_contains_string(&tree, "track "));
        assert!(value_contains_string(&tree, "Save as"));
        assert!(value_contains_string(&tree, "effect-name"));
        assert!(value_contains_string(&tree, "Save & Add"));
        assert!(
            !value_contains_string(&tree, "Search samples"),
            "new effect editor should replace the sample browser chrome"
        );

        let layout = editor
            .runtime_mut()
            .layout_snapshot_for_tree_with_viewport(&tree, Some((28.0, 13.0)))
            .expect("new effect editor sidebar should lay out");
        for label in ["New Effect", "Draft patch", "Save as", "Save & Add"] {
            let node = find_layout_text_containing(&layout, label)
                .unwrap_or_else(|| panic!("expected visible editor text: {label}"));
            assert!(
                node.rect.width.is_finite()
                    && node.rect.height.is_finite()
                    && node.rect.width > 0.0
                    && node.rect.height > 0.0,
                "editor text should have a finite nonzero rect for {label}: {:?}",
                node.rect
            );
        }
    }

    #[test]
    fn metal_seq_browser_editor_macro_action_replaces_finalize_controls() {
        let mut editor = browser_editor_on_instrument_tab();
        editor
            .runtime_mut()
            .set_reactive("SEQ", "editor-active", Value::Bool(true));
        editor.runtime_mut().set_reactive(
            "SEQ",
            "editor-mode",
            Value::String("new-instrument".to_string()),
        );
        editor.runtime_mut().set_reactive(
            "SEQ",
            "editor-active-macro-name",
            Value::String("simp".to_string()),
        );
        editor.runtime_mut().set_reactive(
            "SEQ",
            "editor-active-macro-action",
            Value::String("save-to-library".to_string()),
        );
        editor.runtime_mut().run_reactive_cycle();
        editor.refresh_runtime_side_effects();
        let browser = editor
            .buffers
            .iter()
            .find(|buffer| buffer.name == "*samples*")
            .expect("browser lisp should create the *samples* buffer");
        let tree = browser
            .widget_tree
            .as_ref()
            .expect("browser widget tree")
            .clone();
        assert!(value_contains_string(&tree, "Defmacro"));
        assert!(value_contains_string(&tree, "Current macro"));
        assert!(value_contains_string(&tree, "simp"));
        assert!(value_contains_string(&tree, "Save Macro to Library"));
        assert!(
            !value_contains_string(&tree, "Finalize"),
            "macro action panel should replace the normal new-instrument finalize control"
        );

        let layout = editor
            .runtime_mut()
            .layout_snapshot_for_tree_with_viewport(&tree, Some((28.0, 13.0)))
            .expect("macro action editor sidebar should lay out");
        for label in ["Defmacro", "Current macro", "simp", "Save Macro to Library"] {
            let node = find_layout_text_containing(&layout, label)
                .unwrap_or_else(|| panic!("expected visible macro editor text: {label}"));
            assert!(
                node.rect.width.is_finite()
                    && node.rect.height.is_finite()
                    && node.rect.width > 0.0
                    && node.rect.height > 0.0,
                "macro editor text should have a finite nonzero rect for {label}: {:?}",
                node.rect
            );
        }
    }

    #[test]
    fn metal_seq_browser_explicit_refresh_updates_inactive_samples_editor_panel() {
        let mut editor = browser_editor_on_instrument_tab();
        let browser_idx = editor
            .buffers
            .iter()
            .position(|buffer| buffer.name == "*samples*")
            .expect("browser lisp should create the *samples* buffer");
        assert_ne!(
            editor.active_buffer_idx(),
            browser_idx,
            "test setup should exercise the inactive-buffer refresh path"
        );

        editor
            .runtime_mut()
            .set_reactive("SEQ", "editor-active", Value::Bool(true));
        editor.runtime_mut().set_reactive(
            "SEQ",
            "editor-mode",
            Value::String("new-effect".to_string()),
        );
        editor
            .runtime_mut()
            .set_reactive("SEQ", "current-track", Value::Number(2.0));
        editor
            .runtime_mut()
            .eval_str("(sbrowser-refresh-buffer)")
            .expect("explicit sidebar refresh should render the samples buffer");
        editor.refresh_runtime_side_effects();

        let browser = editor
            .buffers
            .iter()
            .find(|buffer| buffer.name == "*samples*")
            .expect("browser lisp should create the *samples* buffer");
        let tree = browser.widget_tree.as_ref().expect("browser widget tree");
        assert!(value_contains_string(tree, "New Effect"));
        assert!(value_contains_string(tree, "effect-name"));
        assert!(value_contains_string(tree, "Save & Add"));
        assert!(
            !value_contains_string(tree, "Search samples"),
            "explicit refresh should replace stale sample browser content"
        );
    }

    #[test]
    fn metal_seq_browser_editor_compile_status_spinner_is_visible_and_animating() {
        let mut editor = browser_editor_on_instrument_tab();
        let tree = editor
            .runtime_mut()
            .eval_str(r#"(sbrowser-editor-status-row "Preview compiling..." :gray)"#)
            .expect("build compile status row")
            .expect("compile status row should return a widget tree");
        let layout = editor
            .runtime_mut()
            .layout_snapshot_for_tree_with_viewport(&tree, Some((80.0, 30.0)))
            .expect("compile status row should lay out");
        fn find_editor_spinner(
            node: &eseqlisp::layout::LayoutNode,
        ) -> Option<&eseqlisp::layout::LayoutNode> {
            if node.widget_type.contains("editor-spinner") {
                return Some(node);
            }
            node.children.iter().find_map(find_editor_spinner)
        }

        let spinner =
            find_editor_spinner(&layout).expect("compile status should render an animated spinner");

        assert!(
            spinner.rect.width.is_finite()
                && spinner.rect.height.is_finite()
                && spinner.rect.width > 0.0
                && spinner.rect.height > 0.0,
            "compile spinner should have a finite visible rect, got {:?}",
            spinner.rect
        );
        assert!(
            eseqlisp::widget_render::layout_wants_animation_frames(&layout),
            "compile spinner should keep animation frames active"
        );
    }

    #[test]
    fn metal_seq_browser_instrument_tab_renders_visible_instrument_rows() {
        fn render_instrument_browser(cols: u16, rows: u16) -> String {
            let mut editor = browser_editor_on_instrument_tab();
            editor.set_active_buffer(browser_id(&editor));
            editor.set_layout_viewport(cols, rows);

            let layout = editor.widget_layout().expect("browser layout");
            render_layout_cells(&layout, cols, rows)
        }

        for (cols, rows) in [(32, 60), (220, 90)] {
            let rendered = render_instrument_browser(cols, rows);
            assert!(
                rendered.contains("emulations") || rendered.contains("strings"),
                "instrument tab should visibly render top-level instrument rows at {cols}x{rows}; rendered:\n{rendered}"
            );
        }
    }

    #[test]
    fn metal_seq_browser_samples_gap_is_stable_between_track_kinds() {
        fn sample_content_gap(
            editor: &mut eseqlisp::Editor,
            sidebar_kind: &str,
            track: f64,
        ) -> f32 {
            editor.runtime_mut().set_reactive(
                "SEQ",
                "sidebar-kind",
                Value::String(sidebar_kind.to_string()),
            );
            editor
                .runtime_mut()
                .set_reactive("SEQ", "sidebar-track-index", Value::Number(track));
            editor
                .runtime_mut()
                .eval_str("(set! sbrowser-tab \"samples\")")
                .expect("select samples tab");
            editor.refresh_runtime_side_effects();
            editor.set_active_buffer(browser_id(editor));
            editor.set_layout_viewport(72, 60);
            let layout = editor.widget_layout().expect("browser layout");
            let header = find_layout_node_by_stable_key(&layout, "browser-header")
                .expect("browser header node");
            let content = find_layout_node_by_stable_key(&layout, "browser-tabbed-content")
                .expect("browser tabbed content node");
            content.rect.row - (header.rect.row + header.rect.height)
        }

        let mut editor = browser_editor_on_instrument_tab();
        let sampler_gap = sample_content_gap(&mut editor, "sampler", 0.0);
        let instrument_gap = sample_content_gap(&mut editor, "instrument", 1.0);

        assert!(
            (sampler_gap - instrument_gap).abs() < 0.01,
            "samples header/content gap should not depend on selected track kind; sampler={sampler_gap}, instrument={instrument_gap}"
        );
    }

    #[test]
    fn metal_seq_browser_sample_tag_chips_have_visible_layout() {
        fn button_text(node: &eseqlisp::layout::LayoutNode) -> Option<&str> {
            if node.widget_type != "button" {
                return None;
            }
            match node.props.get("text") {
                Some(Value::String(text)) => Some(text.as_str()),
                _ => None,
            }
        }

        fn find_button_with_text<'a>(
            node: &'a eseqlisp::layout::LayoutNode,
            text: &str,
        ) -> Option<&'a eseqlisp::layout::LayoutNode> {
            if button_text(node) == Some(text) {
                return Some(node);
            }
            node.children
                .iter()
                .find_map(|child| find_button_with_text(child, text))
        }

        let mut editor = browser_editor_on_instrument_tab();
        editor
            .runtime_mut()
            .eval_str("(set! sbrowser-tab \"samples\")")
            .expect("select samples tab");
        editor.refresh_runtime_side_effects();
        editor.set_active_buffer(browser_id(&editor));
        editor.set_layout_viewport(72, 60);

        let layout = editor.widget_layout().expect("browser layout");
        let rendered = render_layout_cells(&layout, 72, 60);
        let tag_filter = find_layout_node_by_stable_key(&layout, "sample-tag-filter")
            .unwrap_or_else(|| panic!("sample tag filter should render; rendered:\n{rendered}"));
        let kick = find_button_with_text(&layout, "kick")
            .unwrap_or_else(|| panic!("kick tag chip should render; rendered:\n{rendered}"));

        assert!(
            tag_filter.rect.width > 1.0 && tag_filter.rect.height > 0.4,
            "tag filter should have a finite visible rect: {:?}; rendered:\n{rendered}",
            tag_filter.rect
        );
        assert!(
            kick.rect.width > 1.0 && kick.rect.height > 0.4,
            "kick tag chip should have a finite visible rect: {:?}; rendered:\n{rendered}",
            kick.rect
        );
        assert!(
            kick.rect.height <= 0.95,
            "tag chips should stay visually smaller than regular browser buttons: {:?}; rendered:\n{rendered}",
            kick.rect
        );
        assert!(
            matches!(kick.props.get("background-color"), Some(Value::String(color)) if color == "#f0f0f2"),
            "selected tag chips should use the high-contrast selected sample chip color"
        );
    }

    #[test]
    fn metal_seq_browser_projects_tab_renders_visible_new_project_button() {
        fn node_text(node: &eseqlisp::layout::LayoutNode) -> Option<&str> {
            match node.props.get("text") {
                Some(Value::String(text)) => Some(text.as_str()),
                _ => None,
            }
        }

        fn find_button_by_text<'a>(
            node: &'a eseqlisp::layout::LayoutNode,
            text: &str,
        ) -> Option<&'a eseqlisp::layout::LayoutNode> {
            if node.widget_type == "button" && node_text(node) == Some(text) {
                return Some(node);
            }
            node.children
                .iter()
                .find_map(|child| find_button_by_text(child, text))
        }

        let mut editor = browser_editor_on_instrument_tab();
        editor
            .runtime_mut()
            .eval_str("(set! sbrowser-tab \"projects\")")
            .expect("select projects tab");
        editor.refresh_runtime_side_effects();
        editor.set_active_buffer(browser_id(&editor));
        editor.set_layout_viewport(72, 60);

        let layout = editor.widget_layout().expect("browser layout");
        let rendered = render_layout_cells(&layout, 72, 60);
        let button = find_button_by_text(&layout, "New Project")
            .unwrap_or_else(|| panic!("new project button layout node; rendered:\n{rendered}"));

        assert!(
            button.rect.width > 1.0
                && button.rect.height > 0.4
                && button.rect.col >= 0.0
                && button.rect.row >= 0.0
                && button.rect.col + button.rect.width <= 72.0
                && button.rect.row + button.rect.height <= 60.0,
            "new project button should have a finite visible rect, got {:?}; rendered:\n{rendered}",
            button.rect
        );
        assert!(
            rendered.contains("New Project"),
            "projects tab should visibly render the New button; rendered:\n{rendered}"
        );
    }

    #[test]
    fn metal_seq_browser_render_does_not_mutate_sample_search_without_track_change() {
        let mut editor = browser_editor_on_instrument_tab();
        editor
            .runtime_mut()
            .eval_str("(set! sbrowser-tab \"samples\")")
            .expect("select samples tab");
        editor
            .runtime_mut()
            .eval_str("(sbrowser-build-widgets)")
            .expect("sync initial sampler track");
        editor
            .runtime_mut()
            .eval_str("(set! sbrowser-filter \"kick\")")
            .expect("set sample search");
        editor.refresh_runtime_side_effects();
        editor.set_active_buffer(browser_id(&editor));
        editor.set_layout_viewport(72, 60);
        let _ = editor.widget_layout().expect("browser layout");
        editor
            .runtime_mut()
            .eval_str("(sbrowser-build-widgets)")
            .expect("build browser widgets");

        assert_eq!(
            editor.runtime_mut().eval_str("sbrowser-filter"),
            Ok(Some(Value::String("kick".to_string()))),
            "rendering the browser should not clear the search filter as a side effect"
        );
    }

    #[test]
    fn metal_seq_browser_track_sample_sync_clears_sample_search() {
        let mut editor = browser_editor_on_instrument_tab();
        editor
            .runtime_mut()
            .eval_str("(set! sbrowser-tab \"samples\")")
            .expect("select samples tab");
        editor
            .runtime_mut()
            .eval_str("(sbrowser-build-widgets)")
            .expect("sync initial sampler track");
        editor
            .runtime_mut()
            .eval_str("(set! sbrowser-filter \"snare\")")
            .expect("set sample search");
        editor
            .runtime_mut()
            .set_reactive("SEQ", "sidebar-track-index", Value::Number(1.0));
        editor.runtime_mut().set_reactive(
            "SEQ",
            "sidebar-selected-sample",
            Value::String("samples/loaded-a.wav".to_string()),
        );

        editor
            .runtime_mut()
            .eval_str("(sbrowser-build-widgets)")
            .expect("sync switched sampler track");

        assert_eq!(
            editor.runtime_mut().eval_str("sbrowser-filter"),
            Ok(Some(Value::String(String::new()))),
            "switching sampler tracks should clear the sample search"
        );
        assert_eq!(
            editor
                .runtime_mut()
                .eval_str("(len sbrowser-selected-tags)"),
            Ok(Some(Value::Number(2.0))),
            "switching sampler tracks should load the selected sample's tags"
        );
    }

    #[test]
    fn metal_seq_browser_audition_preserves_sample_filter_context() {
        let mut editor = browser_editor_on_instrument_tab();
        editor
            .runtime_mut()
            .eval_str("(set! sbrowser-tab \"samples\")")
            .expect("select samples tab");
        editor
            .runtime_mut()
            .eval_str("(sbrowser-build-widgets)")
            .expect("sync initial sampler track");
        editor
            .runtime_mut()
            .eval_str("(set! sbrowser-filter \"break\")")
            .expect("set sample search");
        editor
            .runtime_mut()
            .eval_str("(set! sbrowser-selected-tags (list \"kick\" \"808\"))")
            .expect("seed selected tags");
        editor
            .runtime_mut()
            .eval_str(
                r#"(sbrowser-audition
                    (dict :label "audition.wav" :path "samples/audition.wav"))"#,
            )
            .expect("audition sample");
        editor.runtime_mut().set_reactive(
            "SEQ",
            "sidebar-selected-sample",
            Value::String("samples/audition.wav".to_string()),
        );

        editor
            .runtime_mut()
            .eval_str("(sbrowser-build-widgets)")
            .expect("sync auditioned sampler sample");

        assert_eq!(
            editor.runtime_mut().eval_str("sbrowser-filter"),
            Ok(Some(Value::String("break".to_string()))),
            "auditioning a sample should preserve the active sample search"
        );
        assert_eq!(
            editor
                .runtime_mut()
                .eval_str("(len sbrowser-selected-tags)"),
            Ok(Some(Value::Number(2.0))),
            "auditioning a sample should preserve selected tag filters"
        );
        assert_eq!(
            editor
                .runtime_mut()
                .eval_str("(sbrowser-list-contains? sbrowser-selected-tags \"kick\")"),
            Ok(Some(Value::Bool(true))),
            "auditioning a sample should not replace selected tags with that sample's tags"
        );
    }

    #[test]
    fn metal_seq_browser_browser_initiated_new_track_preserves_sample_filter_context() {
        let mut editor = browser_editor_on_instrument_tab();
        editor
            .runtime_mut()
            .eval_str("(set! sbrowser-tab \"samples\")")
            .expect("select samples tab");
        editor
            .runtime_mut()
            .eval_str("(sbrowser-build-widgets)")
            .expect("sync initial sampler track");
        editor
            .runtime_mut()
            .eval_str("(set! sbrowser-filter \"break\")")
            .expect("set sample search");
        editor
            .runtime_mut()
            .eval_str("(set! sbrowser-selected-tags (list \"kick\" \"808\"))")
            .expect("seed selected tags");
        editor
            .runtime_mut()
            .eval_str("(set! sbrowser-auditioned-sample \"samples/new-track.wav\")")
            .expect("mark browser initiated sample load");
        editor
            .runtime_mut()
            .set_reactive("SEQ", "sidebar-track-index", Value::Number(1.0));
        editor.runtime_mut().set_reactive(
            "SEQ",
            "sidebar-selected-sample",
            Value::String("samples/new-track.wav".to_string()),
        );

        editor
            .runtime_mut()
            .eval_str("(sbrowser-build-widgets)")
            .expect("sync browser-created sampler track");

        assert_eq!(
            editor.runtime_mut().eval_str("sbrowser-filter"),
            Ok(Some(Value::String("break".to_string()))),
            "browser-initiated new sampler tracks should preserve sample search"
        );
        assert_eq!(
            editor
                .runtime_mut()
                .eval_str("(len sbrowser-selected-tags)"),
            Ok(Some(Value::Number(2.0))),
            "browser-initiated new sampler tracks should preserve selected tag filters"
        );
    }

    #[test]
    fn metal_seq_browser_search_typing_clears_selected_sample_tags() {
        fn find_widget_type<'a>(
            node: &'a eseqlisp::layout::LayoutNode,
            widget_type: &str,
        ) -> Option<&'a eseqlisp::layout::LayoutNode> {
            if node.widget_type == widget_type {
                return Some(node);
            }
            node.children
                .iter()
                .find_map(|child| find_widget_type(child, widget_type))
        }

        let mut editor = browser_editor_on_instrument_tab();
        editor
            .runtime_mut()
            .eval_str("(set! sbrowser-tab \"samples\")")
            .expect("select samples tab");
        editor
            .runtime_mut()
            .eval_str("(set! sbrowser-selected-tags (list \"kick\" \"808\"))")
            .expect("seed selected tags");
        editor.refresh_runtime_side_effects();
        editor.set_active_buffer(browser_id(&editor));
        editor.set_layout_viewport(72, 60);

        let layout = editor.widget_layout().expect("browser layout");
        let rendered = render_layout_cells(&layout, 72, 60);
        let header = find_layout_node_by_stable_key(&layout, "browser-header")
            .unwrap_or_else(|| panic!("browser header should render; rendered:\n{rendered}"));
        let input = find_widget_type(header, "text-input").unwrap_or_else(|| {
            panic!("browser header text input should render; rendered:\n{rendered}")
        });
        let on_change = input
            .props
            .get("on-change")
            .cloned()
            .expect("browser search input should expose on-change");

        editor
            .runtime_mut()
            .invoke(on_change, vec![Value::String("snare".to_string())])
            .expect("invoke browser search on-change");

        assert_eq!(
            editor.runtime_mut().eval_str("sbrowser-filter"),
            Ok(Some(Value::String("snare".to_string()))),
            "typing in sample search should update the search text"
        );
        assert_eq!(
            editor
                .runtime_mut()
                .eval_str("(len sbrowser-selected-tags)"),
            Ok(Some(Value::Number(0.0))),
            "typing in sample search should clear selected tag filters"
        );
    }

    #[test]
    fn metal_seq_browser_search_keeps_focus_for_consecutive_typing() {
        let mut editor = browser_editor_on_instrument_tab();
        editor
            .runtime_mut()
            .eval_str("(set! sbrowser-tab \"samples\")")
            .expect("select samples tab");
        editor.refresh_runtime_side_effects();
        editor.set_active_buffer(browser_id(&editor));
        editor.set_layout_viewport(72, 60);

        let layout = editor.widget_layout().expect("browser layout");
        let input = find_layout_node_by_stable_key(&layout, "sbrowser-search-input")
            .expect("browser search input");
        let click_col = input.rect.col + 1.0;
        let click_row = input.rect.row + input.rect.height * 0.5;
        editor.handle_mouse_precise(
            crossterm::event::MouseEvent {
                kind: crossterm::event::MouseEventKind::Down(crossterm::event::MouseButton::Left),
                column: click_col as u16,
                row: click_row as u16,
                modifiers: crossterm::event::KeyModifiers::NONE,
            },
            0,
            0,
            72,
            60,
            click_col,
            click_row,
        );
        editor.handle_mouse_precise(
            crossterm::event::MouseEvent {
                kind: crossterm::event::MouseEventKind::Up(crossterm::event::MouseButton::Left),
                column: click_col as u16,
                row: click_row as u16,
                modifiers: crossterm::event::KeyModifiers::NONE,
            },
            0,
            0,
            72,
            60,
            click_col,
            click_row,
        );
        let focused_before = editor
            .focused_widget_id()
            .expect("browser search input should focus");
        let state = Arc::new(SequencerState::new(1, vec![]));
        let current_track = Arc::new(AtomicUsize::new(0));
        let selected_steps = Arc::new(Mutex::new(HashSet::new()));
        let step_clipboard = Arc::new(Mutex::new(None));
        assert!(
            focused_widget_captures_text_input(&editor),
            "browser search input should capture text after click"
        );

        let p_key = crossterm::event::KeyEvent::new(
            crossterm::event::KeyCode::Char('p'),
            crossterm::event::KeyModifiers::NONE,
        );
        assert!(
            !handle_metal_command_shortcut(
                &mut editor,
                &p_key,
                &state,
                &current_track,
                &selected_steps,
                &step_clipboard,
            ),
            "plain text input key should not be handled as a global shortcut"
        );
        editor.handle_key(p_key);
        assert!(
            focused_widget_captures_text_input(&editor),
            "browser search input should still capture text after first edit before the next frame"
        );
        let i_key = crossterm::event::KeyEvent::new(
            crossterm::event::KeyCode::Char('i'),
            crossterm::event::KeyModifiers::NONE,
        );
        assert!(
            !handle_metal_command_shortcut(
                &mut editor,
                &i_key,
                &state,
                &current_track,
                &selected_steps,
                &step_clipboard,
            ),
            "second plain text input key should not be handled as a global shortcut"
        );
        editor.handle_key(i_key);
        let _ = eseqlisp::frame::build_tiled_render_frame_borderless(&mut editor, 72, 60);

        assert_eq!(
            editor.runtime_mut().eval_str("sbrowser-filter"),
            Ok(Some(Value::String("pi".to_string()))),
            "browser search should accept consecutive keypresses"
        );
        assert_eq!(
            editor.focused_widget_id(),
            Some(focused_before),
            "browser search should keep focus while typing"
        );

        refresh_sample_browser_buffer(&mut editor).expect("delayed sample browser refresh");
        assert!(
            focused_widget_captures_text_input(&editor),
            "browser search input should still capture text after delayed results refresh"
        );
        editor.handle_key(crossterm::event::KeyEvent::new(
            crossterm::event::KeyCode::Char('a'),
            crossterm::event::KeyModifiers::NONE,
        ));
        assert_eq!(
            editor.runtime_mut().eval_str("sbrowser-filter"),
            Ok(Some(Value::String("pia".to_string()))),
            "browser search should keep accepting text after delayed refresh"
        );
    }

    #[test]
    fn metal_seq_tiled_browser_search_keeps_focus_for_consecutive_typing() {
        let mut editor = full_grid_editor_for_scroll_tests();
        editor
            .runtime_mut()
            .eval_str("(set! sbrowser-tab \"samples\")")
            .expect("select samples tab");
        editor.refresh_runtime_side_effects();

        let frame = eseqlisp::frame::build_tiled_render_frame_borderless(&mut editor, 180, 90);
        let samples_tile = frame
            .tiles
            .iter()
            .find(|tile| tile.frame.buffer_name == "*samples*")
            .expect("full grid should show samples tile");
        let samples_layout = samples_tile
            .frame
            .widget_layout
            .as_deref()
            .expect("samples tile widget layout");
        let input = find_layout_node_by_stable_key(samples_layout, "sbrowser-search-input")
            .expect("browser search input");
        let click_col = samples_tile.rect.col + input.rect.col + 1.0;
        let click_row = samples_tile.rect.row + input.rect.row + input.rect.height * 0.5;

        for kind in [
            crossterm::event::MouseEventKind::Down(crossterm::event::MouseButton::Left),
            crossterm::event::MouseEventKind::Up(crossterm::event::MouseButton::Left),
        ] {
            editor.handle_tiled_mouse_precise(
                crossterm::event::MouseEvent {
                    kind,
                    column: click_col as u16,
                    row: click_row as u16,
                    modifiers: crossterm::event::KeyModifiers::NONE,
                },
                click_col,
                click_row,
                0,
            );
        }

        assert_eq!(
            editor.active_buffer().name,
            "*samples*",
            "clicking browser search should make the samples tile active"
        );
        let focused_before = editor
            .focused_widget_id()
            .expect("browser search input should focus");
        assert!(
            focused_widget_captures_text_input(&editor),
            "browser search input should capture text after tiled click"
        );

        editor.handle_key(crossterm::event::KeyEvent::new(
            crossterm::event::KeyCode::Char('p'),
            crossterm::event::KeyModifiers::NONE,
        ));
        assert_eq!(
            editor.active_buffer().name,
            "*samples*",
            "typing in browser search should keep the samples tile active before the next frame"
        );
        assert!(
            focused_widget_captures_text_input(&editor),
            "browser search input should still capture text after the first tiled edit before the next frame"
        );
        editor.handle_key(crossterm::event::KeyEvent::new(
            crossterm::event::KeyCode::Char('i'),
            crossterm::event::KeyModifiers::NONE,
        ));
        let _ = eseqlisp::frame::build_tiled_render_frame_borderless(&mut editor, 180, 90);

        assert_eq!(
            editor.runtime_mut().eval_str("sbrowser-filter"),
            Ok(Some(Value::String("pi".to_string()))),
            "tiled browser search should accept consecutive keypresses"
        );
        assert_eq!(
            editor.focused_widget_id(),
            Some(focused_before),
            "tiled browser search should keep focus while typing"
        );

        refresh_sample_browser_buffer(&mut editor).expect("delayed sample browser refresh");
        assert!(
            focused_widget_captures_text_input(&editor),
            "tiled browser search input should still capture text after delayed results refresh"
        );
        editor.handle_key(crossterm::event::KeyEvent::new(
            crossterm::event::KeyCode::Char('a'),
            crossterm::event::KeyModifiers::NONE,
        ));
        assert_eq!(
            editor.runtime_mut().eval_str("sbrowser-filter"),
            Ok(Some(Value::String("pia".to_string()))),
            "tiled browser search should keep accepting text after delayed refresh"
        );
    }

    #[test]
    fn metal_seq_browser_new_project_button_queues_host_command() {
        let mut editor = browser_editor_on_instrument_tab();
        editor
            .runtime_mut()
            .eval_str("(sbrowser-new-project)")
            .expect("invoke new project action");

        let commands = editor.drain_host_commands();
        assert_eq!(commands.len(), 1);
        match &commands[0] {
            eseqlisp::host::HostCommand::Custom { name, payload } => {
                assert_eq!(name, "new-project");
                assert!(
                    matches!(payload, Value::Map(map) if map.is_empty()),
                    "new project payload should be an empty dict: {payload:?}"
                );
            }
            other => panic!("expected new-project host command, got {other:?}"),
        }
        assert_eq!(
            editor
                .runtime_mut()
                .eval_str("sbrowser-tab")
                .expect("read browser tab"),
            Some(Value::String("projects".to_string()))
        );
    }

    #[test]
    fn metal_seq_browser_sample_click_only_updates_browser_selection() {
        let mut editor = browser_editor_on_instrument_tab();
        editor
            .runtime_mut()
            .set_reactive("SEQ", "num-tracks", Value::Number(1.0));
        editor.runtime_mut().set_reactive(
            "SEQ",
            "sidebar-kind",
            Value::String("sampler".to_string()),
        );
        editor
            .runtime_mut()
            .eval_str(
                r#"(sbrowser-select-sample
                    (dict :label "kick.wav" :path "samples/kick.wav"))"#,
            )
            .expect("select sample");

        assert!(
            editor.drain_host_commands().is_empty(),
            "sample click should not audition or add a track"
        );
        assert_eq!(
            editor
                .runtime_mut()
                .eval_str("sbrowser-selected-sample")
                .expect("read selected sample"),
            Some(Value::String("samples/kick.wav".to_string()))
        );
    }

    #[test]
    fn metal_seq_browser_syncs_selected_sample_from_sampler_track_changes() {
        let mut editor = browser_editor_on_instrument_tab();
        editor
            .runtime_mut()
            .eval_str("(set! sbrowser-tab \"samples\")")
            .expect("select samples tab");
        editor
            .runtime_mut()
            .set_reactive("SEQ", "num-tracks", Value::Number(2.0));
        editor.runtime_mut().set_reactive(
            "SEQ",
            "sidebar-kind",
            Value::String("sampler".to_string()),
        );
        editor.runtime_mut().set_reactive(
            "SEQ",
            "sidebar-selected-sample",
            Value::String("samples/loaded-a.wav".to_string()),
        );
        editor
            .runtime_mut()
            .eval_str("(sbrowser-build-widgets)")
            .expect("sync initial sampler sample");
        assert_eq!(
            editor
                .runtime_mut()
                .eval_str("sbrowser-selected-sample")
                .expect("read selected sample"),
            Some(Value::String("samples/loaded-a.wav".to_string()))
        );

        editor
            .runtime_mut()
            .eval_str(
                r#"(sbrowser-select-sample
                    (dict :label "browse.wav" :path "samples/browse.wav"))"#,
            )
            .expect("select browsed sample");
        editor
            .runtime_mut()
            .eval_str("(sbrowser-build-widgets)")
            .expect("render without host sample change");
        assert_eq!(
            editor
                .runtime_mut()
                .eval_str("sbrowser-selected-sample")
                .expect("read browsed sample"),
            Some(Value::String("samples/browse.wav".to_string())),
            "local browsing selection should survive renders until the loaded sample changes"
        );

        editor
            .runtime_mut()
            .set_reactive("SEQ", "sidebar-track-index", Value::Number(1.0));
        editor.runtime_mut().set_reactive(
            "SEQ",
            "sidebar-selected-sample",
            Value::String("samples/loaded-b.wav".to_string()),
        );
        editor
            .runtime_mut()
            .eval_str("(sbrowser-build-widgets)")
            .expect("sync switched sampler sample");
        assert_eq!(
            editor
                .runtime_mut()
                .eval_str("sbrowser-selected-sample")
                .expect("read switched selected sample"),
            Some(Value::String("samples/loaded-b.wav".to_string()))
        );
    }

    #[test]
    fn metal_seq_browser_sample_activation_rejects_current_instrument_track() {
        let mut editor = browser_editor_on_instrument_tab();
        editor
            .runtime_mut()
            .set_reactive("SEQ", "num-tracks", Value::Number(1.0));
        editor.runtime_mut().set_reactive(
            "SEQ",
            "sidebar-kind",
            Value::String("instrument".to_string()),
        );
        editor
            .runtime_mut()
            .eval_str(
                r#"(sbrowser-activate-sample
                    (dict :label "kick.wav" :path "samples/kick.wav"))"#,
            )
            .expect("activate sample from instrument context");

        assert!(
            editor.drain_host_commands().is_empty(),
            "activating a sample on an instrument track should not create a track"
        );
        assert_eq!(
            editor.runtime_mut().take_status_message(),
            Some("Drop samples onto a sampler track or the new-track drop zone".to_string())
        );
    }

    #[test]
    fn metal_seq_browser_sample_activation_auditions_sampler_track() {
        let mut editor = browser_editor_on_instrument_tab();
        editor
            .runtime_mut()
            .set_reactive("SEQ", "num-tracks", Value::Number(1.0));
        editor.runtime_mut().set_reactive(
            "SEQ",
            "sidebar-kind",
            Value::String("sampler".to_string()),
        );
        editor
            .runtime_mut()
            .eval_str(
                r#"(sbrowser-activate-sample
                    (dict :label "kick.wav" :path "samples/kick.wav"))"#,
            )
            .expect("activate sample on sampler track");

        let commands = editor.drain_host_commands();
        assert_eq!(commands.len(), 1);
        match &commands[0] {
            eseqlisp::host::HostCommand::Custom { name, payload } => {
                assert_eq!(name, "audition-sample");
                let Value::Map(payload) = payload else {
                    panic!("sample audition payload should be a dict: {payload:?}");
                };
                assert_eq!(
                    payload.get("path").map(|value| value.borrow().clone()),
                    Some(Value::String("samples/kick.wav".to_string()))
                );
            }
            other => panic!("expected audition-sample host command, got {other:?}"),
        }
    }

    #[test]
    fn metal_seq_browser_sampler_button_queues_blank_sampler_track() {
        let mut editor = browser_editor_on_instrument_tab();
        editor
            .runtime_mut()
            .eval_str("(sbrowser-add-sampler-track)")
            .expect("invoke sampler add action");

        let commands = editor.drain_host_commands();
        assert_eq!(commands.len(), 1);
        match &commands[0] {
            eseqlisp::host::HostCommand::Custom { name, payload } => {
                assert_eq!(name, "add-track-sampler");
                assert!(
                    matches!(payload, Value::Map(map) if map.is_empty()),
                    "blank sampler payload should be an empty dict: {payload:?}"
                );
                assert_eq!(
                    editor
                        .runtime_mut()
                        .eval_str("sbrowser-tab")
                        .expect("read browser tab"),
                    Some(Value::String("samples".to_string()))
                );
            }
            other => panic!("expected add-track-sampler host command, got {other:?}"),
        }
    }

    #[test]
    fn metal_seq_browser_instrument_selection_queues_track_and_switches_to_presets() {
        let mut editor = browser_editor_on_instrument_tab();
        editor
            .runtime_mut()
            .eval_str(
                r#"(sbrowser-select-create-item
                    (dict :kind "instrument" :name "emulations/digitone" :label "digitone"))"#,
            )
            .expect("select instrument");

        let commands = editor.drain_host_commands();
        assert_eq!(commands.len(), 1);
        match &commands[0] {
            eseqlisp::host::HostCommand::Custom { name, payload } => {
                assert_eq!(name, "add-track-instrument");
                let Value::Map(payload) = payload else {
                    panic!("instrument add payload should be a dict: {payload:?}");
                };
                assert_eq!(
                    payload.get("name").map(|value| value.borrow().clone()),
                    Some(Value::String("emulations/digitone".to_string()))
                );
            }
            other => panic!("expected add-track-instrument host command, got {other:?}"),
        }
        assert_eq!(
            editor
                .runtime_mut()
                .eval_str("sbrowser-tab")
                .expect("read browser tab"),
            Some(Value::String("presets".to_string()))
        );
    }

    #[test]
    fn metal_seq_browser_audio_effect_click_only_updates_browser_selection() {
        let mut editor = browser_editor_on_instrument_tab();
        editor
            .runtime_mut()
            .eval_str(
                r#"(sbrowser-select-audio-effect
                    (dict :kind "builtin-audio-effect" :name "Filter" :label "Filter"))"#,
            )
            .expect("select built-in audio effect");

        assert!(
            editor.drain_host_commands().is_empty(),
            "audio effect click should not add an effect"
        );
        assert_eq!(
            editor.runtime_mut().take_status_message(),
            Some("Filter".to_string())
        );
    }

    #[test]
    fn metal_seq_browser_audio_effect_activation_uses_selected_bus() {
        let mut editor = browser_editor_on_instrument_tab();
        editor
            .runtime_mut()
            .eval_str("(set! selected-bus 1)")
            .expect("select bus");
        editor
            .runtime_mut()
            .eval_str(
                r#"(sbrowser-activate-audio-effect
                    (dict :kind "builtin-audio-effect" :name "Filter" :label "Filter"))"#,
            )
            .expect("activate built-in audio effect for bus");

        let commands = editor.drain_host_commands();
        assert_eq!(commands.len(), 1);
        match &commands[0] {
            eseqlisp::host::HostCommand::Custom { name, payload } => {
                assert_eq!(name, "add-builtin-bus-effect");
                let Value::Map(payload) = payload else {
                    panic!("bus effect payload should be a dict: {payload:?}");
                };
                assert_eq!(
                    payload.get("bus").map(|value| value.borrow().clone()),
                    Some(Value::Number(1.0))
                );
                assert_eq!(
                    payload.get("name").map(|value| value.borrow().clone()),
                    Some(Value::String("Filter".to_string()))
                );
            }
            other => panic!("expected add-builtin-bus-effect host command, got {other:?}"),
        }
    }

    #[test]
    fn metal_seq_browser_audio_effect_activation_uses_track_when_no_bus_selected() {
        let mut editor = browser_editor_on_instrument_tab();
        editor
            .runtime_mut()
            .eval_str("(set! selected-bus -1)")
            .expect("clear selected bus");
        editor
            .runtime_mut()
            .eval_str(
                r#"(sbrowser-activate-audio-effect
                    (dict :kind "custom-audio-effect" :name "my-effect" :label "my-effect"))"#,
            )
            .expect("activate custom audio effect for track");

        let commands = editor.drain_host_commands();
        assert_eq!(commands.len(), 1);
        match &commands[0] {
            eseqlisp::host::HostCommand::Custom { name, payload } => {
                assert_eq!(name, "add-effect");
                let Value::Map(payload) = payload else {
                    panic!("track effect payload should be a dict: {payload:?}");
                };
                assert_eq!(
                    payload.get("name").map(|value| value.borrow().clone()),
                    Some(Value::String("my-effect".to_string()))
                );
            }
            other => panic!("expected add-effect host command, got {other:?}"),
        }
    }

    #[test]
    fn metal_seq_browser_midi_effect_click_only_updates_browser_selection() {
        let mut editor = browser_editor_on_instrument_tab();
        editor
            .runtime_mut()
            .eval_str(
                r#"(sbrowser-select-midi-effect
                    (dict :kind "midi-effect" :name "Arp" :label "Arp"))"#,
            )
            .expect("select MIDI effect");

        assert!(
            editor.drain_host_commands().is_empty(),
            "MIDI effect click should not add an effect"
        );
        assert_eq!(
            editor.runtime_mut().take_status_message(),
            Some("Arp".to_string())
        );
    }

    #[test]
    fn metal_seq_browser_midi_effect_activation_adds_midi_effect() {
        let mut editor = browser_editor_on_instrument_tab();
        editor
            .runtime_mut()
            .eval_str(
                r#"(sbrowser-activate-midi-effect
                    (dict :kind "midi-effect" :name "Arp" :label "Arp"))"#,
            )
            .expect("activate MIDI effect");

        let commands = editor.drain_host_commands();
        assert_eq!(commands.len(), 1);
        match &commands[0] {
            eseqlisp::host::HostCommand::Custom { name, payload } => {
                assert_eq!(name, "add-midi-fx");
                let Value::Map(payload) = payload else {
                    panic!("MIDI effect payload should be a dict: {payload:?}");
                };
                assert_eq!(
                    payload.get("name").map(|value| value.borrow().clone()),
                    Some(Value::String("Arp".to_string()))
                );
            }
            other => panic!("expected add-midi-fx host command, got {other:?}"),
        }
    }

    #[test]
    fn metal_seq_browser_instrument_tab_diagnoses_side_by_side_scroll_collapse() {
        fn apply_browser_body(editor: &mut eseqlisp::Editor, body: &str) {
            editor
                .runtime_mut()
                .eval_str(&format!(
                    r#"
                    (effect-buffer "*samples*"
                      (v-stack :width :fill :gap 0.4 :padding 1.2
                        {body}))
                    "#
                ))
                .expect("apply diagnostic browser body");
            editor.refresh_runtime_side_effects();
            if let Some(status) = editor.runtime_mut().take_status_message() {
                panic!("diagnostic browser status after refresh: {status}");
            }
            editor.set_active_buffer(browser_id(editor));
        }

        fn snapshot(body: &str) -> (eseqlisp::layout::Rect, eseqlisp::layout::Rect, bool, String) {
            let mut editor = browser_editor_on_instrument_tab();
            apply_browser_body(&mut editor, body);
            editor.set_layout_viewport(220, 90);
            let layout = editor.widget_layout().expect("diagnostic browser layout");
            let scroll = find_layout_node_by_stable_key(&layout, "instruments-tab-scroll")
                .expect("instrument scroll layout node")
                .rect;
            let tree = find_layout_node_by_stable_key(&layout, "instruments-tab-tree")
                .expect("instrument tree layout node")
                .rect;
            let rendered = render_layout_cells(&layout, 220, 90);
            let has_instrument_rows =
                rendered.contains("emulations") || rendered.contains("strings");
            (scroll, tree, has_instrument_rows, rendered)
        }

        let good_body = r#"
          (list
            (sbrowser-header)
            (sbrowser-tabs)
            (sbrowser-active-tab-panel))
        "#;
        let side_by_side_body = r#"
          (list
            (sbrowser-header)
            (h-stack :key "diagnostic-side-by-side" :width :fill :gap 0.5 :flex 1
              (sbrowser-tabs)
              (sbrowser-active-tab-panel)))
        "#;
        let stretched_side_by_side_body = r#"
          (list
            (sbrowser-header)
            (h-stack :key "diagnostic-side-by-side-stretched" :width :fill :gap 0.5 :flex 1 :align :stretch
              (sbrowser-tabs)
              (sbrowser-active-tab-panel)))
        "#;
        let fixed_side_by_side_body = r#"
          (list
            (sbrowser-header)
            (h-stack :key "diagnostic-side-by-side-fixed" :width :fill :gap 0.5 :flex 1 :align :stretch
              (sbrowser-tabs)
              (box :key "diagnostic-active-tab-panel" :width 0 :flex 1 :padding 0
                (sbrowser-active-tab-panel))))
        "#;

        let (good_scroll, good_tree, good_rows, _) = snapshot(good_body);
        let (side_scroll, side_tree, side_rows, side_rendered) = snapshot(side_by_side_body);
        let (stretched_scroll, stretched_tree, stretched_rows, stretched_rendered) =
            snapshot(stretched_side_by_side_body);
        let (fixed_scroll, fixed_tree, fixed_rows, fixed_rendered) =
            snapshot(fixed_side_by_side_body);

        eprintln!(
            "good scroll={:?} tree={:?} rows={}; side-by-side scroll={:?} tree={:?} rows={}; stretched side-by-side scroll={:?} tree={:?} rows={}; fixed side-by-side scroll={:?} tree={:?} rows={}",
            good_scroll,
            good_tree,
            good_rows,
            side_scroll,
            side_tree,
            side_rows,
            stretched_scroll,
            stretched_tree,
            stretched_rows,
            fixed_scroll,
            fixed_tree,
            fixed_rows
        );

        assert!(
            good_scroll.height > 10.0,
            "known-good layout should give instrument scroll visible height, got {good_scroll:?}"
        );
        assert!(
            side_scroll.height < 0.01,
            "side-by-side diagnostic should reproduce collapsed instrument scroll height; side scroll={side_scroll:?}, rendered:\n{side_rendered}"
        );
        assert!(good_rows, "known-good layout should render instrument rows");
        assert!(
            side_rows,
            "the generic cell renderer still sees rows even though the scroll viewport is collapsed; rendered:\n{side_rendered}"
        );
        assert!(
            stretched_scroll.height > 10.0,
            "stretch-aligned side-by-side layout should give instrument scroll visible height; scroll={stretched_scroll:?}, rendered:\n{stretched_rendered}"
        );
        assert!(
            stretched_rows,
            "stretch-aligned side-by-side layout should render instrument rows; rendered:\n{stretched_rendered}"
        );
        assert!(
            stretched_scroll.col + stretched_scroll.width <= 220.0,
            "stretch-aligned side-by-side layout should keep instrument scroll inside viewport; scroll={stretched_scroll:?}"
        );
        assert!(
            fixed_scroll.height > 10.0,
            "fixed side-by-side layout should give instrument scroll visible height; scroll={fixed_scroll:?}, rendered:\n{fixed_rendered}"
        );
        assert!(
            fixed_scroll.col + fixed_scroll.width <= 220.0,
            "fixed side-by-side layout should keep instrument scroll inside viewport; scroll={fixed_scroll:?}, rendered:\n{fixed_rendered}"
        );
        assert!(
            fixed_rows,
            "fixed side-by-side layout should render instrument rows; rendered:\n{fixed_rendered}"
        );
    }

    #[test]
    fn metal_seq_mixer_lisp_parses() {
        let src = std::fs::read_to_string("metal-seq-mixer-v2.lisp").expect("read mixer lisp");
        let tokens = Parser::new(src)
            .parse()
            .expect("tokenize metal-seq-mixer-v2.lisp");
        let mut pos = 0;
        while pos < tokens.len() {
            if let Err(err) = parse_expression_at(&tokens, &mut pos) {
                let start = pos.saturating_sub(8);
                let end = (pos + 8).min(tokens.len());
                panic!(
                    "parse metal-seq-mixer-v2.lisp at token {pos}: {err:?}\ncontext: {:?}",
                    &tokens[start..end]
                );
            }
        }
        ASTParser::new(tokens)
            .parse()
            .expect("parse metal-seq-mixer-v2.lisp");
    }

    #[test]
    fn metal_seq_fx_lisp_parses() {
        let src = std::fs::read_to_string("metal-seq-fx.lisp").expect("read fx lisp");
        let tokens = Parser::new(src)
            .parse()
            .expect("tokenize metal-seq-fx.lisp");
        let mut pos = 0;
        while pos < tokens.len() {
            if let Err(err) = parse_expression_at(&tokens, &mut pos) {
                let start = pos.saturating_sub(8);
                let end = (pos + 8).min(tokens.len());
                panic!(
                    "parse metal-seq-fx.lisp at token {pos}: {err:?}\ncontext: {:?}",
                    &tokens[start..end]
                );
            }
        }
        ASTParser::new(tokens)
            .parse()
            .expect("parse metal-seq-fx.lisp");
    }

    #[test]
    fn metal_seq_metal_lisp_parses() {
        let src = std::fs::read_to_string("metal-seq-metal.lisp").expect("read metal lisp");
        let tokens = Parser::new(src)
            .parse()
            .expect("tokenize metal-seq-metal.lisp");
        let mut pos = 0;
        while pos < tokens.len() {
            if let Err(err) = parse_expression_at(&tokens, &mut pos) {
                let start = pos.saturating_sub(8);
                let end = (pos + 8).min(tokens.len());
                panic!(
                    "parse metal-seq-metal.lisp at token {pos}: {err:?}\ncontext: {:?}",
                    &tokens[start..end]
                );
            }
        }
        ASTParser::new(tokens)
            .parse()
            .expect("parse metal-seq-metal.lisp");
    }

    #[test]
    fn metal_seq_sequencer_lisp_parses() {
        let src = std::fs::read_to_string("metal-seq-sequencer.lisp").expect("read sequencer lisp");
        let tokens = Parser::new(src)
            .parse()
            .expect("tokenize metal-seq-sequencer.lisp");
        let mut pos = 0;
        while pos < tokens.len() {
            if let Err(err) = parse_expression_at(&tokens, &mut pos) {
                let start = pos.saturating_sub(8);
                let end = (pos + 8).min(tokens.len());
                panic!(
                    "parse metal-seq-sequencer.lisp at token {pos}: {err:?}\ncontext: {:?}",
                    &tokens[start..end]
                );
            }
        }
        ASTParser::new(tokens)
            .parse()
            .expect("parse metal-seq-sequencer.lisp");
    }

    fn test_list(values: Vec<Value>) -> Value {
        Value::List(
            values
                .into_iter()
                .map(|value| Rc::new(RefCell::new(value)))
                .collect(),
        )
    }

    fn test_track_bus_send(bus_idx: usize, name: &str, amount: f64) -> Value {
        let mut map = std::collections::HashMap::new();
        map.insert(
            "bus-idx".to_string(),
            Rc::new(RefCell::new(Value::Number(bus_idx as f64))),
        );
        map.insert(
            "name".to_string(),
            Rc::new(RefCell::new(Value::String(name.to_string()))),
        );
        map.insert(
            "amount".to_string(),
            Rc::new(RefCell::new(Value::Number(amount))),
        );
        Value::Map(map)
    }

    fn test_track_pattern_cell(
        id: f64,
        _assigned: bool,
        _active: bool,
        _override_active: bool,
    ) -> Value {
        map_value([("id", Value::Number(id))])
    }

    fn set_test_track_pattern_cell_bindings(
        editor: &mut eseqlisp::Editor,
        track: usize,
        pattern_id: u64,
        assigned: bool,
        active: bool,
        override_active: bool,
        selected: bool,
    ) {
        let rt = editor.runtime_mut();
        rt.set_reactive(
            "SEQ",
            &track_pattern_cell_assigned_field(track, pattern_id),
            Value::Bool(assigned),
        );
        rt.set_reactive(
            "SEQ",
            &track_pattern_cell_active_field(track, pattern_id),
            Value::Bool(active),
        );
        rt.set_reactive(
            "SEQ",
            &track_pattern_cell_override_field(track, pattern_id),
            Value::Bool(override_active),
        );
        rt.set_reactive(
            "SEQ",
            &track_pattern_cell_selected_field(track, pattern_id),
            Value::Bool(selected),
        );
    }

    #[test]
    fn build_track_pattern_cells_value_exports_cell_maps() {
        let state = Arc::new(SequencerState::new(1, vec![vec![]]));
        let value = build_track_pattern_cells_value(&state, 1);
        let Value::List(tracks) = value else {
            panic!("track pattern cells should be a track list");
        };
        assert_eq!(tracks.len(), 1);
        let Value::List(cells) = &*tracks[0].borrow() else {
            panic!("track pattern cells should contain per-track cell lists");
        };
        assert_eq!(cells.len(), 1);
        let Value::Map(cell) = &*cells[0].borrow() else {
            panic!("track pattern cell should be a map");
        };
        assert_eq!(
            cell.get("id").map(|value| value.borrow().clone()),
            Some(Value::Number(1.0))
        );
        assert_eq!(
            cell.len(),
            1,
            "track pattern cell topology should only include stable identity"
        );
    }

    fn test_delete_target_number(payload: &Value, field: &str) -> Option<usize> {
        let Value::Map(map) = payload else {
            return None;
        };
        map.get(field).and_then(|value| match &*value.borrow() {
            Value::Number(value) if *value >= 0.0 => Some(*value as usize),
            _ => None,
        })
    }

    fn test_delete_target_string(payload: &Value, field: &str) -> Option<String> {
        let Value::Map(map) = payload else {
            return None;
        };
        map.get(field).and_then(|value| match &*value.borrow() {
            Value::String(value) => Some(value.clone()),
            Value::Keyword(value) => Some(value.clone()),
            _ => None,
        })
    }

    fn test_delete_target_key(kind: &str, payload: &Value) -> Option<String> {
        match kind {
            "mixer-track" => Some(format!(
                "mixer-track:{}",
                test_delete_target_number(payload, "track")?
            )),
            "track-pattern" => Some(format!(
                "track-pattern:{}:{}",
                test_delete_target_number(payload, "track")?,
                test_delete_target_number(payload, "pattern-id")?
            )),
            "mod-route" => Some(format!(
                "mod-route:{}:{}:{}",
                test_delete_target_number(payload, "source")?,
                test_delete_target_number(payload, "dest")?,
                test_delete_target_number(payload, "input")?
            )),
            "fx-effect" => {
                let chain = test_delete_target_string(payload, "chain")?;
                let slot = test_delete_target_number(payload, "slot")?;
                if chain == "bus" {
                    Some(format!(
                        "fx-effect:bus:{}:{}",
                        test_delete_target_number(payload, "bus")?,
                        slot
                    ))
                } else {
                    Some(format!("fx-effect:{chain}:{slot}"))
                }
            }
            _ => None,
        }
    }

    fn register_test_delete_target_natives(editor: &mut eseqlisp::Editor, track_count: usize) {
        let active = Arc::new(Mutex::new(None::<(String, Value)>));
        let version = Arc::new(std::sync::atomic::AtomicUsize::new(0));

        {
            let active = Arc::clone(&active);
            let version = Arc::clone(&version);
            editor
                .runtime_mut()
                .register_native("seq-set-delete-target", move |args, _ctx| {
                    let Some(kind) = args.first() else {
                        return Ok(Value::Bool(false));
                    };
                    let Some(payload) = args.get(1) else {
                        return Ok(Value::Bool(false));
                    };
                    let kind = match kind {
                        Value::Keyword(kind) | Value::String(kind) => kind.clone(),
                        _ => return Ok(Value::Bool(false)),
                    };
                    let Some(key) = test_delete_target_key(&kind, payload) else {
                        return Ok(Value::Bool(false));
                    };
                    *active.lock().unwrap() = Some((key, payload.clone()));
                    version.fetch_add(1, Ordering::Relaxed);
                    Ok(Value::Bool(true))
                });
        }

        {
            let active = Arc::clone(&active);
            let version = Arc::clone(&version);
            editor
                .runtime_mut()
                .register_native("seq-clear-delete-target", move |_args, _ctx| {
                    if active.lock().unwrap().take().is_some() {
                        version.fetch_add(1, Ordering::Relaxed);
                    }
                    Ok(Value::Bool(true))
                });
        }

        {
            let active = Arc::clone(&active);
            editor
                .runtime_mut()
                .register_native("seq-delete-target?", move |args, _ctx| {
                    let Some(kind) = args.first() else {
                        return Ok(Value::Bool(false));
                    };
                    let Some(payload) = args.get(1) else {
                        return Ok(Value::Bool(false));
                    };
                    let kind = match kind {
                        Value::Keyword(kind) | Value::String(kind) => kind,
                        _ => return Ok(Value::Bool(false)),
                    };
                    let Some(key) = test_delete_target_key(kind, payload) else {
                        return Ok(Value::Bool(false));
                    };
                    Ok(Value::Bool(
                        active
                            .lock()
                            .unwrap()
                            .as_ref()
                            .is_some_and(|(active_key, _)| active_key == &key),
                    ))
                });
        }

        {
            let active = Arc::clone(&active);
            editor.runtime_mut().register_native(
                "seq-active-delete-target-kind",
                move |_args, _ctx| {
                    let kind = active
                        .lock()
                        .unwrap()
                        .as_ref()
                        .map(|(key, _)| key.split(':').next().unwrap_or_default().to_string());
                    Ok(kind.map(Value::String).unwrap_or(Value::Bool(false)))
                },
            );
        }

        {
            let active = Arc::clone(&active);
            editor
                .runtime_mut()
                .register_native("seq-delete-active-target", move |_args, ctx| {
                    let Some((key, payload)) = active.lock().unwrap().take() else {
                        return Ok(Value::Bool(false));
                    };
                    let kind = key.split(':').next().unwrap_or_default();
                    let name = match (ctx.current_buffer_name().as_str(), kind) {
                        ("*mixer*", "mixer-track") => {
                            let Value::Map(map) = &payload else {
                                return Ok(Value::Bool(false));
                            };
                            let track = map.get("track").and_then(|value| match &*value.borrow() {
                                Value::Number(track) => Some(*track as usize),
                                _ => None,
                            });
                            if track_count <= 1 || track.is_none_or(|track| track >= track_count) {
                                return Ok(Value::Bool(false));
                            }
                            "delete-track"
                        }
                        ("*mixer*", "mod-route") => "delete-mod-route",
                        ("*mixer*", "track-pattern") => "delete-track-pattern",
                        ("*fx*", "fx-effect") => {
                            let chain = match &payload {
                                Value::Map(map) => {
                                    map.get("chain").and_then(|value| match &*value.borrow() {
                                        Value::String(chain) => Some(chain.clone()),
                                        _ => None,
                                    })
                                }
                                _ => None,
                            };
                            match chain.as_deref() {
                                Some("audio") => "delete-effect",
                                Some("midi") => "delete-midi-fx",
                                Some("bus") => "delete-bus-effect",
                                _ => return Ok(Value::Bool(false)),
                            }
                        }
                        _ => return Ok(Value::Bool(false)),
                    };
                    ctx.enqueue_command(eseqlisp::host::HostCommand::Custom {
                        name: name.to_string(),
                        payload,
                    });
                    Ok(Value::Bool(true))
                });
        }

        let active = Arc::clone(&active);
        editor.runtime_mut().register_native(
            "seq-clone-active-track-pattern",
            move |_args, ctx| {
                let Some((key, payload)) = active.lock().unwrap().clone() else {
                    return Ok(Value::Bool(false));
                };
                if ctx.current_buffer_name() != "*mixer*" || !key.starts_with("track-pattern:") {
                    return Ok(Value::Bool(false));
                }
                ctx.enqueue_command(eseqlisp::host::HostCommand::Custom {
                    name: "clone-track-pattern".to_string(),
                    payload,
                });
                Ok(Value::Bool(true))
            },
        );
    }

    fn value_contains_string(value: &Value, needle: &str) -> bool {
        match value {
            Value::String(text) => text.contains(needle),
            Value::List(items) => items
                .iter()
                .any(|item| value_contains_string(&item.borrow(), needle)),
            Value::Map(map) => map
                .values()
                .any(|item| value_contains_string(&item.borrow(), needle)),
            _ => false,
        }
    }

    fn value_contains_keyword(value: &Value, needle: &str) -> bool {
        match value {
            Value::Keyword(text) => text == needle,
            Value::List(items) => items
                .iter()
                .any(|item| value_contains_keyword(&item.borrow(), needle)),
            Value::Map(map) => map
                .values()
                .any(|item| value_contains_keyword(&item.borrow(), needle)),
            _ => false,
        }
    }

    fn layout_contains_widget_type(node: &eseqlisp::layout::LayoutNode, widget_type: &str) -> bool {
        node.widget_type == widget_type
            || node
                .children
                .iter()
                .any(|child| layout_contains_widget_type(child, widget_type))
    }

    fn find_layout_node_by_widget_type<'a>(
        node: &'a eseqlisp::layout::LayoutNode,
        widget_type: &str,
    ) -> Option<&'a eseqlisp::layout::LayoutNode> {
        if node.widget_type == widget_type {
            return Some(node);
        }
        node.children
            .iter()
            .find_map(|child| find_layout_node_by_widget_type(child, widget_type))
    }

    fn count_widget_type(node: &eseqlisp::layout::LayoutNode, widget_type: &str) -> usize {
        usize::from(node.widget_type == widget_type)
            + node
                .children
                .iter()
                .map(|child| count_widget_type(child, widget_type))
                .sum::<usize>()
    }

    fn count_stable_key_prefix(node: &eseqlisp::layout::LayoutNode, prefix: &str) -> usize {
        usize::from(
            node.stable_key
                .as_deref()
                .is_some_and(|key| key.starts_with(prefix)),
        ) + node
            .children
            .iter()
            .map(|child| count_stable_key_prefix(child, prefix))
            .sum::<usize>()
    }

    fn layout_contains_debug_name(node: &eseqlisp::layout::LayoutNode, needle: &str) -> bool {
        matches!(node.props.get("debug-name"), Some(Value::String(name)) if name.contains(needle))
            || node
                .children
                .iter()
                .any(|child| layout_contains_debug_name(child, needle))
    }

    fn find_layout_node_by_debug_name<'a>(
        node: &'a eseqlisp::layout::LayoutNode,
        needle: &str,
    ) -> Option<&'a eseqlisp::layout::LayoutNode> {
        if matches!(node.props.get("debug-name"), Some(Value::String(name)) if name.contains(needle))
        {
            return Some(node);
        }
        node.children
            .iter()
            .find_map(|child| find_layout_node_by_debug_name(child, needle))
    }

    fn find_layout_node_by_text<'a>(
        node: &'a eseqlisp::layout::LayoutNode,
        text: &str,
    ) -> Option<&'a eseqlisp::layout::LayoutNode> {
        if matches!(node.props.get("text"), Some(Value::String(value)) if value == text) {
            return Some(node);
        }
        node.children
            .iter()
            .find_map(|child| find_layout_node_by_text(child, text))
    }

    fn collect_layout_node_summaries(node: &eseqlisp::layout::LayoutNode, out: &mut Vec<String>) {
        let debug_name = match node.props.get("debug-name") {
            Some(Value::String(value)) => value.as_str(),
            _ => "",
        };
        let text = match node.props.get("text") {
            Some(Value::String(value)) => value.as_str(),
            _ => "",
        };
        if out.len() < 160 {
            out.push(format!(
                "{} debug={debug_name:?} text={text:?} rect={:?}",
                node.widget_type, node.rect
            ));
        }
        for child in &node.children {
            collect_layout_node_summaries(child, out);
        }
    }

    fn collect_tile_buffer_names(editor: &eseqlisp::Editor) -> Vec<String> {
        fn visit(
            node: &eseqlisp::tile::TileNode,
            buffers: &[eseqlisp::buffer::Buffer],
            out: &mut Vec<String>,
        ) {
            match node {
                eseqlisp::tile::TileNode::Leaf(leaf) => {
                    out.push(buffers[leaf.buffer_idx].name.clone());
                }
                eseqlisp::tile::TileNode::Split(split) => {
                    visit(&split.a, buffers, out);
                    visit(&split.b, buffers, out);
                }
            }
        }

        let mut names = Vec::new();
        visit(&editor.tile_root, &editor.buffers, &mut names);
        names
    }

    fn tile_tabs_for_buffer(editor: &eseqlisp::Editor, buffer_name: &str) -> Vec<(String, String)> {
        let buffer_idx = editor
            .buffers
            .iter()
            .position(|buffer| buffer.name == buffer_name)
            .unwrap_or_else(|| panic!("missing buffer {buffer_name}"));
        let leaf = editor
            .tile_root
            .find_leaf_by_buffer_idx(buffer_idx)
            .unwrap_or_else(|| panic!("missing tile for buffer {buffer_name}"));
        leaf.tabs
            .iter()
            .map(|tab| {
                (
                    tab.label.clone(),
                    editor.buffers[tab.buffer_idx].name.clone(),
                )
            })
            .collect()
    }

    fn layout_bottom(node: &eseqlisp::layout::LayoutNode) -> f32 {
        node.children
            .iter()
            .map(layout_bottom)
            .fold(node.rect.row + node.rect.height, f32::max)
    }

    fn assert_finite_layout_tree(node: &eseqlisp::layout::LayoutNode) {
        assert!(
            node.rect.col.is_finite()
                && node.rect.row.is_finite()
                && node.rect.width.is_finite()
                && node.rect.height.is_finite(),
            "layout node has non-finite rect: type={} rect={:?}",
            node.widget_type,
            node.rect
        );
        for child in &node.children {
            assert_finite_layout_tree(child);
        }
    }

    fn test_param_map(
        name: &str,
        idx: usize,
        value: f64,
        min: f64,
        max: f64,
    ) -> std::collections::HashMap<String, Rc<RefCell<Value>>> {
        let mut param = std::collections::HashMap::new();
        param.insert(
            "name".to_string(),
            Rc::new(RefCell::new(Value::String(name.to_string()))),
        );
        param.insert(
            "idx".to_string(),
            Rc::new(RefCell::new(Value::Number(idx as f64))),
        );
        param.insert(
            "value".to_string(),
            Rc::new(RefCell::new(Value::Number(value))),
        );
        param.insert("min".to_string(), Rc::new(RefCell::new(Value::Number(min))));
        param.insert("max".to_string(), Rc::new(RefCell::new(Value::Number(max))));
        param
    }

    fn test_param_map_with_ui_metadata(
        name: &str,
        idx: usize,
        value: f64,
        min: f64,
        max: f64,
        group: Option<&str>,
        env: Option<&str>,
        role: Option<&str>,
    ) -> std::collections::HashMap<String, Rc<RefCell<Value>>> {
        let mut param = test_param_map(name, idx, value, min, max);
        if let Some(group) = group {
            param.insert(
                "group".to_string(),
                Rc::new(RefCell::new(Value::String(group.to_string()))),
            );
        }
        if let Some(env) = env {
            param.insert(
                "env".to_string(),
                Rc::new(RefCell::new(Value::String(env.to_string()))),
            );
        }
        if let Some(role) = role {
            param.insert(
                "role".to_string(),
                Rc::new(RefCell::new(Value::String(role.to_string()))),
            );
        }
        param
    }

    fn load_param_grid_test_lisp(editor: &mut eseqlisp::Editor) {
        editor
            .runtime_mut()
            .eval_str(
                r#"
                (def seq-has-selection? () false)
                (def fx-clear-selected-effect () false)
                (defmacro aqua-slider-material () `(material :color (rgba 0.15 0.15 0.88 1.0)))
                (load "metal-seq-fx/state.lisp")
                (def visible-params (params)
                  (filter |p| (not (= (get p :name) "enabled")) params))
                (load "metal-seq-fx/param-controls.lisp")
                (load "metal-seq-fx/param-grid.lisp")
                "#,
            )
            .expect("load param grid test lisp");
    }

    fn param_grid_test_editor(params: Vec<Value>) -> eseqlisp::Editor {
        let mut editor = eseqlisp::Editor::new(Runtime::new(), eseqlisp::EditorConfig::default());
        let fx = Value::Map(test_fx_map("metadata-effect", 0, params.clone()));
        editor.runtime_mut().register_reactive(
            "TEST",
            vec![("params", test_list(params)), ("fx", fx)],
            true,
        );
        load_param_grid_test_lisp(&mut editor);
        editor
            .runtime_mut()
            .eval_str(r#"(effect-buffer "*param-grid-test*" (fx-param-grid TEST.params TEST.fx))"#)
            .expect("create param grid test buffer");
        editor.refresh_runtime_side_effects();
        if let Some(status) = editor.runtime_mut().take_status_message() {
            panic!("param grid test lisp status after refresh: {status}");
        }
        let buffer_id = editor
            .buffers
            .iter()
            .find(|buffer| buffer.name == "*param-grid-test*")
            .expect("param grid test buffer")
            .id;
        editor.set_active_buffer(buffer_id);
        editor.set_layout_viewport(140, 18);
        editor
    }

    fn custom_audio_fx_body_test_editor(params: Vec<Value>) -> eseqlisp::Editor {
        let mut editor = eseqlisp::Editor::new(Runtime::new(), eseqlisp::EditorConfig::default());
        let fx = Value::Map(test_fx_map("metadata-custom-fx", 0, params));
        editor
            .runtime_mut()
            .register_reactive("TEST", vec![("fx", fx)], true);
        editor
            .runtime_mut()
            .eval_str(
                r#"
                (def seq-has-selection? () false)
                (def fx-clear-selected-effect () false)
                (def custom-audio-fx-ui (fx) false)
                (def custom-midi-fx-ui (fx) false)
                (def custom-instrument-synth-ui (inst) false)
                (defmacro aqua-slider-material () `(material :color (rgba 0.15 0.15 0.88 1.0)))
                (load "metal-seq-fx/state.lisp")
                (def visible-params (params)
                  (filter |p| (not (= (get p :name) "enabled")) params))
                (load "metal-seq-fx/param-controls.lisp")
                (load "metal-seq-fx/param-grid.lisp")
                (load "metal-seq-fx/builtin/audio-fx.lisp")
                (load "metal-seq-fx/panel-bodies.lisp")
                (effect-buffer "*custom-audio-fx-body-test*"
                  (audio-fx-panel-body TEST.fx (get TEST.fx :params)))
                "#,
            )
            .expect("load custom audio effect body test lisp");
        editor.refresh_runtime_side_effects();
        if let Some(status) = editor.runtime_mut().take_status_message() {
            panic!("custom audio effect body test status after refresh: {status}");
        }
        let buffer_id = editor
            .buffers
            .iter()
            .find(|buffer| buffer.name == "*custom-audio-fx-body-test*")
            .expect("custom audio effect body test buffer")
            .id;
        editor.set_active_buffer(buffer_id);
        editor.set_layout_viewport(140, 18);
        editor
    }

    fn test_enum_param_map(
        name: &str,
        idx: usize,
        value: f64,
        labels: Vec<&str>,
    ) -> std::collections::HashMap<String, Rc<RefCell<Value>>> {
        let mut param = test_param_map(name, idx, value, 0.0, (labels.len() - 1) as f64);
        let selected = labels
            .get(value.round() as usize)
            .copied()
            .unwrap_or_default();
        param.insert(
            "text-value".to_string(),
            Rc::new(RefCell::new(Value::String(selected.to_string()))),
        );
        param.insert(
            "options".to_string(),
            Rc::new(RefCell::new(test_list(
                labels
                    .into_iter()
                    .map(|label| Value::String(label.to_string()))
                    .collect(),
            ))),
        );
        param
    }

    fn test_base_note_param_map(
        idx: usize,
    ) -> std::collections::HashMap<String, Rc<RefCell<Value>>> {
        let mut param = test_param_map("base_note", idx, 0.0, -48.0, 48.0);
        param.insert(
            "control".to_string(),
            Rc::new(RefCell::new(Value::String("base-note".to_string()))),
        );
        param
    }

    fn test_filter_params() -> Vec<Value> {
        vec![
            Value::Map(test_param_map("enabled", 0, 1.0, 0.0, 1.0)),
            Value::Map(test_enum_param_map(
                "mode",
                1,
                0.0,
                vec!["lowpass", "highpass", "bandpass", "notch"],
            )),
            Value::Map(test_param_map("cutoff", 2, 1000.0, 20.0, 20000.0)),
            Value::Map(test_param_map("resonance", 3, 1.0, 0.5, 10.0)),
            Value::Map(test_param_map("drive", 4, 0.0, 0.0, 1.0)),
            Value::Map(test_param_map("wet", 5, 1.0, 0.0, 1.0)),
            Value::Map(test_param_map("lfo amt", 6, 0.0, -1.0, 1.0)),
            Value::Map(test_param_map("lfo rate", 7, 1.0, 0.01, 40.0)),
            Value::Map(test_param_map("lfo sync", 8, 0.0, 0.0, 1.0)),
            Value::Map(test_enum_param_map(
                "lfo div",
                9,
                6.0,
                vec![
                    "1/32", "1/16", "1/16t", "1/8", "1/8t", "1/8.", "1/4", "1/4t", "1/4.", "1/2",
                    "1",
                ],
            )),
            Value::Map(test_enum_param_map(
                "lfo wave",
                10,
                0.0,
                vec!["sine", "tri", "saw", "ramp", "square", "s&h"],
            )),
            Value::Map(test_param_map("lfo phase", 11, 0.0, 0.0, 1.0)),
            Value::Map(test_param_map("env amt", 12, 0.0, -1.0, 1.0)),
            Value::Map(test_param_map("env attack", 13, 5.0, 0.1, 5000.0)),
            Value::Map(test_param_map("env release", 14, 120.0, 1.0, 5000.0)),
            Value::Map(test_enum_param_map("slope", 15, 0.0, vec!["12", "24"])),
        ]
    }

    fn test_str8_delay_params() -> Vec<Value> {
        let divs = vec![
            "1/32", "1/16", "1/16t", "1/8", "1/8t", "1/8.", "1/4", "1/4t", "1/4.", "1/2", "1",
        ];
        vec![
            Value::Map(test_param_map("enabled", 0, 1.0, 0.0, 1.0)),
            Value::Map(test_param_map("wet", 1, 0.5, 0.0, 1.0)),
            Value::Map(test_param_map("feedback", 2, 0.5, 0.0, 0.95)),
            Value::Map(test_param_map("left sync", 3, 1.0, 0.0, 1.0)),
            Value::Map(test_enum_param_map("left div", 4, 6.0, divs.clone())),
            Value::Map(test_param_map("left offset", 5, 0.0, -0.5, 0.5)),
            Value::Map(test_param_map("left time", 6, 250.0, 1.0, 2000.0)),
            Value::Map(test_param_map("right sync", 7, 1.0, 0.0, 1.0)),
            Value::Map(test_enum_param_map("right div", 8, 6.0, divs)),
            Value::Map(test_param_map("right offset", 9, 0.0, -0.5, 0.5)),
            Value::Map(test_param_map("right time", 10, 250.0, 1.0, 2000.0)),
            Value::Map(test_param_map("filter freq", 11, 1140.0, 20.0, 20000.0)),
            Value::Map(test_param_map("filter width", 12, 4.5, 0.25, 6.0)),
            Value::Map(test_param_map("mod rate", 13, 0.5, 0.01, 20.0)),
            Value::Map(test_param_map("mod amount", 14, 0.0, 0.0, 1.0)),
            Value::Map(test_param_map("mod phase", 15, 0.5, 0.0, 1.0)),
        ]
    }

    fn test_reverb_params() -> Vec<Value> {
        vec![
            Value::Map(test_param_map("mix", 0, 0.35, 0.0, 1.0)),
            Value::Map(test_param_map("size", 1, 0.2, 0.0, 1.0)),
            Value::Map(test_param_map("brightness", 2, 0.8, 0.0, 1.0)),
            Value::Map(test_param_map("replace", 3, 0.3, 0.0, 1.0)),
            Value::Map(test_param_map("enabled", 4, 1.0, 0.0, 1.0)),
        ]
    }

    fn test_modum_delay_params() -> Vec<Value> {
        vec![
            Value::Map(test_param_map("max1", 0, 500.0, 50.0, 5000.0)),
            Value::Map(test_param_map("max2", 1, 750.0, 50.0, 5000.0)),
            Value::Map(test_param_map("fbk", 2, 0.5, 0.1, 0.9)),
            Value::Map(test_param_map("cutoff", 3, 900.0, 50.0, 4000.0)),
            Value::Map(test_param_map("res", 4, 4.0, 1.0, 16.0)),
            Value::Map(test_param_map("rate", 5, 1.0, 0.1, 100.0)),
            Value::Map(test_param_map("enabled", 6, 1.0, 0.0, 1.0)),
        ]
    }

    fn test_dimension_d_params() -> Vec<Value> {
        vec![
            Value::Map(test_param_map("rate", 0, 0.32, 0.05, 1.2)),
            Value::Map(test_param_map("depth", 1, 8.5, 1.0, 20.0)),
            Value::Map(test_param_map("base", 2, 12.5, 6.0, 28.0)),
            Value::Map(test_param_map("spread", 3, 6.0, 0.0, 14.0)),
            Value::Map(test_param_map("mix", 4, 0.68, 0.0, 1.0)),
            Value::Map(test_param_map("tone", 5, 10500.0, 2000.0, 18000.0)),
            Value::Map(test_param_map("width", 6, 1.0, 0.0, 1.0)),
            Value::Map(test_param_map("shimmer", 7, 0.28, 0.0, 1.0)),
            Value::Map(test_param_map("enabled", 8, 1.0, 0.0, 1.0)),
        ]
    }

    fn test_lexilush_params() -> Vec<Value> {
        vec![
            Value::Map(test_param_map("pre_dly", 0, 1051.0, 0.0, 2000.0)),
            Value::Map(test_param_map("size", 1, 1.3, 0.1, 5.0)),
            Value::Map(test_param_map("decay", 2, 0.98, 0.1, 8.0)),
            Value::Map(test_param_map("diffusion", 3, 0.7, 0.0, 1.0)),
            Value::Map(test_param_map("damping", 4, 500.0, 100.0, 12000.0)),
            Value::Map(test_param_map("mod_freq", 5, 0.8, 0.0, 10.0)),
            Value::Map(test_param_map("mod_amt", 6, 40.0, 0.0, 100.0)),
            Value::Map(test_param_map("mix", 7, 0.35, 0.0, 1.0)),
            Value::Map(test_param_map("enabled", 8, 1.0, 0.0, 1.0)),
        ]
    }

    fn test_fx_map(
        name: &str,
        slot_idx: usize,
        params: Vec<Value>,
    ) -> std::collections::HashMap<String, Rc<RefCell<Value>>> {
        let mut map = std::collections::HashMap::new();
        map.insert(
            "name".to_string(),
            Rc::new(RefCell::new(Value::String(name.to_string()))),
        );
        map.insert(
            "slot-idx".to_string(),
            Rc::new(RefCell::new(Value::Number(slot_idx as f64))),
        );
        map.insert(
            "params".to_string(),
            Rc::new(RefCell::new(test_list(params))),
        );
        map
    }

    fn test_bus_fx_map(
        name: &str,
        bus_idx: usize,
        slot_idx: usize,
        mut params: Vec<Value>,
    ) -> std::collections::HashMap<String, Rc<RefCell<Value>>> {
        let mut sidechain = test_param_map("sidechain signal", params.len(), 0.0, 0.0, 3.0);
        sidechain.insert(
            "text-value".to_string(),
            Rc::new(RefCell::new(Value::String("off".to_string()))),
        );
        sidechain.insert(
            "options".to_string(),
            Rc::new(RefCell::new(test_list(vec![
                Value::String("off".to_string()),
                Value::String("kick".to_string()),
                Value::String("snare".to_string()),
                Value::String("hat".to_string()),
            ]))),
        );
        params.push(Value::Map(sidechain));

        let mut map = test_fx_map(name, slot_idx, params);
        map.insert(
            "bus-idx".to_string(),
            Rc::new(RefCell::new(Value::Number(bus_idx as f64))),
        );
        map.insert(
            "bus-fx".to_string(),
            Rc::new(RefCell::new(Value::Bool(true))),
        );
        map
    }

    fn test_instrument_map() -> std::collections::HashMap<String, Rc<RefCell<Value>>> {
        let mut inst = std::collections::HashMap::new();
        inst.insert(
            "name".to_string(),
            Rc::new(RefCell::new(Value::String("test-instrument".to_string()))),
        );
        inst.insert(
            "display-name".to_string(),
            Rc::new(RefCell::new(Value::String("test-instrument".to_string()))),
        );
        inst.insert(
            "type".to_string(),
            Rc::new(RefCell::new(Value::String("synth".to_string()))),
        );
        inst.insert(
            "synth".to_string(),
            Rc::new(RefCell::new(test_list(vec![
                Value::Map(test_param_map("cutoff", 0, 0.5, 0.0, 1.0)),
                Value::Map(test_param_map("amp_attack", 1, 5.0, 1.0, 1000.0)),
                Value::Map(test_param_map("amp_decay", 2, 120.0, 1.0, 2000.0)),
                Value::Map(test_param_map("amp_sustain", 3, 0.7, 0.0, 1.0)),
                Value::Map(test_param_map("amp_release", 4, 120.0, 1.0, 3000.0)),
            ]))),
        );
        inst.insert("mod".to_string(), Rc::new(RefCell::new(test_list(vec![]))));
        inst.insert(
            "sources".to_string(),
            Rc::new(RefCell::new(test_list(vec![]))),
        );
        inst
    }

    fn test_sampler_instrument_map(
        track: usize,
    ) -> std::collections::HashMap<String, Rc<RefCell<Value>>> {
        let mut enabled = test_param_map("enabled", 4, 1.0, 0.0, 1.0);
        enabled.insert(
            "boolean".to_string(),
            Rc::new(RefCell::new(Value::Bool(true))),
        );
        let mut reverse = test_param_map("reverse", 5, 0.0, 0.0, 1.0);
        reverse.insert(
            "boolean".to_string(),
            Rc::new(RefCell::new(Value::Bool(true))),
        );
        let mut loop_mode = test_param_map("loop", 6, 0.0, 0.0, 2.0);
        loop_mode.insert(
            "text-value".to_string(),
            Rc::new(RefCell::new(Value::String("gate".to_string()))),
        );
        loop_mode.insert(
            "options".to_string(),
            Rc::new(RefCell::new(test_list(vec![
                Value::String("off".to_string()),
                Value::String("gate".to_string()),
            ]))),
        );
        let mut warp = test_param_map("warp", 9, 0.0, 0.0, 1.0);
        warp.insert(
            "boolean".to_string(),
            Rc::new(RefCell::new(Value::Bool(true))),
        );

        let mut inst = test_instrument_map();
        inst.insert(
            "name".to_string(),
            Rc::new(RefCell::new(Value::String("sampler".to_string()))),
        );
        inst.insert(
            "display-name".to_string(),
            Rc::new(RefCell::new(Value::String("Sampler".to_string()))),
        );
        inst.insert(
            "type".to_string(),
            Rc::new(RefCell::new(Value::String("sampler".to_string()))),
        );
        inst.insert(
            "track".to_string(),
            Rc::new(RefCell::new(Value::Number(track as f64))),
        );
        inst.insert(
            "duration".to_string(),
            Rc::new(RefCell::new(Value::Number(1.0))),
        );
        inst.insert(
            "synth".to_string(),
            Rc::new(RefCell::new(test_list(vec![
                Value::Map(test_param_map("base", 100, 0.0, -48.0, 48.0)),
                Value::Map(test_param_map("attack", 0, 0.0, 0.0, 500.0)),
                Value::Map(test_param_map("release", 1, 0.0, 0.0, 2000.0)),
                Value::Map(test_param_map("start", 2, 0.0, 0.0, 1.0)),
                Value::Map(test_param_map("end", 3, 1.0, 0.0, 1.0)),
                Value::Map(enabled),
                Value::Map(reverse),
                Value::Map(loop_mode),
                Value::Map(test_param_map("xfade", 7, 0.0, 0.0, 250.0)),
                Value::Map(test_param_map("sr", 8, 44100.0, 2000.0, 44100.0)),
                Value::Map(warp),
                Value::Map(test_param_map("mode", 10, 0.0, 0.0, 0.0)),
                Value::Map(test_param_map("speed", 11, 1.0, -4.0, 4.0)),
                Value::Map(test_param_map("scrub", 12, 0.0, -1.0, 1.0)),
                Value::Map(test_param_map("bpm", 13, 120.0, 20.0, 400.0)),
            ]))),
        );
        inst.insert("mod".to_string(), Rc::new(RefCell::new(test_list(vec![]))));
        inst.insert(
            "modulators".to_string(),
            Rc::new(RefCell::new(test_list(vec![]))),
        );
        inst.insert(
            "source-names".to_string(),
            Rc::new(RefCell::new(test_list(vec![]))),
        );
        inst.insert(
            "sources".to_string(),
            Rc::new(RefCell::new(test_list(vec![]))),
        );
        inst
    }

    fn test_modulator_instrument_map() -> std::collections::HashMap<String, Rc<RefCell<Value>>> {
        let mut inst = std::collections::HashMap::new();
        inst.insert(
            "name".to_string(),
            Rc::new(RefCell::new(Value::String("Modulator".to_string()))),
        );
        inst.insert(
            "display-name".to_string(),
            Rc::new(RefCell::new(Value::String("Modulator".to_string()))),
        );
        inst.insert(
            "type".to_string(),
            Rc::new(RefCell::new(Value::String("modulator".to_string()))),
        );
        inst.insert(
            "phase-field".to_string(),
            Rc::new(RefCell::new(Value::String(
                modulator_phase_field(0).to_string(),
            ))),
        );
        inst.insert(
            "level-field".to_string(),
            Rc::new(RefCell::new(Value::String(
                modulator_level_field(0).to_string(),
            ))),
        );
        inst.insert(
            "synth".to_string(),
            Rc::new(RefCell::new(test_list(vec![
                Value::Map(test_param_map("enabled", 0, 1.0, 0.0, 1.0)),
                Value::Map(test_param_map("rise", 1, 18.0, 0.0, 5000.0)),
                Value::Map(test_param_map("fall", 2, 140.0, 0.0, 5000.0)),
            ]))),
        );
        inst.insert("mod".to_string(), Rc::new(RefCell::new(test_list(vec![]))));
        inst.insert(
            "sources".to_string(),
            Rc::new(RefCell::new(test_list(vec![]))),
        );
        inst
    }

    fn korg1_test_instrument_map() -> std::collections::HashMap<String, Rc<RefCell<Value>>> {
        let mut inst = std::collections::HashMap::new();
        inst.insert(
            "name".to_string(),
            Rc::new(RefCell::new(Value::String("korg1/".to_string()))),
        );
        inst.insert(
            "display-name".to_string(),
            Rc::new(RefCell::new(Value::String("korg1".to_string()))),
        );
        inst.insert(
            "type".to_string(),
            Rc::new(RefCell::new(Value::String("synth".to_string()))),
        );

        let params = [
            ("base_note", 48.0, 0.0, 127.0),
            ("vco1_saw", 0.75, 0.0, 1.0),
            ("vco1_pulse", 0.35, 0.0, 1.0),
            ("vco2_level", 0.45, 0.0, 1.0),
            ("sub_level", 0.25, 0.0, 1.0),
            ("gain", 0.80, 0.0, 1.0),
            ("analog_drift", 0.15, 0.0, 1.0),
            ("noise_level", 0.10, 0.0, 1.0),
            ("vco2_interval", 7.0, -24.0, 24.0),
            ("vco2_fine", 0.0, -100.0, 100.0),
            ("pulse_width", 0.50, 0.0, 1.0),
            ("pwm_amount", 0.20, 0.0, 1.0),
            ("input_drive", 0.35, 0.0, 1.0),
            ("output_bite", 0.25, 0.0, 1.0),
            ("ring_level", 0.15, 0.0, 1.0),
            ("cutoff", 0.58, 0.0, 1.0),
            ("resonance", 0.30, 0.0, 1.0),
            ("filter_env_amount", 0.40, -1.0, 1.0),
            ("keytrack", 0.50, 0.0, 1.0),
            ("hp_cutoff", 0.15, 0.0, 1.0),
            ("hp_resonance", 0.20, 0.0, 1.0),
            ("scream", 0.18, 0.0, 1.0),
            ("filter_drive", 0.28, 0.0, 1.0),
            ("lfo_rate", 4.0, 0.0, 20.0),
            ("lfo_filter_amount", 0.30, -1.0, 1.0),
            ("lfo_pitch", 0.0, -1.0, 1.0),
            ("pitch_env_amount", 0.0, -1.0, 1.0),
            ("amp_attack", 5.0, 1.0, 1000.0),
            ("amp_decay", 120.0, 1.0, 2000.0),
            ("amp_sustain", 0.70, 0.0, 1.0),
            ("amp_release", 150.0, 1.0, 3000.0),
            ("filt_attack", 8.0, 1.0, 1000.0),
            ("filt_decay", 180.0, 1.0, 2000.0),
            ("filt_sustain", 0.55, 0.0, 1.0),
            ("filt_release", 220.0, 1.0, 3000.0),
        ];
        inst.insert(
            "synth".to_string(),
            Rc::new(RefCell::new(test_list(
                params
                    .iter()
                    .enumerate()
                    .map(|(idx, (name, value, min, max))| {
                        Value::Map(test_param_map(name, idx, *value, *min, *max))
                    })
                    .collect(),
            ))),
        );
        inst.insert("mod".to_string(), Rc::new(RefCell::new(test_list(vec![]))));
        inst.insert(
            "sources".to_string(),
            Rc::new(RefCell::new(test_list(vec![]))),
        );
        inst
    }

    fn mod_fm_messui_test_instrument_map() -> std::collections::HashMap<String, Rc<RefCell<Value>>>
    {
        let mut inst = test_instrument_map();
        inst.insert(
            "name".to_string(),
            Rc::new(RefCell::new(Value::String("mod-fm-messui/".to_string()))),
        );
        inst.insert(
            "display-name".to_string(),
            Rc::new(RefCell::new(Value::String("mod-fm-messui".to_string()))),
        );

        let params = [
            ("amp_attack", 4.0, 1.0, 2000.0),
            ("amp_decay", 220.0, 1.0, 4000.0),
            ("amp_sustain", 0.65, 0.0, 1.0),
            ("amp_release", 180.0, 1.0, 5000.0),
            ("mod_attack", 2.0, 1.0, 2000.0),
            ("mod_decay", 380.0, 1.0, 5000.0),
            ("mod_sustain", 0.18, 0.0, 1.0),
            ("mod_release", 140.0, 1.0, 5000.0),
            ("op1_ratio", 0.25, 0.25, 16.0),
            ("op2_ratio", 2.0, 0.25, 16.0),
            ("op3_ratio", 1.5, 0.25, 16.0),
            ("op1_detune", 1.0, -1200.0, 1200.0),
            ("op2_detune", 0.0, -1200.0, 1200.0),
            ("op3_detune", 3.0, -1200.0, 1200.0),
            ("op1_level", 0.43, 0.0, 1.0),
            ("op2_level", 0.44, 0.0, 1.0),
            ("op3_level", 0.15, 0.0, 1.0),
            ("op2_to_op1", 1.0, 0.0, 12.0),
            ("op3_to_op1", 0.37, 0.0, 12.0),
            ("op3_to_op2", 2.0, 0.0, 12.0),
            ("mod_env_to_op2", 1.0, -4.0, 8.0),
            ("mod_env_to_op3", -3.2, -4.0, 8.0),
            ("lfo_rate", 3.05, 0.02, 30.0),
            ("lfo_to_index", 0.13, 0.0, 6.0),
            ("lfo_to_pitch", 0.0, 0.0, 48.0),
            ("filter_route", 1.0, 1.0, 5.0),
            ("f1_mode", 0.0, 0.0, 5.0),
            ("f2_mode", 0.0, 0.0, 5.0),
            ("f1_cutoff", 1517.0, 40.0, 16000.0),
            ("f2_cutoff", 535.0, 40.0, 16000.0),
            ("f1_resonance", 2.05, 0.5, 6.0),
            ("f2_resonance", 0.75, 0.5, 6.0),
            ("f1_drive", 0.37, 0.2, 8.0),
            ("f2_drive", 0.20, 0.2, 8.0),
            ("f1_env_amt", 4671.0, -8000.0, 8000.0),
            ("f2_env_amt", -483.0, -8000.0, 8000.0),
            ("f1_lfo_amt", 15.0, -4000.0, 4000.0),
            ("f2_lfo_amt", 2.0, -4000.0, 4000.0),
            ("filter_blend", 1.0, 0.0, 1.0),
            ("fold", 0.18, 0.0, 1.0),
            ("drive", 1.25, 0.2, 8.0),
            ("gain", 0.28, 0.0, 1.0),
        ];
        inst.insert(
            "synth".to_string(),
            Rc::new(RefCell::new(test_list(
                std::iter::once(Value::Map(test_base_note_param_map(0)))
                    .chain(
                        params
                            .iter()
                            .enumerate()
                            .map(|(idx, (name, value, min, max))| {
                                Value::Map(test_param_map(name, idx + 1, *value, *min, *max))
                            }),
                    )
                    .collect(),
            ))),
        );
        inst
    }

    fn mutant_909_test_instrument_map() -> std::collections::HashMap<String, Rc<RefCell<Value>>> {
        fn test_mod_target(source_idx: f64, depth_idx: f64, source_slot: f64, depth: f64) -> Value {
            Value::Map(HashMap::from([
                (
                    "source-idx".to_string(),
                    Rc::new(RefCell::new(Value::Number(source_idx))),
                ),
                (
                    "depth-idx".to_string(),
                    Rc::new(RefCell::new(Value::Number(depth_idx))),
                ),
                (
                    "source-slot".to_string(),
                    Rc::new(RefCell::new(Value::Number(source_slot))),
                ),
                (
                    "depth".to_string(),
                    Rc::new(RefCell::new(Value::Number(depth))),
                ),
                (
                    "depth-min".to_string(),
                    Rc::new(RefCell::new(Value::Number(-1.0))),
                ),
                (
                    "depth-max".to_string(),
                    Rc::new(RefCell::new(Value::Number(1.0))),
                ),
            ]))
        }

        let mut inst = test_instrument_map();
        inst.insert(
            "name".to_string(),
            Rc::new(RefCell::new(Value::String("909-mutant-fm/".to_string()))),
        );
        inst.insert(
            "display-name".to_string(),
            Rc::new(RefCell::new(Value::String("909-mutant-f".to_string()))),
        );
        inst.insert(
            "modulators".to_string(),
            Rc::new(RefCell::new(test_list(
                [
                    (1.0, "Mod 1"),
                    (2.0, "Mod 2"),
                    (3.0, "Mod 3"),
                    (4.0, "Mod 4"),
                ]
                .into_iter()
                .map(|(slot, label)| {
                    Value::Map(HashMap::from([
                        (
                            "slot".to_string(),
                            Rc::new(RefCell::new(Value::Number(slot))),
                        ),
                        (
                            "label".to_string(),
                            Rc::new(RefCell::new(Value::String(label.to_string()))),
                        ),
                    ]))
                })
                .collect(),
            ))),
        );

        let params = [
            ("voice", 0.0, 0.0, 10.0),
            ("body_wave", 0.0, 0.0, 3.0),
            ("filter_mode", 0.0, 0.0, 5.0),
            ("tune", 0.0, -24.0, 24.0),
            ("decay", 220.0, 1.0, 4000.0),
            ("tone", 0.0, 0.0, 1.0),
            ("sweep_decay", 36.0, 1.0, 1000.0),
            ("sweep_curve", 1.25, 0.0, 4.0),
            ("keytrack", 0.40, 0.0, 1.0),
            ("punch", 0.0, 0.0, 1.0),
            ("pitch_sweep", 0.0, -48.0, 48.0),
            ("membrane_fm", 0.0, -4.0, 4.0),
            ("pulse_width", 0.0, 0.0, 1.0),
            ("sub_level", 0.0, 0.0, 1.0),
            ("click_level", 0.0, 0.0, 1.0),
            ("snap", 0.0, 0.0, 1.0),
            ("amp_attack", 0.6, 0.0, 1000.0),
            ("amp_release", 18.0, 1.0, 5000.0),
            ("body_level", 0.0, 0.0, 1.0),
            ("noise_level", 0.0, 0.0, 1.0),
            ("metal_level", 0.0, 0.0, 1.0),
            ("metal_tune", 0.0, -24.0, 24.0),
            ("metal_spread", 0.0, 0.0, 1.0),
            ("hats_decay", 0.0, 0.0, 2000.0),
            ("noise_res", 0.0, 0.0, 1.0),
            ("body_ratio", 0.0, 0.0, 8.0),
            ("partial_spread", 0.0, 0.0, 1.0),
            ("noise_color", 0.0, 0.0, 1.0),
            ("gain", 0.0, 0.0, 1.0),
            ("resonance", 0.0, 0.0, 1.0),
            ("cross_ring", 0.0, 0.0, 1.0),
            ("drive", 0.0, 0.0, 8.0),
            ("fold", 0.0, 0.0, 1.0),
            ("crush", 0.0, 0.0, 1.0),
        ];
        inst.insert(
            "synth".to_string(),
            Rc::new(RefCell::new(test_list(
                std::iter::once(Value::Map(test_base_note_param_map(0)))
                    .chain(
                        params
                            .iter()
                            .enumerate()
                            .map(|(idx, (name, value, min, max))| {
                                let mut param = if *name == "voice" {
                                    test_enum_param_map(
                                        name,
                                        idx + 1,
                                        *value,
                                        vec![
                                            "kick", "snare", "lo tom", "mid tom", "hi tom", "rim",
                                            "clap", "closed", "open", "ride", "crash",
                                        ],
                                    )
                                } else if *name == "body_wave" {
                                    test_enum_param_map(
                                        name,
                                        idx + 1,
                                        *value,
                                        vec!["sin", "saw", "pulse", "tri"],
                                    )
                                } else if *name == "filter_mode" {
                                    test_enum_param_map(
                                        name,
                                        idx + 1,
                                        *value,
                                        vec!["LP", "BP", "HP", "notch", "peak", "all"],
                                    )
                                } else {
                                    test_param_map(name, idx + 1, *value, *min, *max)
                                };
                                if matches!(
                                    *name,
                                    "tune"
                                        | "decay"
                                        | "tone"
                                        | "pitch_sweep"
                                        | "membrane_fm"
                                        | "pulse_width"
                                        | "body_level"
                                        | "noise_level"
                                        | "metal_level"
                                        | "body_ratio"
                                        | "partial_spread"
                                        | "noise_color"
                                        | "drive"
                                        | "fold"
                                        | "crush"
                                ) {
                                    param.insert(
                                        "modulatable".to_string(),
                                        Rc::new(RefCell::new(Value::Bool(true))),
                                    );
                                    param.insert(
                                        "mod-targets".to_string(),
                                        Rc::new(RefCell::new(test_list(vec![test_mod_target(
                                            1000.0 + idx as f64,
                                            1100.0 + idx as f64,
                                            1.0,
                                            0.25,
                                        )]))),
                                    );
                                }
                                Value::Map(param)
                            }),
                    )
                    .collect(),
            ))),
        );
        inst
    }

    fn prophet_6_inspired_test_instrument_map() -> HashMap<String, Rc<RefCell<Value>>> {
        let mut inst = test_instrument_map();
        inst.insert(
            "name".to_string(),
            Rc::new(RefCell::new(Value::String(
                "emulations/prophet-6-inspired/".to_string(),
            ))),
        );
        inst.insert(
            "display-name".to_string(),
            Rc::new(RefCell::new(Value::String(
                "prophet-6-inspired".to_string(),
            ))),
        );

        let params = [
            ("amp_attack_ms", 4.0, 1.0, 4000.0),
            ("amp_decay_ms", 150.0, 1.0, 4000.0),
            ("amp_sustain", 0.82, 0.0, 1.0),
            ("amp_release_ms", 320.0, 1.0, 6000.0),
            ("filt_attack_ms", 2.0, 1.0, 4000.0),
            ("filt_decay_ms", 220.0, 1.0, 4000.0),
            ("filt_sustain", 0.10, 0.0, 1.0),
            ("filt_release_ms", 340.0, 1.0, 6000.0),
            ("osc1_shape", 0.92, 0.0, 1.0),
            ("osc2_shape", 0.24, 0.0, 1.0),
            ("osc1_semitones", 0.0, -24.0, 24.0),
            ("osc2_semitones", 0.0, -24.0, 24.0),
            ("pulse_width", 0.50, 0.10, 0.90),
            ("osc_mix", 0.43, 0.0, 1.0),
            ("osc_slop", 0.22, 0.0, 0.6),
            ("osc_detune_cents", 8.0, -25.0, 25.0),
            ("shape_drift", 0.16, 0.0, 0.5),
            ("sub_level", 0.12, 0.0, 0.7),
            ("noise_level", 0.010, 0.0, 0.25),
            ("brass", 0.38, 0.0, 1.0),
            ("cutoff", 820.0, 30.0, 12000.0),
            ("resonance", 1.75, 0.5, 3.9),
            ("filter_env_amt", 4200.0, -8000.0, 8000.0),
            ("keytrack", 0.46, 0.0, 1.0),
            ("vel_to_filter", 0.34, 0.0, 1.0),
            ("filter_drive", 1.35, 0.2, 5.0),
            ("filter_tone", 0.68, 0.0, 1.0),
            ("cutoff_skew", 0.16, 0.0, 1.0),
            ("lfo_rate_hz", 3.6, 0.05, 20.0),
            ("lfo_to_pw", 0.0, 0.0, 0.35),
            ("lfo_to_cutoff", 0.0, 0.0, 1800.0),
            ("env_to_pitch", 0.0, -12.0, 12.0),
            ("vibrato", 0.0, 0.0, 0.08),
            ("stereo_spread", 0.08, 0.0, 1.0),
            ("gain", 0.18, 0.0, 1.0),
        ];
        inst.insert(
            "synth".to_string(),
            Rc::new(RefCell::new(test_list(
                std::iter::once(Value::Map(test_base_note_param_map(0)))
                    .chain(
                        params
                            .iter()
                            .enumerate()
                            .map(|(idx, (name, value, min, max))| {
                                Value::Map(test_param_map(name, idx + 1, *value, *min, *max))
                            }),
                    )
                    .collect(),
            ))),
        );
        inst
    }

    fn minimoog_lad2_test_instrument_map() -> std::collections::HashMap<String, Rc<RefCell<Value>>>
    {
        let mut inst = std::collections::HashMap::new();
        inst.insert(
            "name".to_string(),
            Rc::new(RefCell::new(Value::String(
                "emulations/minimoog-lad2".to_string(),
            ))),
        );
        inst.insert(
            "display-name".to_string(),
            Rc::new(RefCell::new(Value::String(
                "emulations/minimoog-lad2".to_string(),
            ))),
        );
        inst.insert(
            "type".to_string(),
            Rc::new(RefCell::new(Value::String("synth".to_string()))),
        );

        let params = [
            ("base_note", 48.0, 0.0, 127.0),
            ("osc1_level", 0.80, 0.0, 1.0),
            ("osc2_level", 0.45, 0.0, 1.0),
            ("osc3_level", 0.25, 0.0, 1.0),
            ("noise_level", 0.10, 0.0, 1.0),
            ("gain", 0.80, 0.0, 1.0),
            ("drive", 0.35, 0.0, 1.0),
            ("key_track", 0.50, 0.0, 1.0),
            ("osc1_wave", 0.0, 0.0, 1.0),
            ("osc2_wave", 0.2, 0.0, 1.0),
            ("osc3_wave", 0.4, 0.0, 1.0),
            ("pulse_width", 0.50, 0.0, 1.0),
            ("osc1_oct", 0.0, -2.0, 2.0),
            ("osc2_oct", 0.0, -2.0, 2.0),
            ("osc3_oct", -1.0, -2.0, 2.0),
            ("osc2_detune", 0.0, -100.0, 100.0),
            ("osc3_detune", 0.0, -100.0, 100.0),
            ("cutoff", 1200.0, 20.0, 18_000.0),
            ("resonance", 0.30, 0.0, 1.0),
            ("filter_env_amount", 600.0, -6000.0, 6000.0),
            ("amp_attack", 5.0, 1.0, 1000.0),
            ("amp_decay", 120.0, 1.0, 2000.0),
            ("amp_sustain", 0.70, 0.0, 1.0),
            ("amp_release", 150.0, 1.0, 3000.0),
            ("filt_attack", 8.0, 1.0, 1000.0),
            ("filt_decay", 180.0, 1.0, 2000.0),
            ("filt_sustain", 0.55, 0.0, 1.0),
            ("filt_release", 220.0, 1.0, 3000.0),
        ];
        inst.insert(
            "synth".to_string(),
            Rc::new(RefCell::new(test_list(
                params
                    .iter()
                    .enumerate()
                    .map(|(idx, (name, value, min, max))| {
                        Value::Map(test_param_map(name, idx, *value, *min, *max))
                    })
                    .collect(),
            ))),
        );
        inst.insert("mod".to_string(), Rc::new(RefCell::new(test_list(vec![]))));
        inst.insert(
            "sources".to_string(),
            Rc::new(RefCell::new(test_list(vec![]))),
        );
        inst
    }

    fn test_number_list(values: &[f64]) -> Value {
        test_list(values.iter().copied().map(Value::Number).collect())
    }

    fn test_track_colors() -> Value {
        test_list(vec![test_number_list(&[0.96, 0.28, 0.52])])
    }

    fn test_bool_list(values: &[bool]) -> Value {
        test_list(values.iter().copied().map(Value::Bool).collect())
    }

    fn bool_list_values(value: &Value) -> Vec<bool> {
        match value {
            Value::List(items) => items
                .iter()
                .map(|item| match &*item.borrow() {
                    Value::Bool(value) => *value,
                    other => panic!("expected bool list item, got {other:?}"),
                })
                .collect(),
            other => panic!("expected bool list, got {other:?}"),
        }
    }

    fn string_list_values(value: &Value) -> Vec<String> {
        match value {
            Value::List(items) => items
                .iter()
                .map(|item| match &*item.borrow() {
                    Value::String(value) => value.clone(),
                    other => panic!("expected string list item, got {other:?}"),
                })
                .collect(),
            other => panic!("expected string list, got {other:?}"),
        }
    }

    #[test]
    fn duration_spans_cover_steps_held_by_active_sources() {
        let state = Arc::new(SequencerState::new(1, vec![]));
        let track = 0;
        state.pattern.patterns[track].set_step_active(0, true);
        state.pattern.patterns[track].set_step_active(3, true);
        state.pattern.step_data[track].set(0, StepParam::Duration, 2.0);
        state.pattern.step_data[track].set(3, StepParam::Duration, 0.5);

        let spans = bool_list_values(&build_track_duration_spans_value(&state, track));

        assert!(spans[0]);
        assert!(spans[1]);
        assert!(!spans[2]);
        assert!(spans[3]);
        assert!(!spans[4]);
    }

    #[test]
    fn sequencer_track_timebase_labels_show_current_selected_step_plock() {
        let state = Arc::new(SequencerState::new(2, vec![]));
        state.pattern.track_params[0].set_timebase(Timebase::Sixteenth);
        state.pattern.track_params[1].set_timebase(Timebase::Quarter);
        state.pattern.timebase_plocks[0].set(3, Timebase::EighthTriplet);
        state.pattern.timebase_plocks[1].set(3, Timebase::HalfTriplet);

        let selected = Some(3);
        assert_eq!(
            string_list_values(&build_track_timebase_labels_value(&state, 2, 0, selected)),
            ["8T", "4"],
            "only the current track row should reflect the selected step's timebase plock"
        );
        assert_eq!(
            string_list_values(&build_track_timebase_labels_value(&state, 2, 1, selected)),
            ["16", "2T"],
            "switching current track should resolve that track's selected-step timebase plock"
        );
        assert_eq!(
            string_list_values(&build_track_timebase_labels_value(&state, 2, 0, None)),
            ["16", "4"],
            "without selected steps every row should show its track default timebase"
        );
    }

    #[test]
    fn displayed_plock_step_uses_selection_before_playback() {
        let state = Arc::new(SequencerState::new(1, vec![]));
        let selected_steps = Arc::new(Mutex::new(HashSet::from([5, 2])));
        state.transport.playing.store(true, Ordering::Relaxed);
        state.transport.track_playheads[0].store(7, Ordering::Relaxed);

        let selected_step = selected_plock_step(&selected_steps);

        assert_eq!(selected_step, Some(2));
        assert_eq!(displayed_plock_step(&state, 0, selected_step), Some(2));
    }

    #[test]
    fn displayed_plock_step_follows_playhead_only_while_playing() {
        let state = Arc::new(SequencerState::new(1, vec![]));
        state.pattern.track_params[0].set_num_steps(16);
        state.transport.track_playheads[0].store(4, Ordering::Relaxed);

        assert_eq!(displayed_plock_step(&state, 0, None), None);

        state.transport.playing.store(true, Ordering::Relaxed);
        assert_eq!(displayed_plock_step(&state, 0, None), Some(4));
    }

    #[test]
    fn slot_param_stored_value_returns_played_plock_or_default() {
        let desc = sequencer::effects::EffectDescriptor::builtin_filter();
        let cutoff_idx = desc
            .params
            .iter()
            .position(|param| param.name == "cutoff")
            .expect("filter descriptor should include cutoff");
        let slot = sequencer::effects::EffectSlotState::new(&desc, 0);
        slot.defaults.set(cutoff_idx, 5000.0);
        slot.set_plock(4, cutoff_idx, 200.0);

        assert_eq!(
            slot_param_stored_value(&slot, &desc.params[cutoff_idx], cutoff_idx, None),
            5000.0
        );
        assert_eq!(
            slot_param_stored_value(&slot, &desc.params[cutoff_idx], cutoff_idx, Some(4)),
            200.0
        );
        assert_eq!(
            slot_param_stored_value(&slot, &desc.params[cutoff_idx], cutoff_idx, Some(5)),
            5000.0
        );
    }

    #[test]
    fn sync_track_effect_param_value_field_uses_selected_plock_value() {
        let desc = sequencer::effects::EffectDescriptor::builtin_filter();
        let cutoff_idx = desc
            .params
            .iter()
            .position(|param| param.name == "cutoff")
            .expect("filter descriptor should include cutoff");
        let state = Arc::new(SequencerState::new(
            1,
            vec![vec![sequencer::effects::EffectSlotState::new(&desc, 0)]],
        ));
        state.pattern.effect_chains[0][0]
            .defaults
            .set(cutoff_idx, 5000.0);
        state.pattern.effect_chains[0][0].set_plock(3, cutoff_idx, 1200.0);
        let descriptors = vec![vec![desc.clone()]];
        let field = track_effect_param_value_field(0, 0, cutoff_idx, &desc.params[cutoff_idx].name);
        let mut runtime = Runtime::new();

        sync_track_effect_param_value_field(
            &mut runtime,
            &state,
            &descriptors,
            0,
            0,
            cutoff_idx,
            Some(3),
        );

        assert_eq!(reactive_number(&runtime, "SEQ", &field), Some(1200.0));

        sync_track_effect_param_value_field(
            &mut runtime,
            &state,
            &descriptors,
            0,
            0,
            cutoff_idx,
            None,
        );

        assert_eq!(reactive_number(&runtime, "SEQ", &field), Some(5000.0));
    }

    #[test]
    fn sync_instrument_param_value_field_uses_selected_neuron_plock_value() {
        let desc = sequencer::effects::EffectDescriptor::builtin_filter();
        let resonance_idx = desc
            .params
            .iter()
            .position(|param| param.name == "resonance")
            .expect("filter descriptor should include resonance");
        let app = test_app_with_instrument_descriptor(desc.clone());
        app.state.pattern.instrument_slots[0].apply_descriptor(&desc, 17);
        app.state.pattern.instrument_slots[0]
            .defaults
            .set(resonance_idx, 0.2);
        app.state
            .edit_current_neural_networks(|networks| {
                let mut network = sequencer::neural::ProjectNeuralNetwork {
                    id: 11,
                    name: "router".to_string(),
                    num_neurons: 1,
                    ..sequencer::neural::ProjectNeuralNetwork::default()
                };
                network.neurons[0].output_overrides.instrument =
                    vec![sequencer::neural::ProjectParamOverride {
                        target_track: 0,
                        param_id: sequencer::neural::ParamNodeId {
                            logical_id: 17,
                            node_param_idx: desc.params[resonance_idx].node_param_idx,
                        },
                        param_index: resonance_idx,
                        value: 0.75,
                    }];
                networks.push(network);
                Ok(())
            })
            .unwrap();
        let selection =
            std::collections::BTreeSet::from([sequencer::lisp_host::SelectedNeuralNeuron {
                pattern_idx: 0,
                network_id: 11,
                neuron_idx: 0,
            }]);
        let field =
            instrument_param_value_field(0, resonance_idx, &desc.params[resonance_idx].name);
        let mut runtime = Runtime::new();

        sync_instrument_param_value_field_with_neural_selection(
            &mut runtime,
            &app,
            0,
            resonance_idx,
            None,
            Some(&selection),
        );

        assert_eq!(reactive_number(&runtime, "SEQ", &field), Some(0.75));
    }

    #[test]
    fn track_plocks_value_shows_selected_neuron_plocks() {
        let desc = sequencer::effects::EffectDescriptor::builtin_filter();
        let resonance_idx = desc
            .params
            .iter()
            .position(|param| param.name == "resonance")
            .expect("filter descriptor should include resonance");
        let app = test_app_with_instrument_descriptor(desc.clone());
        app.state.pattern.instrument_slots[0].apply_descriptor(&desc, 17);
        app.state.pattern.instrument_slots[0].set_plock(2, resonance_idx, 0.25);
        app.state
            .edit_current_neural_networks(|networks| {
                let mut network = sequencer::neural::ProjectNeuralNetwork {
                    id: 11,
                    name: "router".to_string(),
                    num_neurons: 1,
                    ..sequencer::neural::ProjectNeuralNetwork::default()
                };
                network.neurons[0].output_overrides.instrument =
                    vec![sequencer::neural::ProjectParamOverride {
                        target_track: 0,
                        param_id: sequencer::neural::ParamNodeId {
                            logical_id: 17,
                            node_param_idx: desc.params[resonance_idx].node_param_idx,
                        },
                        param_index: resonance_idx,
                        value: 0.75,
                    }];
                networks.push(network);
                Ok(())
            })
            .unwrap();
        let selected_steps = Arc::new(Mutex::new(HashSet::from([2])));
        let selection =
            std::collections::BTreeSet::from([sequencer::lisp_host::SelectedNeuralNeuron {
                pattern_idx: 0,
                network_id: 11,
                neuron_idx: 0,
            }]);

        let Value::List(items) = build_track_plocks_value_with_neural_selection(
            &app,
            &app.state,
            0,
            &selected_steps,
            Some(&selection),
        ) else {
            panic!("track p-locks should be a list");
        };

        assert_eq!(
            items.len(),
            1,
            "selected neuron p-locks should replace selected-step p-lock display"
        );
        let item = items[0].borrow();
        let Value::Map(map) = &*item else {
            panic!("track p-lock item should be a map");
        };
        assert_eq!(value_map_string(map, "label").as_deref(), Some("N1"));
        assert_eq!(value_map_string(map, "source").as_deref(), Some("neuron"));
        assert_eq!(
            value_map_string(map, "target").as_deref(),
            Some("neural-instrument")
        );
        assert_eq!(value_map_string(map, "group").as_deref(), Some("T1 inst"));
        assert_eq!(value_map_string(map, "name").as_deref(), Some("resonance"));
        assert_eq!(value_map_number(map, "network-id"), Some(11.0));
        assert_eq!(value_map_number(map, "neuron-idx"), Some(0.0));
        assert_eq!(value_map_number(map, "target-track"), Some(0.0));
        assert_eq!(
            value_map_number(map, "param-idx"),
            Some(resonance_idx as f64)
        );
        let value = map.get("value").and_then(|value| match &*value.borrow() {
            Value::Number(value) => Some(*value),
            _ => None,
        });
        assert_eq!(value, Some(0.75));
    }

    #[test]
    fn instrument_panel_restores_value_field_after_selection_clears() {
        let desc = sequencer::effects::EffectDescriptor::builtin_filter();
        let cutoff_idx = desc
            .params
            .iter()
            .position(|param| param.name == "cutoff")
            .expect("filter descriptor should include cutoff");
        let app = test_app_with_instrument_descriptor(desc.clone());
        app.state.pattern.instrument_slots[0]
            .defaults
            .set(cutoff_idx, 5000.0);
        app.state.pattern.instrument_slots[0].set_plock(3, cutoff_idx, 1200.0);
        let selected_steps = Arc::new(Mutex::new(HashSet::new()));
        let value_field =
            instrument_param_value_field(0, cutoff_idx, &desc.params[cutoff_idx].name);

        let default_panel = build_instrument_panel_value(&app, 0, &selected_steps);
        assert_eq!(
            value_param_number(&default_panel, "cutoff"),
            Some(5000.0),
            "unselected instrument panel should show the default value"
        );
        assert!(
            value_param_has_value_field(&default_panel, "cutoff", &value_field),
            "unselected instrument panel should bind cutoff to its default value field"
        );

        selected_steps.lock().unwrap().insert(3);
        let selected_panel = build_instrument_panel_value(&app, 0, &selected_steps);
        assert_eq!(
            value_param_number(&selected_panel, "cutoff"),
            Some(1200.0),
            "selected instrument panel should show the selected step p-lock"
        );
        assert!(
            value_param_has_value_field(&selected_panel, "cutoff", &value_field),
            "selected p-lock panel should keep a reactive display value field"
        );

        selected_steps.lock().unwrap().clear();
        let cleared_panel = build_instrument_panel_value(&app, 0, &selected_steps);
        assert_eq!(
            value_param_number(&cleared_panel, "cutoff"),
            Some(5000.0),
            "cleared selection should return the panel to default values"
        );
        assert!(
            value_param_has_value_field(&cleared_panel, "cutoff", &value_field),
            "cleared selection should restore default value binding"
        );
    }

    #[test]
    fn sampler_panel_restores_value_field_after_selection_clears() {
        let desc = sequencer::effects::EffectDescriptor::builtin_sampler();
        let attack_idx = desc
            .params
            .iter()
            .position(|param| param.name == "attack")
            .expect("sampler descriptor should include attack");
        let app = test_app_with_sampler_descriptor(desc.clone());
        app.state.pattern.instrument_slots[0]
            .defaults
            .set(attack_idx, 40.0);
        app.state.pattern.instrument_slots[0].set_plock(3, attack_idx, 120.0);
        let selected_steps = Arc::new(Mutex::new(HashSet::new()));
        let value_field =
            instrument_param_value_field(0, attack_idx, &desc.params[attack_idx].name);

        let default_panel = build_instrument_panel_value(&app, 0, &selected_steps);
        assert_eq!(
            value_param_number_by_idx(&default_panel, "attack", attack_idx),
            Some(40.0),
            "unselected sampler panel should show the default attack value"
        );
        assert!(
            value_param_has_value_field_by_idx(&default_panel, "attack", attack_idx, &value_field),
            "unselected sampler panel should bind attack to its default value field"
        );

        selected_steps.lock().unwrap().insert(3);
        let selected_panel = build_instrument_panel_value(&app, 0, &selected_steps);
        assert_eq!(
            value_param_number_by_idx(&selected_panel, "attack", attack_idx),
            Some(120.0),
            "selected sampler panel should show the selected step p-lock"
        );
        assert!(
            value_param_has_value_field_by_idx(&selected_panel, "attack", attack_idx, &value_field),
            "selected sampler p-lock panel should keep a reactive display value field"
        );

        selected_steps.lock().unwrap().clear();
        let cleared_panel = build_instrument_panel_value(&app, 0, &selected_steps);
        assert_eq!(
            value_param_number_by_idx(&cleared_panel, "attack", attack_idx),
            Some(40.0),
            "cleared selection should return sampler attack to its default value"
        );
        assert!(
            value_param_has_value_field_by_idx(&cleared_panel, "attack", attack_idx, &value_field),
            "cleared selection should restore sampler attack default value binding"
        );
    }

    fn test_app_with_instrument_descriptor(desc: sequencer::effects::EffectDescriptor) -> ui::App {
        let state = Arc::new(SequencerState::new(1, vec![vec![]]));
        state.pattern.instrument_slots[0].apply_descriptor(&desc, 0);
        let (keyboard_tx, _keyboard_rx) = std::sync::mpsc::channel();
        let mut app = ui::App::new(
            state,
            sequencer::audiograph::LiveGraphPtr(std::ptr::null_mut()),
            44_100,
            ui::AudioBuses {
                bus_l_id: 0,
                bus_r_id: 0,
                default_bus_nodes: Vec::new(),
                bus_gate_runtime: Arc::new(Mutex::new(Vec::new())),
                bus_gate_playheads: Arc::new(Mutex::new(Vec::new())),
                reverb_bus_id: 0,
                reverb_node_id: 0,
            },
            Arc::new(sequencer::recorder::MasterRecorder::new(44_100, 2)),
            keyboard_tx,
        );
        app.tracks = vec!["Track 1".to_string()];
        app.graph.instrument_descriptors = vec![desc];
        app
    }

    fn test_app_with_sampler_descriptor(desc: sequencer::effects::EffectDescriptor) -> ui::App {
        let mut app = test_app_with_instrument_descriptor(desc);
        app.graph.track_instrument_types = vec![sequencer::sequencer::InstrumentType::Sampler];
        app
    }

    fn reactive_number(runtime: &Runtime, namespace: &str, field: &str) -> Option<f64> {
        let namespace = runtime.global_value(namespace)?;
        let Value::Map(map) = namespace else {
            return None;
        };
        map.get(field).and_then(|value| match &*value.borrow() {
            Value::Number(value) => Some(*value),
            _ => None,
        })
    }

    fn value_param_number(value: &Value, name: &str) -> Option<f64> {
        let Value::Map(map) = value else {
            if let Value::List(items) = value {
                return items
                    .iter()
                    .find_map(|item| value_param_number(&item.borrow(), name));
            }
            return None;
        };
        if value_map_string(map, "name").as_deref() == Some(name) {
            return map.get("value").and_then(|value| match &*value.borrow() {
                Value::Number(value) => Some(*value),
                _ => None,
            });
        }
        map.values()
            .find_map(|value| value_param_number(&value.borrow(), name))
    }

    fn value_param_has_value_field(value: &Value, name: &str, expected_field: &str) -> bool {
        value_param_string(value, name, "value-field").as_deref() == Some(expected_field)
    }

    fn value_param_number_by_idx(value: &Value, name: &str, idx: usize) -> Option<f64> {
        let Value::Map(map) = value else {
            if let Value::List(items) = value {
                return items
                    .iter()
                    .find_map(|item| value_param_number_by_idx(&item.borrow(), name, idx));
            }
            return None;
        };
        if value_map_matches_name_idx(map, name, idx) {
            return map.get("value").and_then(|value| match &*value.borrow() {
                Value::Number(value) => Some(*value),
                _ => None,
            });
        }
        map.values()
            .find_map(|value| value_param_number_by_idx(&value.borrow(), name, idx))
    }

    fn value_param_has_value_field_by_idx(
        value: &Value,
        name: &str,
        idx: usize,
        expected_field: &str,
    ) -> bool {
        value_param_string_by_idx(value, name, idx, "value-field").as_deref()
            == Some(expected_field)
    }

    fn value_param_has_key(value: &Value, name: &str, key: &str) -> bool {
        let Value::Map(map) = value else {
            if let Value::List(items) = value {
                return items
                    .iter()
                    .any(|item| value_param_has_key(&item.borrow(), name, key));
            }
            return false;
        };
        if value_map_string(map, "name").as_deref() == Some(name) {
            return map.contains_key(key);
        }
        map.values()
            .any(|value| value_param_has_key(&value.borrow(), name, key))
    }

    fn value_param_has_key_by_idx(value: &Value, name: &str, idx: usize, key: &str) -> bool {
        let Value::Map(map) = value else {
            if let Value::List(items) = value {
                return items
                    .iter()
                    .any(|item| value_param_has_key_by_idx(&item.borrow(), name, idx, key));
            }
            return false;
        };
        if value_map_matches_name_idx(map, name, idx) {
            return map.contains_key(key);
        }
        map.values()
            .any(|value| value_param_has_key_by_idx(&value.borrow(), name, idx, key))
    }

    fn value_param_string(value: &Value, name: &str, key: &str) -> Option<String> {
        let Value::Map(map) = value else {
            if let Value::List(items) = value {
                return items
                    .iter()
                    .find_map(|item| value_param_string(&item.borrow(), name, key));
            }
            return None;
        };
        if value_map_string(map, "name").as_deref() == Some(name) {
            return value_map_string(map, key);
        }
        map.values()
            .find_map(|value| value_param_string(&value.borrow(), name, key))
    }

    fn value_param_string_by_idx(
        value: &Value,
        name: &str,
        idx: usize,
        key: &str,
    ) -> Option<String> {
        let Value::Map(map) = value else {
            if let Value::List(items) = value {
                return items
                    .iter()
                    .find_map(|item| value_param_string_by_idx(&item.borrow(), name, idx, key));
            }
            return None;
        };
        if value_map_matches_name_idx(map, name, idx) {
            return value_map_string(map, key);
        }
        map.values()
            .find_map(|value| value_param_string_by_idx(&value.borrow(), name, idx, key))
    }

    fn value_map_matches_name_idx(
        map: &HashMap<String, Rc<RefCell<Value>>>,
        name: &str,
        idx: usize,
    ) -> bool {
        value_map_string(map, "name").as_deref() == Some(name)
            && map.get("idx").is_some_and(
                |value| matches!(&*value.borrow(), Value::Number(value) if *value as usize == idx),
            )
    }

    fn value_map_string(map: &HashMap<String, Rc<RefCell<Value>>>, key: &str) -> Option<String> {
        map.get(key).and_then(|value| match &*value.borrow() {
            Value::String(value) => Some(value.clone()),
            _ => None,
        })
    }

    fn value_map_number(map: &HashMap<String, Rc<RefCell<Value>>>, key: &str) -> Option<f64> {
        map.get(key).and_then(|value| match &*value.borrow() {
            Value::Number(value) => Some(*value),
            _ => None,
        })
    }

    fn test_string_list(values: &[&str]) -> Value {
        test_list(
            values
                .iter()
                .map(|value| Value::String((*value).to_string()))
                .collect(),
        )
    }

    fn test_owned_string_list(values: &[String]) -> Value {
        test_list(values.iter().cloned().map(Value::String).collect())
    }

    fn test_repeated_number_list(value: f64, len: usize) -> Value {
        test_list((0..len).map(|_| Value::Number(value)).collect())
    }

    fn test_repeated_bool_list(value: bool, len: usize) -> Value {
        test_list((0..len).map(|_| Value::Bool(value)).collect())
    }

    fn test_multi_track_colors(track_count: usize) -> Value {
        let palette = [
            [0.96, 0.28, 0.52],
            [0.98, 0.55, 0.25],
            [0.95, 0.78, 0.28],
            [0.32, 0.78, 0.48],
            [0.24, 0.72, 0.78],
            [0.48, 0.54, 0.94],
            [0.82, 0.42, 0.92],
            [0.72, 0.74, 0.78],
            [0.92, 0.38, 0.34],
            [0.38, 0.86, 0.68],
        ];
        test_list(
            (0..track_count)
                .map(|track| {
                    let color = palette[track % palette.len()];
                    test_number_list(&color)
                })
                .collect(),
        )
    }

    fn sequencer_perf_steps(track: usize, generation: usize, step_count: usize) -> Value {
        test_list(
            (0..step_count)
                .map(|step| Value::Bool((step + track + generation) % 3 == 0))
                .collect(),
        )
    }

    fn sequencer_perf_plocks(track: usize, generation: usize, step_count: usize) -> Value {
        test_list(
            (0..step_count)
                .map(|step| Value::Bool((step * 5 + track + generation) % 11 == 0))
                .collect(),
        )
    }

    fn sequencer_perf_duration_spans(track: usize, generation: usize, step_count: usize) -> Value {
        test_list(
            (0..step_count)
                .map(|step| Value::Bool((step + track * 2 + generation) % 7 == 0))
                .collect(),
        )
    }

    fn register_full_grid_test_natives(editor: &mut eseqlisp::Editor) {
        editor
            .runtime_mut()
            .register_native("seq-filter-sample-tree", |_args, _ctx| {
                Ok(test_list(vec![]))
            });
        editor
            .runtime_mut()
            .register_native("seq-sample-browser", |_args, _ctx| {
                Ok(map_value([
                    ("tags", test_list(vec![])),
                    ("items", test_list(vec![])),
                ]))
            });
        editor
            .runtime_mut()
            .register_native("seq-sample-tags-for-path", |_args, _ctx| {
                Ok(test_list(vec![]))
            });
        editor
            .runtime_mut()
            .register_native("seq-project-tree", |_args, _ctx| Ok(test_list(vec![])));
        editor
            .runtime_mut()
            .register_native("seq-preset-tree", |_args, _ctx| Ok(test_list(vec![])));
        editor
            .runtime_mut()
            .register_native("seq-audio-effect-tree", |_args, _ctx| Ok(test_list(vec![])));
        editor
            .runtime_mut()
            .register_native("seq-midi-effect-tree", |_args, _ctx| Ok(test_list(vec![])));
        editor
            .runtime_mut()
            .register_native("seq-saved-instrument-tree", |args, _ctx| {
                let query = match args.first() {
                    Some(Value::String(s)) => s.as_str(),
                    _ => "",
                };
                Ok(build_instrument_tree_value(query))
            });
        editor
            .runtime_mut()
            .register_native("seq-piano-roll-action", |_args, _ctx| Ok(Value::Bool(true)));
        editor
            .runtime_mut()
            .register_native("seq-pause-auto-follow", |_args, _ctx| Ok(Value::Bool(true)));
        editor
            .runtime_mut()
            .register_native("seq-set-track", |_args, _ctx| Ok(Value::Bool(true)));
        editor
            .runtime_mut()
            .register_native("seq-clear-selection", |_args, _ctx| Ok(Value::Bool(true)));
        editor
            .runtime_mut()
            .register_native("seq-has-selection?", |_args, _ctx| Ok(Value::Bool(false)));
        editor
            .runtime_mut()
            .register_native("seq-track-step-active?", |_args, _ctx| {
                Ok(Value::Bool(true))
            });
        editor
            .runtime_mut()
            .register_native("seq-set-step-param", |_args, _ctx| Ok(Value::Bool(true)));
        editor
            .runtime_mut()
            .register_native("seq-set-step-param-plock", |_args, _ctx| {
                Ok(Value::Bool(true))
            });
        editor
            .runtime_mut()
            .register_native("seqv-sync-expanded-step-slots", |_args, _ctx| {
                Ok(Value::Bool(true))
            });
        editor
            .runtime_mut()
            .register_native("seqv-clear-expanded-step-slots", |_args, _ctx| {
                Ok(Value::Bool(true))
            });
        editor
            .runtime_mut()
            .register_native("seq-double-track-pattern", |_args, _ctx| {
                Ok(Value::Bool(true))
            });
        editor
            .runtime_mut()
            .register_native("seq-halve-track-pattern", |_args, _ctx| {
                Ok(Value::Bool(true))
            });
        editor
            .runtime_mut()
            .register_native("seq-toggle-track-collapsed", |_args, _ctx| {
                Ok(Value::Bool(true))
            });
        editor
            .runtime_mut()
            .register_native("seq-toggle-master-recording", |_args, _ctx| {
                Ok(Value::Bool(true))
            });
    }

    fn full_grid_editor_for_scroll_tests() -> eseqlisp::Editor {
        struct TestTextMeasurer;
        impl eseqlisp::layout::TextMeasurer for TestTextMeasurer {
            fn measure_text_px(&self, text: &str, _font_size: f32) -> f32 {
                text.chars().count() as f32 * 8.0
            }

            fn line_height_px(&self, _font_size: f32) -> f32 {
                16.0
            }
        }

        let mut editor = eseqlisp::Editor::new(Runtime::new(), eseqlisp::EditorConfig::default());
        editor.set_text_measurer(Box::new(TestTextMeasurer), 8.0, 16.0);
        register_agent_test_natives(editor.runtime_mut());
        register_full_grid_test_natives(&mut editor);
        editor.runtime_mut().register_reactive(
            "AGENT",
            vec![("generation", Value::Number(0.0))],
            false,
        );

        let steps = test_bool_list(&[
            true, false, true, false, true, false, true, false, true, false, true, false, true,
            false, true, false,
        ]);
        let step_numbers = test_number_list(&[
            1.0, 0.0, 0.8, 0.0, 0.7, 0.0, 0.9, 0.0, 0.6, 0.0, 0.8, 0.0, 1.0, 0.0, 0.7, 0.0,
        ]);
        let empty_plocks = test_bool_list(&[false; 16]);
        let one_track_bus_sends = test_list(vec![
            test_track_bus_send(0, "Bus A", 0.0),
            test_track_bus_send(1, "Bus B", 0.0),
        ]);

        editor.runtime_mut().register_reactive(
            "SEQ",
            vec![
                ("num-tracks", Value::Number(1.0)),
                ("track-ids", test_number_list(&[0.0])),
                ("track-names", test_string_list(&["bd02"])),
                ("track-colors", test_track_colors()),
                ("track-collapsed", test_bool_list(&[false])),
                (
                    "track-pattern-cells",
                    test_list(vec![test_list(vec![
                        test_track_pattern_cell(1.0, true, true, false),
                        test_track_pattern_cell(2.0, false, false, false),
                    ])]),
                ),
                ("current-track", Value::Number(0.0)),
                ("delete-target-version", Value::Number(0.0)),
                ("record-armed", test_bool_list(&[false])),
                ("track-mutes", test_bool_list(&[false])),
                ("track-solos", test_bool_list(&[false])),
                ("track-muted-by-solo", test_bool_list(&[false])),
                ("track-volumes", test_number_list(&[1.0])),
                ("track-mixer-pans", test_number_list(&[0.0])),
                ("track-outputs", test_string_list(&["main"])),
                (
                    "track-output-options",
                    test_string_list(&["main", "sends only", "Bus A", "Bus B"]),
                ),
                ("track-bus-sends", test_list(vec![one_track_bus_sends])),
                ("track-0-bus-0-send", Value::Number(0.0)),
                ("track-0-bus-1-send", Value::Number(0.0)),
                ("tp-bus-0-send", Value::Number(0.0)),
                ("tp-bus-1-send", Value::Number(0.0)),
                ("track-steps", test_list(vec![steps.clone()])),
                ("track-num-steps", test_number_list(&[16.0])),
                ("track-timebases", test_string_list(&["16"])),
                (
                    "track-duration-spans",
                    test_list(vec![test_bool_list(&[false; 16])]),
                ),
                (
                    "track-step-has-plocks",
                    test_list(vec![empty_plocks.clone()]),
                ),
                ("track-velocities", test_list(vec![step_numbers.clone()])),
                (
                    "track-durations",
                    test_list(vec![test_number_list(&[1.0; 16])]),
                ),
                ("track-auxas", test_list(vec![test_number_list(&[0.0; 16])])),
                (
                    "track-transposes",
                    test_list(vec![test_number_list(&[0.0; 16])]),
                ),
                ("track-pans", test_list(vec![test_number_list(&[0.0; 16])])),
                ("track-syncs", test_list(vec![test_number_list(&[0.0; 16])])),
                ("track-plocks", test_list(vec![])),
                ("selected-neural-neurons", test_list(vec![])),
                ("steps", steps.clone()),
                ("velocities", step_numbers.clone()),
                ("durations", test_number_list(&[1.0; 16])),
                ("auxas", test_number_list(&[0.0; 16])),
                ("transposes", test_number_list(&[0.0; 16])),
                ("pans", test_number_list(&[0.0; 16])),
                ("syncs", test_number_list(&[0.0; 16])),
                ("step-has-plocks", empty_plocks.clone()),
                ("selected-steps", test_bool_list(&[false; 16])),
                ("playhead", Value::Number(0.0)),
                ("playhead-page", Value::Number(0.0)),
                ("playing", Value::Bool(false)),
                ("auto-follow", Value::Bool(false)),
                ("tp-num-steps", Value::Number(16.0)),
                ("tp-timebase", Value::Number(0.0)),
                ("tp-swing", Value::Number(0.0)),
                ("tp-swing-resolution", Value::Number(0.0)),
                ("tp-gate", Value::Number(1.0)),
                ("tp-poly", Value::Bool(false)),
                ("tp-max-polyphony", Value::Number(6.0)),
                ("tp-accumulator", Value::Number(0.0)),
                ("tp-accum-mode", Value::Number(0.0)),
                ("tp-accum-limit", Value::Number(0.0)),
                ("tp-fts", Value::Number(0.0)),
                (
                    "sync-labels",
                    test_string_list(&["off", "1/16", "1/8", "1/4"]),
                ),
                ("fts-options", test_string_list(&["off", "up", "down"])),
                ("accum-mode-options", test_string_list(&["off", "wrap"])),
                ("accumulator-options", test_string_list(&["off", "step"])),
                ("bus-names", test_string_list(&["Mix", "Bus A", "Bus B"])),
                ("bus-mutes", test_bool_list(&[false, false, false])),
                ("bus-solos", test_bool_list(&[false, false, false])),
                ("bus-volumes", test_number_list(&[1.0, 1.0, 1.0])),
                (
                    "bus-steps",
                    test_list(vec![steps.clone(), steps.clone(), steps.clone()]),
                ),
                (
                    "bus-velocities",
                    test_list(vec![
                        step_numbers.clone(),
                        step_numbers.clone(),
                        step_numbers.clone(),
                    ]),
                ),
                (
                    "bus-durations",
                    test_list(vec![
                        test_number_list(&[1.0; 16]),
                        test_number_list(&[1.0; 16]),
                        test_number_list(&[1.0; 16]),
                    ]),
                ),
                (
                    "bus-syncs",
                    test_list(vec![
                        test_number_list(&[0.0; 16]),
                        test_number_list(&[0.0; 16]),
                        test_number_list(&[0.0; 16]),
                    ]),
                ),
                (
                    "bus-step-has-plocks",
                    test_list(vec![
                        empty_plocks.clone(),
                        empty_plocks.clone(),
                        empty_plocks.clone(),
                    ]),
                ),
                ("bus-playheads", test_number_list(&[0.0, 0.0, 0.0])),
                ("bus-num-steps", test_number_list(&[16.0, 16.0, 16.0])),
                ("bus-timebases", test_number_list(&[0.0, 0.0, 0.0])),
                ("bus-swings", test_number_list(&[0.0, 0.0, 0.0])),
                ("bus-swing-resolutions", test_number_list(&[0.0, 0.0, 0.0])),
                ("compiling", Value::Bool(false)),
                ("available-effects", test_list(vec![])),
                ("available-builtin-effects", test_list(vec![])),
                ("available-midi-effects", test_list(vec![])),
                ("effects", test_list(vec![])),
                ("midi-effects", test_list(vec![])),
                (
                    "bus-effects",
                    test_list(vec![
                        test_list(vec![]),
                        test_list(vec![]),
                        test_list(vec![]),
                    ]),
                ),
                (
                    "instrument-panel",
                    test_list(vec![Value::Map(test_instrument_map())]),
                ),
                ("piano-roll-lanes", build_piano_roll_lanes_value()),
                ("piano-roll-items", Value::List(vec![])),
                ("piano-roll-selection", Value::List(vec![])),
                ("sidebar-kind", Value::String("sampler".to_string())),
                ("sidebar-track-index", Value::Number(0.0)),
                ("sidebar-selected-sample", Value::String(String::new())),
                ("sidebar-presets", test_list(vec![])),
                ("sidebar-loaded-preset", Value::String(String::new())),
                ("sidebar-instrument-name", Value::String(String::new())),
                (
                    "sidebar-instrument-display-name",
                    Value::String(String::new()),
                ),
                ("current-project-name", Value::String("test".to_string())),
                ("current-pattern", Value::Number(0.0)),
                ("num-patterns", Value::Number(1.0)),
                ("editor-mode", Value::String(String::new())),
                ("editor-buffer-name", Value::String(String::new())),
                ("editor-error", Value::String(String::new())),
                ("editor-active-macro-name", Value::String(String::new())),
                ("editor-active-macro-action", Value::String(String::new())),
                (
                    "editor-instrument-run-mode",
                    Value::String("instrument".to_string()),
                ),
                ("recording", Value::Bool(false)),
                ("master-recording", Value::Bool(false)),
                ("transport-playhead", Value::Number(0.0)),
                ("bpm", Value::Number(120.0)),
                ("sampler-playhead", Value::Number(0.0)),
                ("master-peak-l", Value::Number(0.0)),
                ("master-peak-r", Value::Number(0.0)),
                ("cpu-load-pct", Value::Number(0.0)),
                ("track-peak-0", Value::Number(0.0)),
                ("bus-peak-0", Value::Number(0.0)),
                ("bus-peak-1", Value::Number(0.0)),
                ("bus-peak-2", Value::Number(0.0)),
            ],
            true,
        );
        editor.runtime_mut().register_reactive("SEQV", vec![], true);
        for step in 0..16 {
            editor.runtime_mut().set_reactive(
                "SEQ",
                &track_playhead_active_field(0, step),
                Value::Bool(step == 0),
            );
        }
        editor
            .runtime_mut()
            .set_reactive("SEQ", &track_playhead_page_field(0), Value::Number(0.0));
        set_test_expanded_step_slot_projection(&mut editor, 0, 0, 0, 0, 16, 0, 0);
        register_test_delete_target_natives(&mut editor, 1);

        let src = std::fs::read_to_string("metal-seq-grid.lisp").expect("read grid lisp");
        editor.runtime_mut().eval_str(&src).expect("load grid lisp");
        apply_startup_grid_layout(&mut editor).expect("apply startup grid layout");
        if let Some(status) = editor.runtime_mut().take_status_message() {
            if status.to_ascii_lowercase().contains("error") {
                panic!("full grid lisp status after refresh: {status}");
            }
        }
        editor
    }

    fn set_full_grid_track_count(
        editor: &mut eseqlisp::Editor,
        track_count: usize,
        step_count: usize,
    ) {
        let names = (0..track_count)
            .map(|track| format!("track-{track}"))
            .collect::<Vec<_>>();
        let ids = (0..track_count)
            .map(|track| Value::Number(track as f64))
            .collect::<Vec<_>>();
        let steps = (0..track_count)
            .map(|track| {
                test_list(
                    (0..step_count)
                        .map(|step| Value::Bool((step + track) % 2 == 0))
                        .collect(),
                )
            })
            .collect::<Vec<_>>();
        let repeated_param_lists = |value: f64| {
            (0..track_count)
                .map(|_| test_repeated_number_list(value, step_count))
                .collect::<Vec<_>>()
        };
        let rt = editor.runtime_mut();
        rt.set_reactive("SEQ", "num-tracks", Value::Number(track_count as f64));
        rt.set_reactive("SEQ", "track-ids", test_list(ids));
        rt.set_reactive("SEQ", "track-names", test_owned_string_list(&names));
        rt.set_reactive("SEQ", "track-colors", test_multi_track_colors(track_count));
        rt.set_reactive(
            "SEQ",
            "track-collapsed",
            test_repeated_bool_list(false, track_count),
        );
        rt.set_reactive(
            "SEQ",
            "track-pattern-cells",
            test_list(
                (0..track_count)
                    .map(|track| {
                        test_list(vec![test_track_pattern_cell(
                            (track + 1) as f64,
                            true,
                            track == 0,
                            false,
                        )])
                    })
                    .collect(),
            ),
        );
        rt.set_reactive("SEQ", "current-track", Value::Number(0.0));
        rt.set_reactive(
            "SEQ",
            "record-armed",
            test_repeated_bool_list(false, track_count),
        );
        rt.set_reactive(
            "SEQ",
            "track-mutes",
            test_repeated_bool_list(false, track_count),
        );
        rt.set_reactive(
            "SEQ",
            "track-solos",
            test_repeated_bool_list(false, track_count),
        );
        rt.set_reactive(
            "SEQ",
            "track-muted-by-solo",
            test_repeated_bool_list(false, track_count),
        );
        rt.set_reactive("SEQ", "track-steps", test_list(steps));
        rt.set_reactive(
            "SEQ",
            "track-num-steps",
            test_repeated_number_list(step_count as f64, track_count),
        );
        rt.set_reactive(
            "SEQ",
            "track-timebases",
            test_list(
                (0..track_count)
                    .map(|_| Value::String("16".to_string()))
                    .collect(),
            ),
        );
        rt.set_reactive(
            "SEQ",
            "track-duration-spans",
            test_list(
                (0..track_count)
                    .map(|_| test_repeated_bool_list(false, step_count))
                    .collect(),
            ),
        );
        rt.set_reactive(
            "SEQ",
            "track-step-has-plocks",
            test_list(
                (0..track_count)
                    .map(|_| test_repeated_bool_list(false, step_count))
                    .collect(),
            ),
        );
        rt.set_reactive(
            "SEQ",
            "track-playheads",
            test_repeated_number_list(0.0, track_count),
        );
        rt.set_reactive(
            "SEQ",
            "track-velocities",
            test_list(repeated_param_lists(1.0)),
        );
        rt.set_reactive(
            "SEQ",
            "track-durations",
            test_list(repeated_param_lists(1.0)),
        );
        rt.set_reactive("SEQ", "track-auxas", test_list(repeated_param_lists(0.0)));
        rt.set_reactive(
            "SEQ",
            "track-transposes",
            test_list(repeated_param_lists(0.0)),
        );
        rt.set_reactive("SEQ", "track-pans", test_list(repeated_param_lists(0.0)));
        rt.set_reactive("SEQ", "track-syncs", test_list(repeated_param_lists(0.0)));
        rt.set_reactive("SEQ", "tp-num-steps", Value::Number(step_count as f64));
        rt.set_reactive(
            "SEQ",
            "selected-steps",
            test_repeated_bool_list(false, step_count),
        );
        rt.set_reactive("SEQ", "steps", test_repeated_bool_list(true, step_count));
        rt.set_reactive(
            "SEQ",
            "velocities",
            test_repeated_number_list(1.0, step_count),
        );
        rt.set_reactive(
            "SEQ",
            "durations",
            test_repeated_number_list(1.0, step_count),
        );
        rt.set_reactive("SEQ", "auxas", test_repeated_number_list(0.0, step_count));
        rt.set_reactive(
            "SEQ",
            "transposes",
            test_repeated_number_list(0.0, step_count),
        );
        rt.set_reactive("SEQ", "pans", test_repeated_number_list(0.0, step_count));
        rt.set_reactive("SEQ", "syncs", test_repeated_number_list(0.0, step_count));
        rt.set_reactive(
            "SEQ",
            "step-has-plocks",
            test_repeated_bool_list(false, step_count),
        );
        for track in 0..track_count {
            for step in 0..step_count {
                rt.set_reactive(
                    "SEQ",
                    &track_step_active_field(track, step),
                    Value::Bool((step + track) % 2 == 0),
                );
                rt.set_reactive(
                    "SEQ",
                    &track_step_duration_field(track, step),
                    Value::Bool(false),
                );
                rt.set_reactive(
                    "SEQ",
                    &track_step_plocked_field(track, step),
                    Value::Bool(false),
                );
                rt.set_reactive(
                    "SEQ",
                    &track_step_selected_field(track, step),
                    Value::Bool(false),
                );
                rt.set_reactive(
                    "SEQ",
                    &track_playhead_active_field(track, step),
                    Value::Bool(step == 0),
                );
            }
            let rows = (step_count + PAGE_SIZE - 1) / PAGE_SIZE;
            for row in 0..rows {
                rt.set_reactive(
                    "SEQ",
                    &track_playhead_row_field(track, row),
                    Value::Number(if row == 0 { 0.0 } else { -1.0 }),
                );
            }
            rt.set_reactive("SEQ", &track_playhead_page_field(track), Value::Number(0.0));
        }
        for track in 0..track_count {
            set_test_expanded_step_slot_projection(editor, track, track, 0, 0, step_count, 0, 0);
        }
    }

    fn set_test_expanded_step_slot_projection(
        editor: &mut eseqlisp::Editor,
        track: usize,
        track_id: usize,
        page: usize,
        mode: usize,
        step_count: usize,
        cursor_step: usize,
        playhead_step: usize,
    ) {
        let rt = editor.runtime_mut();
        let page_count = ((step_count + PAGE_SIZE - 1) / PAGE_SIZE).max(1);
        for candidate in 0..((MAX_STEPS + PAGE_SIZE - 1) / PAGE_SIZE) {
            rt.set_reactive(
                "SEQ",
                &expanded_step_page_active_field(track_id, candidate),
                Value::Bool(candidate == page && candidate < page_count),
            );
        }
        for slot in 0..PAGE_SIZE {
            let step = page.saturating_mul(PAGE_SIZE).saturating_add(slot);
            let visible = step < step_count.min(MAX_STEPS);
            rt.set_reactive(
                "SEQ",
                &expanded_step_slot_index_field(track_id, slot),
                Value::Number(if visible { step as f64 } else { -1.0 }),
            );
            rt.set_reactive(
                "SEQ",
                &expanded_step_slot_label_field(track_id, slot),
                Value::Number((step + 1) as f64),
            );
            rt.set_reactive(
                "SEQ",
                &expanded_step_slot_visible_field(track_id, slot),
                Value::Bool(visible),
            );
            rt.set_reactive(
                "SEQ",
                &expanded_step_slot_active_field(track_id, slot),
                Value::Bool(visible && (step + track).is_multiple_of(2)),
            );
            rt.set_reactive(
                "SEQ",
                &expanded_step_slot_plocked_field(track_id, slot),
                Value::Bool(false),
            );
            rt.set_reactive(
                "SEQ",
                &expanded_step_slot_selected_field(track_id, slot),
                Value::Bool(false),
            );
            rt.set_reactive(
                "SEQ",
                &expanded_step_slot_playhead_field(track_id, slot),
                Value::Bool(visible && step == playhead_step),
            );
            rt.set_reactive(
                "SEQ",
                &expanded_step_slot_cursor_field(track_id, slot),
                Value::Bool(visible && step == cursor_step),
            );
            for candidate_mode in 0..=6 {
                let value = if visible && candidate_mode == 0 {
                    1.0
                } else {
                    0.0
                };
                rt.set_reactive(
                    "SEQ",
                    &expanded_step_slot_param_slider_field(track_id, candidate_mode, slot),
                    Value::Number(value),
                );
                rt.set_reactive(
                    "SEQ",
                    &expanded_step_slot_param_haptic_field(track_id, candidate_mode, slot),
                    Value::Number(value),
                );
            }
        }
        rt.set_reactive(
            "SEQ",
            &expanded_step_slot_param_slider_field(track_id, mode.min(6), 0),
            Value::Number(1.0),
        );
    }

    fn assert_finite_nonzero_rect(node: &eseqlisp::layout::LayoutNode, label: &str) {
        assert!(
            node.rect.width.is_finite()
                && node.rect.width > 0.0
                && node.rect.height.is_finite()
                && node.rect.height > 0.0,
            "{label} should have a finite nonzero rect: {:?}",
            node.rect
        );
    }

    fn apply_sequencer_perf_pattern(
        editor: &mut eseqlisp::Editor,
        track_count: usize,
        step_count: usize,
        generation: usize,
    ) {
        let names = (0..track_count)
            .map(|track| format!("perf-track-{track:02}"))
            .collect::<Vec<_>>();
        let ids = (0..track_count)
            .map(|track| Value::Number((1000 + track) as f64))
            .collect::<Vec<_>>();
        let track_steps = (0..track_count)
            .map(|track| sequencer_perf_steps(track, generation, step_count))
            .collect::<Vec<_>>();
        let track_duration_spans = (0..track_count)
            .map(|track| sequencer_perf_duration_spans(track, generation, step_count))
            .collect::<Vec<_>>();
        let track_step_has_plocks = (0..track_count)
            .map(|track| sequencer_perf_plocks(track, generation, step_count))
            .collect::<Vec<_>>();
        let track_velocities = (0..track_count)
            .map(|_| test_repeated_number_list(1.0, step_count))
            .collect::<Vec<_>>();
        let track_durations = (0..track_count)
            .map(|_| test_repeated_number_list(1.0, step_count))
            .collect::<Vec<_>>();
        let track_auxas = (0..track_count)
            .map(|_| test_repeated_number_list(0.0, step_count))
            .collect::<Vec<_>>();
        let track_transposes = (0..track_count)
            .map(|_| test_repeated_number_list(0.0, step_count))
            .collect::<Vec<_>>();
        let track_pans = (0..track_count)
            .map(|_| test_repeated_number_list(0.0, step_count))
            .collect::<Vec<_>>();
        let track_syncs = (0..track_count)
            .map(|_| test_repeated_number_list(0.0, step_count))
            .collect::<Vec<_>>();

        let rt = editor.runtime_mut();
        rt.set_reactive("SEQ", "num-tracks", Value::Number(track_count as f64));
        rt.set_reactive("SEQ", "track-ids", test_list(ids));
        rt.set_reactive("SEQ", "track-names", test_owned_string_list(&names));
        rt.set_reactive("SEQ", "track-colors", test_multi_track_colors(track_count));
        rt.set_reactive(
            "SEQ",
            "track-collapsed",
            test_repeated_bool_list(false, track_count),
        );
        rt.set_reactive("SEQ", "current-track", Value::Number(0.0));
        rt.set_reactive(
            "SEQ",
            "record-armed",
            test_repeated_bool_list(false, track_count),
        );
        rt.set_reactive(
            "SEQ",
            "track-mutes",
            test_repeated_bool_list(false, track_count),
        );
        rt.set_reactive(
            "SEQ",
            "track-solos",
            test_repeated_bool_list(false, track_count),
        );
        rt.set_reactive(
            "SEQ",
            "track-muted-by-solo",
            test_repeated_bool_list(false, track_count),
        );
        rt.set_reactive(
            "SEQ",
            "track-volumes",
            test_repeated_number_list(1.0, track_count),
        );
        rt.set_reactive(
            "SEQ",
            "track-mixer-pans",
            test_repeated_number_list(0.0, track_count),
        );
        rt.set_reactive(
            "SEQ",
            "track-outputs",
            test_list(
                (0..track_count)
                    .map(|_| Value::String("main".to_string()))
                    .collect(),
            ),
        );
        rt.set_reactive(
            "SEQ",
            "track-num-steps",
            test_repeated_number_list(step_count as f64, track_count),
        );
        rt.set_reactive(
            "SEQ",
            "track-timebases",
            test_list(
                (0..track_count)
                    .map(|_| Value::String("16".to_string()))
                    .collect(),
            ),
        );
        rt.set_reactive("SEQ", "track-steps", test_list(track_steps));
        rt.set_reactive(
            "SEQ",
            "track-duration-spans",
            test_list(track_duration_spans),
        );
        rt.set_reactive(
            "SEQ",
            "track-step-has-plocks",
            test_list(track_step_has_plocks),
        );
        rt.set_reactive("SEQ", "track-velocities", test_list(track_velocities));
        rt.set_reactive("SEQ", "track-durations", test_list(track_durations));
        rt.set_reactive("SEQ", "track-auxas", test_list(track_auxas));
        rt.set_reactive("SEQ", "track-transposes", test_list(track_transposes));
        rt.set_reactive("SEQ", "track-pans", test_list(track_pans));
        rt.set_reactive("SEQ", "track-syncs", test_list(track_syncs));
        rt.set_reactive("SEQ", "tp-num-steps", Value::Number(step_count as f64));
        rt.set_reactive(
            "SEQ",
            "steps",
            sequencer_perf_steps(0, generation, step_count),
        );
        rt.set_reactive(
            "SEQ",
            "step-has-plocks",
            sequencer_perf_plocks(0, generation, step_count),
        );
        rt.set_reactive(
            "SEQ",
            "selected-steps",
            test_repeated_bool_list(false, step_count),
        );
        rt.set_reactive(
            "SEQ",
            "durations",
            test_repeated_number_list(1.0, step_count),
        );
        rt.set_reactive(
            "SEQ",
            "velocities",
            test_repeated_number_list(1.0, step_count),
        );
        rt.set_reactive("SEQ", "auxas", test_repeated_number_list(0.0, step_count));
        rt.set_reactive(
            "SEQ",
            "transposes",
            test_repeated_number_list(0.0, step_count),
        );
        rt.set_reactive("SEQ", "pans", test_repeated_number_list(0.0, step_count));
        rt.set_reactive("SEQ", "syncs", test_repeated_number_list(0.0, step_count));

        let max_rows = (step_count + PAGE_SIZE - 1) / PAGE_SIZE;
        for track in 0..track_count {
            for step in 0..step_count {
                let active = (step + track + generation) % 3 == 0;
                let duration = (step + track * 2 + generation) % 7 == 0;
                let plocked = (step * 5 + track + generation) % 11 == 0;
                rt.set_reactive(
                    "SEQ",
                    &track_step_active_field(track, step),
                    Value::Bool(active),
                );
                rt.set_reactive(
                    "SEQ",
                    &track_step_duration_field(track, step),
                    Value::Bool(duration),
                );
                rt.set_reactive(
                    "SEQ",
                    &track_step_plocked_field(track, step),
                    Value::Bool(plocked),
                );
                rt.set_reactive(
                    "SEQ",
                    &track_step_selected_field(track, step),
                    Value::Bool(false),
                );
                rt.set_reactive(
                    "SEQ",
                    &track_playhead_active_field(track, step),
                    Value::Bool(step == (generation % PAGE_SIZE)),
                );
            }
            for row in 0..max_rows {
                rt.set_reactive(
                    "SEQ",
                    &track_playhead_row_field(track, row),
                    Value::Number(if row == 0 {
                        generation as f64 % PAGE_SIZE as f64
                    } else {
                        -1.0
                    }),
                );
            }
            rt.set_reactive("SEQ", &track_playhead_page_field(track), Value::Number(0.0));
        }
    }

    fn sequencer_perf_editor(track_count: usize, step_count: usize) -> eseqlisp::Editor {
        struct TestTextMeasurer;
        impl eseqlisp::layout::TextMeasurer for TestTextMeasurer {
            fn measure_text_px(&self, text: &str, _font_size: f32) -> f32 {
                text.chars().count() as f32 * 8.0
            }

            fn line_height_px(&self, _font_size: f32) -> f32 {
                16.0
            }
        }

        let mut editor = eseqlisp::Editor::new(Runtime::new(), eseqlisp::EditorConfig::default());
        editor.set_text_measurer(Box::new(TestTextMeasurer), 8.0, 16.0);
        editor.runtime_mut().register_reactive("SEQ", vec![], true);
        editor
            .runtime_mut()
            .eval_str("(defstate selected-bus -1)")
            .expect("install sequencer selection state");
        apply_sequencer_perf_pattern(&mut editor, track_count, step_count, 0);
        editor
            .runtime_mut()
            .eval_str(
                r#"
                  (load "metal-seq-themes.lisp")
                  (seq-theme-mac-osx-dark)
                  (load "metal-seq-materials.lisp")
                  (load "metal-seq-sequencer.lisp")
                "#,
            )
            .expect("load sequencer lisp");
        editor.refresh_runtime_side_effects();
        if let Some(status) = editor.runtime_mut().take_status_message() {
            if status.to_ascii_lowercase().contains("error") {
                panic!("sequencer perf fixture setup status: {status}");
            }
        }

        let sequencer_id = editor
            .buffers
            .iter()
            .find(|buffer| buffer.name == "*sequencer*")
            .expect("sequencer buffer should exist")
            .id;
        editor.set_active_buffer(sequencer_id);
        editor.set_layout_viewport(150, 44);
        let layout = editor
            .widget_layout()
            .expect("sequencer perf fixture layout should build");
        assert_eq!(
            count_stable_key_prefix(&layout, "seqv-step-cell-"),
            track_count * step_count,
            "sequencer perf fixture should render every track step"
        );
        editor
    }

    fn mixer_v2_perf_cells(track_count: usize, cell_count: usize) -> Value {
        test_list(
            (0..track_count)
                .map(|track| {
                    test_list(
                        (0..cell_count)
                            .map(|cell| {
                                let pattern_id = (track * 100 + cell + 1) as f64;
                                test_track_pattern_cell(pattern_id, true, false, false)
                            })
                            .collect(),
                    )
                })
                .collect(),
        )
    }

    fn apply_mixer_v2_perf_pattern(
        editor: &mut eseqlisp::Editor,
        track_count: usize,
        cell_count: usize,
        generation: usize,
    ) {
        for track in 0..track_count {
            for cell in 0..cell_count {
                set_test_track_pattern_cell_bindings(
                    editor,
                    track,
                    (track * 100 + cell + 1) as u64,
                    true,
                    cell == generation % cell_count,
                    cell == (generation + track) % cell_count,
                    false,
                );
            }
        }
    }

    fn apply_mixer_v2_perf_scene_switch(
        editor: &mut eseqlisp::Editor,
        track_count: usize,
        cell_count: usize,
        generation: usize,
    ) {
        editor.runtime_mut().set_reactive(
            "SEQ",
            "current-pattern",
            Value::Number((generation % cell_count) as f64),
        );
        editor
            .runtime_mut()
            .set_reactive("SEQ", "num-patterns", Value::Number(cell_count as f64));
        editor.runtime_mut().set_reactive(
            "SEQ",
            "track-pattern-cells",
            mixer_v2_perf_cells(track_count, cell_count),
        );
        apply_mixer_v2_perf_pattern(editor, track_count, cell_count, generation);
    }

    fn mixer_v2_perf_editor(track_count: usize, cell_count: usize) -> eseqlisp::Editor {
        struct TestTextMeasurer;
        impl eseqlisp::layout::TextMeasurer for TestTextMeasurer {
            fn measure_text_px(&self, text: &str, _font_size: f32) -> f32 {
                text.chars().count() as f32 * 8.0
            }

            fn line_height_px(&self, _font_size: f32) -> f32 {
                16.0
            }
        }

        let names = (0..track_count)
            .map(|track| format!("perf-track-{track:02}"))
            .collect::<Vec<_>>();
        let outputs = (0..track_count)
            .map(|_| Value::String("main".to_string()))
            .collect::<Vec<_>>();
        let bus_sends = (0..track_count)
            .map(|_| {
                test_list(vec![
                    test_track_bus_send(1, "Bus A", 0.0),
                    test_track_bus_send(2, "Bus B", 0.0),
                ])
            })
            .collect::<Vec<_>>();

        let mut editor = eseqlisp::Editor::new(Runtime::new(), eseqlisp::EditorConfig::default());
        editor.set_text_measurer(Box::new(TestTextMeasurer), 8.0, 16.0);
        editor.set_layout_viewport(160, 34);
        editor.runtime_mut().register_reactive(
            "SEQ",
            vec![
                ("track-names", test_owned_string_list(&names)),
                ("track-colors", test_multi_track_colors(track_count)),
                (
                    "track-collapsed",
                    test_repeated_bool_list(false, track_count),
                ),
                (
                    "track-pattern-cells",
                    mixer_v2_perf_cells(track_count, cell_count),
                ),
                ("num-tracks", Value::Number(track_count as f64)),
                ("current-track", Value::Number(0.0)),
                ("delete-target-version", Value::Number(0.0)),
                ("record-armed", test_repeated_bool_list(false, track_count)),
                ("track-mutes", test_repeated_bool_list(false, track_count)),
                ("track-solos", test_repeated_bool_list(false, track_count)),
                (
                    "track-muted-by-solo",
                    test_repeated_bool_list(false, track_count),
                ),
                (
                    "track-instrument-types",
                    test_list(
                        (0..track_count)
                            .map(|_| Value::String("instrument".to_string()))
                            .collect(),
                    ),
                ),
                (
                    "track-mod-output-available",
                    test_repeated_bool_list(true, track_count),
                ),
                ("mod-routes", test_list(vec![])),
                ("selected-mod-routes", test_list(vec![])),
                ("track-volumes", test_repeated_number_list(1.0, track_count)),
                (
                    "track-mixer-pans",
                    test_repeated_number_list(0.0, track_count),
                ),
                ("track-outputs", test_list(outputs)),
                (
                    "track-output-options",
                    test_string_list(&["main", "sends only", "Bus A", "Bus B"]),
                ),
                ("track-bus-sends", test_list(bus_sends)),
                ("bus-names", test_string_list(&["Mix", "Bus A", "Bus B"])),
                ("bus-volumes", test_number_list(&[1.0, 1.0, 1.0])),
                ("bus-mutes", test_bool_list(&[false, false, false])),
                ("bus-solos", test_bool_list(&[false, false, false])),
                ("master-peak-l", Value::Number(0.0)),
                ("master-peak-r", Value::Number(0.0)),
                ("bus-peak-0", Value::Number(0.0)),
                ("bus-peak-1", Value::Number(0.0)),
                ("bus-peak-2", Value::Number(0.0)),
            ],
            true,
        );

        for track in 0..track_count {
            editor.runtime_mut().set_reactive(
                "SEQ",
                &track_selected_field(track),
                Value::Bool(track == 0),
            );
            editor.runtime_mut().set_reactive(
                "SEQ",
                &mixer_track_delete_target_field(track),
                Value::Bool(false),
            );
            editor.runtime_mut().set_reactive(
                "SEQ",
                &format!("track-peak-{track}"),
                Value::Number(0.0),
            );
            for bus in 1..=2 {
                editor.runtime_mut().set_reactive(
                    "SEQ",
                    &format!("track-{track}-bus-{bus}-send"),
                    Value::Number(0.0),
                );
            }
            for cell in 0..cell_count {
                set_test_track_pattern_cell_bindings(
                    &mut editor,
                    track,
                    (track * 100 + cell + 1) as u64,
                    true,
                    cell == 0,
                    cell == track % cell_count,
                    false,
                );
            }
        }

        editor
            .runtime_mut()
            .eval_str(
                "(defmacro aqua-slider-material () `(material :color (rgba 0.15 0.15 0.88 1.0)))",
            )
            .expect("install test slider material macro");
        editor
            .runtime_mut()
            .eval_str("(defstate selected-bus -1)")
            .expect("install shared mixer selection state");
        register_test_delete_target_natives(&mut editor, track_count);
        let src = std::fs::read_to_string("metal-seq-mixer-v2.lisp").expect("read mixer lisp");
        editor
            .runtime_mut()
            .eval_str(&src)
            .expect("load mixer lisp");
        editor.refresh_runtime_side_effects();
        let mixer_id = editor
            .buffers
            .iter()
            .find(|buffer| buffer.name == "*mixer*")
            .expect("mixer lisp should create the *mixer* buffer")
            .id;
        editor.set_active_buffer(mixer_id);
        editor.active_buffer_mut().view_mode = eseqlisp::editor::ViewMode::UiOnly;
        editor.set_layout_viewport(160, 34);
        editor.refresh_runtime_side_effects();
        let layout = editor
            .widget_layout()
            .expect("mixer perf fixture layout should build");
        assert_eq!(
            count_stable_key_prefix(&layout, "mixer-v2-track-pattern-cell-"),
            track_count * cell_count,
            "mixer perf fixture should render every track pattern cell"
        );
        editor
    }

    #[test]
    fn metal_seq_cmd_a_global_binding_selects_steps_outside_piano_roll() {
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

        let mut editor = full_grid_editor_for_scroll_tests();
        editor
            .runtime_mut()
            .eval_str(
                r#"
                  (def selected-count (state 0))
                  (def select-all-steps () (set! selected-count (+ selected-count 1)))
                "#,
            )
            .expect("install select-all test hook");
        editor.refresh_runtime_side_effects();

        editor
            .runtime_mut()
            .eval_str(r#"(set-window-buffer "*fx*")"#)
            .expect("switch to fx buffer");
        editor.refresh_runtime_side_effects();
        editor
            .runtime_mut()
            .eval_str(r#"(seqv-handle-key "C-a" nil)"#)
            .expect("route select-all through sequencer key handler");

        assert_eq!(
            editor.runtime_mut().eval_str("selected-count").unwrap(),
            Some(Value::Number(1.0))
        );

        editor
            .runtime_mut()
            .eval_str(r#"(set-window-buffer "*transport*")"#)
            .expect("switch to transport buffer");
        editor.refresh_runtime_side_effects();
        editor.handle_key(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::CONTROL));

        assert_eq!(
            editor.runtime_mut().eval_str("selected-count").unwrap(),
            Some(Value::Number(2.0))
        );

        editor
            .runtime_mut()
            .eval_str(r#"(set-window-buffer "*piano-roll*")"#)
            .expect("switch to piano roll buffer");
        editor.refresh_runtime_side_effects();
        editor.handle_key(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::CONTROL));

        assert_eq!(
            editor.runtime_mut().eval_str("selected-count").unwrap(),
            Some(Value::Number(2.0))
        );

        editor.open_scratch_buffer("*editable*", "abc");
        editor.active_buffer_mut().cursor = (0, 2);
        editor.handle_key(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::CONTROL));

        assert_eq!(
            editor.runtime_mut().eval_str("selected-count").unwrap(),
            Some(Value::Number(2.0))
        );
        assert_eq!(editor.active_buffer().cursor, (0, 0));
    }

    #[test]
    fn metal_seq_default_step_panel_has_no_matrix_tab_or_buffer() {
        let editor = full_grid_editor_for_scroll_tests();

        assert!(
            !editor
                .buffers
                .iter()
                .any(|buffer| buffer.name == "*matrix*"),
            "default grid load must not create the legacy matrix buffer"
        );
        assert!(
            collect_tile_buffer_names(&editor)
                .iter()
                .all(|name| name != "*matrix*"),
            "default layout must not reference the legacy matrix buffer"
        );
        assert!(
            tile_tabs_for_buffer(&editor, "*sequencer*").is_empty(),
            "default sequencer tile should not render a one-item tab bar"
        );
    }

    #[test]
    fn metal_seq_register_step_sequencer_tab_adds_tab_without_selecting_custom_buffer() {
        let mut editor = full_grid_editor_for_scroll_tests();

        editor
            .runtime_mut()
            .eval_str(
                r#"
                (effect-buffer "*fake-seq*" (label "fake sequencer"))
                (seq-register-step-sequencer-tab "Fake" "*fake-seq*")
                "#,
            )
            .expect("register fake sequencer tab");
        editor.refresh_runtime_side_effects();

        assert_eq!(
            editor.runtime_mut().eval_str("step-panel-buffer").unwrap(),
            Some(Value::String("*sequencer*".to_string()))
        );
        assert_eq!(
            editor
                .runtime_mut()
                .eval_str("remembered-step-panel-buffer")
                .unwrap(),
            Some(Value::String("*sequencer*".to_string()))
        );
        assert!(
            collect_tile_buffer_names(&editor).contains(&"*sequencer*".to_string()),
            "registration should preserve the visible sequencer panel"
        );
        assert_eq!(
            tile_tabs_for_buffer(&editor, "*sequencer*"),
            vec![
                ("Seq".to_string(), "*sequencer*".to_string()),
                ("Fake".to_string(), "*fake-seq*".to_string())
            ]
        );
    }

    #[test]
    fn metal_seq_register_step_sequencer_tab_upserts_by_buffer_and_preserves_selection() {
        let mut editor = full_grid_editor_for_scroll_tests();

        editor
            .runtime_mut()
            .eval_str(
                r#"
                (effect-buffer "*fake-seq*" (label "fake sequencer"))
                (seq-register-step-sequencer-tab "Fake" "*fake-seq*")
                (seq-register-step-sequencer-tab "Renamed" "*fake-seq*")
                "#,
            )
            .expect("register and rename fake sequencer tab");
        editor.refresh_runtime_side_effects();

        assert_eq!(
            tile_tabs_for_buffer(&editor, "*sequencer*"),
            vec![
                ("Seq".to_string(), "*sequencer*".to_string()),
                ("Renamed".to_string(), "*fake-seq*".to_string())
            ],
            "re-registering the same buffer should update the label without duplicating the tab"
        );

        editor
            .runtime_mut()
            .eval_str("(seq-apply-fx-layout)")
            .expect("reapply main layout");
        editor.refresh_runtime_side_effects();

        assert_eq!(
            editor.runtime_mut().eval_str("step-panel-buffer").unwrap(),
            Some(Value::String("*sequencer*".to_string()))
        );
        assert_eq!(
            tile_tabs_for_buffer(&editor, "*sequencer*"),
            vec![
                ("Seq".to_string(), "*sequencer*".to_string()),
                ("Renamed".to_string(), "*fake-seq*".to_string())
            ],
            "reapplying the layout should keep the registered tabs without selecting the custom tab"
        );
    }

    #[test]
    fn metal_seq_register_step_sequencer_tab_does_not_restore_main_layout_when_step_tile_absent() {
        let mut editor = full_grid_editor_for_scroll_tests();
        editor.create_scratch_buffer("*code*", "", eseqlisp::BufferMode::ESeqLisp);
        editor.create_scratch_buffer("*fake-seq*", "", eseqlisp::BufferMode::ESeqLisp);

        editor
            .runtime_mut()
            .eval_str(
                r#"
                (set-layout
                  (list :cols :gap 1
                    0.5 (list :buf "*code*" :hide-status true)
                    0.5 (list :buf "*fake-seq*" :hide-status true)))
                "#,
            )
            .expect("install custom code/ui layout");
        editor.refresh_runtime_side_effects();

        editor
            .runtime_mut()
            .eval_str(
                r#"
                (effect-buffer "*fake-seq*" (label "fake sequencer"))
                (seq-register-step-sequencer-tab "Fake" "*fake-seq*")
                "#,
            )
            .expect("register fake sequencer tab");
        editor.refresh_runtime_side_effects();

        let tile_buffers = collect_tile_buffer_names(&editor);
        assert_eq!(
            tile_buffers,
            vec!["*code*".to_string(), "*fake-seq*".to_string()],
            "registration should not rebuild the main sequencer layout when no step tile is present"
        );
        assert_eq!(
            tile_tabs_for_buffer(&editor, "*fake-seq*"),
            vec![
                ("Seq".to_string(), "*sequencer*".to_string()),
                ("Fake".to_string(), "*fake-seq*".to_string())
            ],
            "registration should still add tabs to an already visible custom sequencer tile"
        );
    }

    #[test]
    fn metal_seq_grid_reload_does_not_reapply_startup_layout() {
        let mut editor = full_grid_editor_for_scroll_tests();
        editor.create_scratch_buffer("*code*", "", eseqlisp::BufferMode::ESeqLisp);
        editor.create_scratch_buffer("*fake-seq*", "", eseqlisp::BufferMode::ESeqLisp);

        editor
            .runtime_mut()
            .eval_str(
                r#"
                (set-layout
                  (list :cols :gap 1
                    0.5 (list :buf "*code*" :hide-status true)
                    0.5 (list :buf "*fake-seq*" :hide-status true)))
                "#,
            )
            .expect("install custom code/ui layout");
        editor.refresh_runtime_side_effects();

        let src = std::fs::read_to_string("metal-seq-grid.lisp").expect("read grid lisp");
        let overlays = editor.snapshot_file_backed_sources();
        let report = editor.runtime_mut().eval_source_transactional(
            Some(PathBuf::from("metal-seq-grid.lisp")),
            &src,
            overlays,
        );
        assert!(
            report.success,
            "failed to reload grid UI: {}",
            report.failure_message()
        );
        editor.process_lisp_reload_report(report);

        assert_eq!(
            collect_tile_buffer_names(&editor),
            vec!["*code*".to_string(), "*fake-seq*".to_string()],
            "reloading metal-seq-grid.lisp should refresh definitions without replacing the active layout"
        );
    }

    #[test]
    fn metal_seq_script_picker_lists_lisp_scripts() {
        let mut editor = full_grid_editor_for_scroll_tests();

        let value = editor
            .runtime_mut()
            .eval_str(
                r#"
                (map
                  (lambda (entry) (get entry :name))
                  (filter
                    (lambda (entry) (seq-script-entry-visible? entry))
                    (list-directory seq-script-picker-current-dir)))
                "#,
            )
            .expect("list script picker entries")
            .expect("script picker entries value");
        let Value::List(items) = value else {
            panic!("expected script picker entries to be a list");
        };
        let text = items
            .iter()
            .map(|item| match &*item.borrow() {
                Value::String(name) => name.clone(),
                other => panic!("expected script name string, got {other:?}"),
            })
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            text.contains("graph-neural-8x8-demo.lisp"),
            "script picker should list lisp scripts; entries:\n{text}"
        );
        assert!(
            text.contains("graph-neural-16-demo.lisp"),
            "script picker should include graph demos; entries:\n{text}"
        );
        assert!(
            !text.contains("generate_dgenlisp_api.py"),
            "script picker should filter non-lisp files; entries:\n{text}"
        );

        let scratch_entry = editor
            .runtime_mut()
            .eval_str(r#"(seq-script-scratch-entry "scripts/graph-neural-8x8-demo.lisp")"#)
            .expect("build script scratch entry")
            .expect("script scratch entry value");
        let Value::List(lines) = scratch_entry else {
            panic!("expected script scratch entry to be a list");
        };
        let lines = lines
            .iter()
            .map(|line| match &*line.borrow() {
                Value::String(line) => line.clone(),
                other => panic!("expected scratch entry line string, got {other:?}"),
            })
            .collect::<Vec<_>>();
        assert_eq!(
            lines,
            vec!["(load \"crates/sequencer/scripts/graph-neural-8x8-demo.lisp\")"],
            "scratch entry should persist only the project-relative load form"
        );
    }

    #[test]
    fn metal_seq_script_picker_load_returns_to_source_and_registers_tab() {
        let mut editor = full_grid_editor_for_scroll_tests();
        let script_path = std::env::temp_dir().join(format!(
            "eseq-script-picker-test-{}.lisp",
            std::process::id()
        ));
        std::fs::write(
            &script_path,
            r#"
            (def script-buffer-name "*picker-test-seq*")
            (def script-tab-label "Picker Test")
            (def script-init-called false)
            (def script-init-fn ()
              (do
                (set! script-init-called true)
                (effect-buffer "*picker-test-seq*" (label "picker test"))))
            "#,
        )
        .expect("write script picker fixture");

        editor
            .runtime_mut()
            .eval_str(
                r#"
                (set-window-buffer "*sequencer*")
                (set! seq-script-picker-source-buffer "*sequencer*")
                "#,
            )
            .expect("select sequencer source buffer");
        editor.refresh_runtime_side_effects();
        assert_eq!(editor.active_buffer().name, "*sequencer*");

        editor
            .runtime_mut()
            .eval_str(
                r#"
                (create-buffer "*scripts*")
                "#,
            )
            .expect("show script picker buffer");
        editor.refresh_runtime_side_effects();
        assert_eq!(editor.active_buffer().name, "*scripts*");

        let load_form = format!(
            "(seq-script-load-file {:?})",
            script_path.display().to_string()
        );
        editor
            .runtime_mut()
            .eval_str(&load_form)
            .expect("load selected script through script picker");
        editor.refresh_runtime_side_effects();
        let _ = std::fs::remove_file(&script_path);

        assert_eq!(
            editor.active_buffer().name,
            "*sequencer*",
            "script picker should return to the buffer that opened it instead of showing scratch"
        );
        assert_eq!(
            editor
                .runtime_mut()
                .eval_str("script-init-called")
                .expect("read script init flag"),
            Some(Value::Bool(true)),
            "script picker should call script-init-fn after loading the script"
        );
        assert_eq!(
            tile_tabs_for_buffer(&editor, "*sequencer*"),
            vec![
                ("Seq".to_string(), "*sequencer*".to_string()),
                ("Picker Test".to_string(), "*picker-test-seq*".to_string())
            ],
            "loading through the picker should register the script buffer tab on the first load"
        );
    }

    #[test]
    fn metal_seq_16_cycle_script_effect_buffer_matches_script_contract() {
        let src = std::fs::read_to_string("scripts/graph-neural-16-cycle-demo.lisp")
            .expect("read 16-cycle script");
        let tokens = Parser::new(src).parse().expect("tokenize 16-cycle script");
        let exprs = ASTParser::new(tokens)
            .parse()
            .expect("parse 16-cycle script");

        let mut script_buffer_name = None;
        let mut effect_buffer_targets = Vec::new();
        for expr in &exprs {
            let Expression::List(items) = expr else {
                continue;
            };
            match items.as_slice() {
                [Expression::Symbol(form), Expression::Symbol(name), Expression::String(value), ..]
                    if form == "def" && name == "script-buffer-name" =>
                {
                    script_buffer_name = Some(value.clone());
                }
                [Expression::Symbol(form), Expression::String(target), ..]
                    if form == "effect-buffer" =>
                {
                    effect_buffer_targets.push(target.clone());
                }
                [Expression::Symbol(form), Expression::Symbol(target), ..]
                    if form == "effect-buffer" =>
                {
                    panic!(
                        "effect-buffer target names are literal; pass a string target, not symbol {target}"
                    );
                }
                _ => {}
            }
        }

        let script_buffer_name =
            script_buffer_name.expect("script should define script-buffer-name");
        assert_eq!(
            effect_buffer_targets,
            vec![script_buffer_name],
            "16-cycle script tab registration and effect-buffer must target the same buffer"
        );
    }

    #[test]
    fn metal_seq_piano_roll_placement_preference_controls_next_tab() {
        let mut editor = full_grid_editor_for_scroll_tests();

        assert_eq!(
            editor
                .runtime_mut()
                .eval_str("piano-roll-placement")
                .unwrap(),
            Some(Value::Keyword("bottom".to_string()))
        );
        assert_eq!(
            editor.runtime_mut().eval_str("step-panel-buffer").unwrap(),
            Some(Value::String("*sequencer*".to_string()))
        );
        assert_eq!(
            editor.runtime_mut().eval_str("lower-panel-buffer").unwrap(),
            Some(Value::String("*fx*".to_string()))
        );

        editor
            .runtime_mut()
            .eval_str("(seq-toggle-piano-roll-placement)")
            .expect("switch placement preference to main");
        assert_eq!(
            editor
                .runtime_mut()
                .eval_str("piano-roll-placement")
                .unwrap(),
            Some(Value::Keyword("main".to_string()))
        );
        assert_eq!(
            editor.runtime_mut().eval_str("step-panel-buffer").unwrap(),
            Some(Value::String("*sequencer*".to_string())),
            "changing placement while closed must not open or move piano roll"
        );

        editor
            .runtime_mut()
            .eval_str("(seq-toggle-main-or-piano-roll)")
            .expect("open piano roll in preferred main panel");
        assert_eq!(
            editor.runtime_mut().eval_str("step-panel-buffer").unwrap(),
            Some(Value::String("*piano-roll*".to_string()))
        );
        assert_eq!(
            editor.runtime_mut().eval_str("lower-panel-buffer").unwrap(),
            Some(Value::String("*fx*".to_string()))
        );

        editor
            .runtime_mut()
            .eval_str("(seq-toggle-piano-roll-placement)")
            .expect("switch placement preference to bottom and move open piano roll");
        assert_eq!(
            editor
                .runtime_mut()
                .eval_str("piano-roll-placement")
                .unwrap(),
            Some(Value::Keyword("bottom".to_string()))
        );
        assert_eq!(
            editor.runtime_mut().eval_str("step-panel-buffer").unwrap(),
            Some(Value::String("*sequencer*".to_string()))
        );
        assert_eq!(
            editor.runtime_mut().eval_str("lower-panel-buffer").unwrap(),
            Some(Value::String("*piano-roll*".to_string()))
        );
    }

    #[test]
    fn metal_seq_bottom_piano_roll_layout_expands_track_panel_over_step_panel() {
        let mut editor = full_grid_editor_for_scroll_tests();

        editor
            .runtime_mut()
            .eval_str("(seq-toggle-main-or-piano-roll)")
            .expect("open piano roll in bottom panel");
        editor.refresh_runtime_side_effects();

        let tile_buffers = collect_tile_buffer_names(&editor);
        assert!(
            tile_buffers.contains(&"*track*".to_string()),
            "bottom piano roll layout should keep the track parameters panel visible: {tile_buffers:?}"
        );
        assert!(
            !tile_buffers.contains(&"*metal*".to_string())
                && !tile_buffers.contains(&"*sequencer*".to_string()),
            "bottom piano roll layout should replace the step panel with the expanded track panel: {tile_buffers:?}"
        );

        let track_count = tile_buffers
            .iter()
            .filter(|name| name.as_str() == "*track*")
            .count();
        assert_eq!(
            track_count, 1,
            "bottom piano roll layout should have one expanded track tile: {tile_buffers:?}"
        );
    }

    #[test]
    fn metal_seq_samples_sidebar_toggle_hides_and_restores_samples_tile() {
        let mut editor = full_grid_editor_for_scroll_tests();

        assert!(
            collect_tile_buffer_names(&editor).contains(&"*samples*".to_string()),
            "default layout should show the samples sidebar"
        );

        editor
            .runtime_mut()
            .eval_str("(seq-toggle-samples-sidebar)")
            .expect("hide samples sidebar");
        editor.refresh_runtime_side_effects();

        assert_eq!(
            editor
                .runtime_mut()
                .eval_str("samples-sidebar-visible")
                .unwrap(),
            Some(Value::Bool(false))
        );
        let hidden_buffers = collect_tile_buffer_names(&editor);
        assert!(
            !hidden_buffers.contains(&"*samples*".to_string()),
            "hidden samples sidebar should be removed from the tile layout: {hidden_buffers:?}"
        );
        for expected in ["*transport*", "*sequencer*", "*track*", "*mixer*", "*fx*"] {
            assert!(
                hidden_buffers.contains(&expected.to_string()),
                "hiding samples should preserve {expected}: {hidden_buffers:?}"
            );
        }

        editor
            .runtime_mut()
            .eval_str("(seq-toggle-samples-sidebar)")
            .expect("show samples sidebar");
        editor.refresh_runtime_side_effects();

        assert_eq!(
            editor
                .runtime_mut()
                .eval_str("samples-sidebar-visible")
                .unwrap(),
            Some(Value::Bool(true))
        );
        assert!(
            collect_tile_buffer_names(&editor).contains(&"*samples*".to_string()),
            "second toggle should restore the samples sidebar"
        );
    }

    #[test]
    fn metal_seq_instrument_patcher_layout_uses_transport_patcher_and_three_part_bottom_bar() {
        let mut editor = full_grid_editor_for_scroll_tests();
        editor.create_scratch_buffer(
            "*instrument-patcher:test*",
            "",
            eseqlisp::BufferMode::ESeqLisp,
        );

        editor
            .runtime_mut()
            .eval_str(r#"(seq-apply-instrument-patcher-layout "*instrument-patcher:test*")"#)
            .expect("apply instrument patcher layout");
        editor.refresh_runtime_side_effects();

        let frame = eseqlisp::frame::build_tiled_render_frame_borderless(&mut editor, 180, 90);
        let tile = |name: &str| {
            frame
                .tiles
                .iter()
                .find(|tile| tile.frame.buffer_name == name)
                .unwrap_or_else(|| panic!("expected tile for {name}"))
        };

        let transport = tile("*transport*").rect;
        let patcher = tile("*instrument-patcher:test*").rect;
        let samples = tile("*samples*").rect;
        let mixer = tile("*mixer*").rect;
        let fx = tile("*fx*").rect;

        assert!(
            transport.row < patcher.row && patcher.row + patcher.height <= samples.row,
            "patcher mode should stack transport, patcher, then bottom bar; transport={transport:?} patcher={patcher:?} samples={samples:?}"
        );
        assert!(
            patcher.height > samples.height * 4.0,
            "patcher should dominate the viewport; patcher={patcher:?} bottom={samples:?}"
        );
        assert!(
            (samples.height - 13.0).abs() < 0.75
                && (mixer.height - 13.0).abs() < 0.75
                && (fx.height - 13.0).abs() < 0.75,
            "bottom bar should use the fixed mixer-panel height; samples={samples:?} mixer={mixer:?} fx={fx:?}"
        );
        assert!(
            (samples.width - mixer.width).abs() <= 1.0
                && (mixer.width - fx.width).abs() <= 1.0
                && (samples.row - mixer.row).abs() <= 0.1
                && (mixer.row - fx.row).abs() <= 0.1,
            "samples/mixer/fx should divide the bottom bar into three horizontal parts; samples={samples:?} mixer={mixer:?} fx={fx:?}"
        );
    }

    #[test]
    fn metal_seq_samples_sidebar_toggle_hides_patcher_samples_tile() {
        let mut editor = full_grid_editor_for_scroll_tests();
        editor.create_scratch_buffer(
            "*instrument-patcher:test*",
            "",
            eseqlisp::BufferMode::ESeqLisp,
        );

        editor
            .runtime_mut()
            .eval_str(r#"(seq-apply-instrument-patcher-layout "*instrument-patcher:test*")"#)
            .expect("apply instrument patcher layout");
        editor
            .runtime_mut()
            .eval_str("(seq-toggle-samples-sidebar)")
            .expect("hide samples sidebar");
        editor.refresh_runtime_side_effects();

        let hidden_buffers = collect_tile_buffer_names(&editor);
        assert!(
            !hidden_buffers.contains(&"*samples*".to_string()),
            "hidden samples sidebar should be removed from patcher layout: {hidden_buffers:?}"
        );

        let frame = eseqlisp::frame::build_tiled_render_frame_borderless(&mut editor, 180, 90);
        let tile = |name: &str| {
            frame
                .tiles
                .iter()
                .find(|tile| tile.frame.buffer_name == name)
                .unwrap_or_else(|| panic!("expected tile for {name}"))
        };

        let mixer = tile("*mixer*").rect;
        let fx = tile("*fx*").rect;
        assert!(
            (mixer.row - fx.row).abs() <= 0.1
                && (mixer.height - fx.height).abs() <= 0.75
                && mixer.width > 0.0
                && fx.width > 0.0,
            "with samples hidden, mixer/fx should remain visible in the patcher bottom bar; mixer={mixer:?} fx={fx:?}"
        );
    }

    #[test]
    fn metal_seq_dot_global_binding_toggles_recording_outside_editable_text_buffers() {
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

        let mut editor = full_grid_editor_for_scroll_tests();
        editor
            .runtime_mut()
            .eval_str(
                r#"
                  (def record-toggle-count (state 0))
                  (def seq-toggle-record () (set! record-toggle-count (+ record-toggle-count 1)))
                "#,
            )
            .expect("install record toggle test hook");
        editor.refresh_runtime_side_effects();

        editor
            .runtime_mut()
            .eval_str(r#"(set-window-buffer "*fx*")"#)
            .expect("switch to fx buffer");
        editor.refresh_runtime_side_effects();
        editor.handle_key(KeyEvent::new(KeyCode::Char('.'), KeyModifiers::NONE));

        assert_eq!(
            editor
                .runtime_mut()
                .eval_str("record-toggle-count")
                .unwrap(),
            Some(Value::Number(1.0))
        );

        editor
            .runtime_mut()
            .eval_str(r#"(set-window-buffer "*piano-roll*")"#)
            .expect("switch to piano roll buffer");
        editor.refresh_runtime_side_effects();
        editor.handle_key(KeyEvent::new(KeyCode::Char('.'), KeyModifiers::NONE));

        assert_eq!(
            editor
                .runtime_mut()
                .eval_str("record-toggle-count")
                .unwrap(),
            Some(Value::Number(2.0))
        );

        editor.open_scratch_buffer("*editable*", "");
        editor.handle_key(KeyEvent::new(KeyCode::Char('.'), KeyModifiers::NONE));

        assert_eq!(
            editor.active_buffer().text(),
            ".",
            "editable buffers should keep normal text insertion"
        );
        assert_eq!(
            editor
                .runtime_mut()
                .eval_str("record-toggle-count")
                .unwrap(),
            Some(Value::Number(2.0))
        );
    }

    #[test]
    fn metal_seq_global_step_edit_shortcuts_work_outside_grid_buffers() {
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

        let mut editor = full_grid_editor_for_scroll_tests();
        editor
            .runtime_mut()
            .eval_str(
                r#"
                  (def cursor-left-count (state 0))
                  (def cursor-right-count (state 0))
                  (def delete-count (state 0))
                  (def cursor-left () (set! cursor-left-count (+ cursor-left-count 1)))
                  (def cursor-right () (set! cursor-right-count (+ cursor-right-count 1)))
                  (def delete-selected-steps () (set! delete-count (+ delete-count 1)))
                "#,
            )
            .expect("install step edit shortcut hooks");
        editor.refresh_runtime_side_effects();

        let state = Arc::new(SequencerState::new(1, vec![]));
        let current_track = Arc::new(AtomicUsize::new(0));
        let selected_steps = Arc::new(Mutex::new(HashSet::new()));
        let step_clipboard = Arc::new(Mutex::new(None));

        editor
            .runtime_mut()
            .eval_str(r#"(set-window-buffer "*fx*")"#)
            .expect("switch to fx buffer");
        editor.refresh_runtime_side_effects();

        assert!(handle_metal_command_shortcut(
            &mut editor,
            &KeyEvent::new(KeyCode::Left, KeyModifiers::NONE),
            &state,
            &current_track,
            &selected_steps,
            &step_clipboard,
        ));
        assert!(handle_metal_command_shortcut(
            &mut editor,
            &KeyEvent::new(KeyCode::Right, KeyModifiers::NONE),
            &state,
            &current_track,
            &selected_steps,
            &step_clipboard,
        ));
        assert!(!handle_metal_command_shortcut(
            &mut editor,
            &KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE),
            &state,
            &current_track,
            &selected_steps,
            &step_clipboard,
        ));
        assert_eq!(
            editor.runtime_mut().eval_str("delete-count").unwrap(),
            Some(Value::Number(0.0))
        );

        selected_steps.lock().unwrap().insert(3);
        assert!(handle_metal_command_shortcut(
            &mut editor,
            &KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE),
            &state,
            &current_track,
            &selected_steps,
            &step_clipboard,
        ));

        assert_eq!(
            editor.runtime_mut().eval_str("cursor-left-count").unwrap(),
            Some(Value::Number(1.0))
        );
        assert_eq!(
            editor.runtime_mut().eval_str("cursor-right-count").unwrap(),
            Some(Value::Number(1.0))
        );
        assert_eq!(
            editor.runtime_mut().eval_str("delete-count").unwrap(),
            Some(Value::Number(1.0))
        );

        editor
            .runtime_mut()
            .eval_str(r#"(set-window-buffer "*piano-roll*")"#)
            .expect("switch to piano roll buffer");
        editor.refresh_runtime_side_effects();
        assert!(!handle_metal_command_shortcut(
            &mut editor,
            &KeyEvent::new(KeyCode::Left, KeyModifiers::NONE),
            &state,
            &current_track,
            &selected_steps,
            &step_clipboard,
        ));

        editor.open_scratch_buffer("*editable*", "");
        assert!(!handle_metal_command_shortcut(
            &mut editor,
            &KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE),
            &state,
            &current_track,
            &selected_steps,
            &step_clipboard,
        ));
    }

    #[test]
    fn metal_seq_copy_paste_step_shortcuts_work_outside_grid_buffers() {
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

        let mut editor = full_grid_editor_for_scroll_tests();
        let state = Arc::new(SequencerState::new(1, vec![]));
        let current_track = Arc::new(AtomicUsize::new(0));
        let selected_steps = Arc::new(Mutex::new(HashSet::new()));
        let step_clipboard = Arc::new(Mutex::new(None));
        let ui_epoch = AtomicUsize::new(0);

        state.pattern.patterns[0].set_step_active(0, true);
        editor
            .runtime_mut()
            .eval_str(
                r#"
                  (set-window-buffer "*fx*")
                  (set! cursor-step 0)
                "#,
            )
            .expect("switch to fx buffer and set cursor");
        editor.refresh_runtime_side_effects();

        assert!(handle_metal_command_shortcut_with_ui_epoch(
            &mut editor,
            &KeyEvent::new(KeyCode::Char('c'), KeyModifiers::SUPER),
            &state,
            &current_track,
            &selected_steps,
            &step_clipboard,
            &ui_epoch,
        ));
        assert!(step_clipboard.lock().unwrap().is_some());
        assert_eq!(
            ui_epoch.load(Ordering::Relaxed),
            0,
            "copy should not invalidate sequencer UI state"
        );

        editor
            .runtime_mut()
            .eval_str("(set! cursor-step 1)")
            .expect("move cursor");
        assert!(handle_metal_command_shortcut_with_ui_epoch(
            &mut editor,
            &KeyEvent::new(KeyCode::Char('v'), KeyModifiers::SUPER),
            &state,
            &current_track,
            &selected_steps,
            &step_clipboard,
            &ui_epoch,
        ));
        assert!(state.pattern.patterns[0].is_active(1));
        assert_eq!(
            ui_epoch.load(Ordering::Relaxed),
            1,
            "paste must invalidate sequencer UI state so expanded tracks repaint"
        );

        editor
            .runtime_mut()
            .eval_str(r#"(set-window-buffer "*piano-roll*")"#)
            .expect("switch to piano roll buffer");
        editor.refresh_runtime_side_effects();
        assert!(!handle_metal_command_shortcut(
            &mut editor,
            &KeyEvent::new(KeyCode::Char('c'), KeyModifiers::SUPER),
            &state,
            &current_track,
            &selected_steps,
            &step_clipboard,
        ));

        editor.open_scratch_buffer("*editable*", "");
        assert!(!handle_metal_command_shortcut(
            &mut editor,
            &KeyEvent::new(KeyCode::Char('v'), KeyModifiers::SUPER),
            &state,
            &current_track,
            &selected_steps,
            &step_clipboard,
        ));
    }

    #[test]
    fn metal_seq_full_grid_fx_panel_does_not_y_scroll_when_content_fits() {
        use crossterm::event::{KeyModifiers, MouseEvent, MouseEventKind};

        fn mouse_event(kind: MouseEventKind, column: u16, row: u16) -> MouseEvent {
            MouseEvent {
                kind,
                column,
                row,
                modifiers: KeyModifiers::NONE,
            }
        }

        fn layout_bottom(node: &eseqlisp::layout::LayoutNode) -> f32 {
            node.children
                .iter()
                .map(layout_bottom)
                .fold(node.rect.row + node.rect.height, f32::max)
        }

        let mut editor = full_grid_editor_for_scroll_tests();
        let frame = eseqlisp::frame::build_tiled_render_frame_borderless(&mut editor, 180, 90);
        let fx_tile = frame
            .tiles
            .iter()
            .find(|tile| tile.frame.buffer_name == "*fx*")
            .expect("full grid layout should contain *fx* tile");
        let fx_tile_id = fx_tile.tile_id;
        let content_bottom = fx_tile
            .frame
            .widget_layout
            .as_ref()
            .map(|layout| layout_bottom(layout))
            .expect("fx tile should have widget layout");
        let fx_leaf = editor
            .tile_root
            .find_leaf(fx_tile_id)
            .expect("fx tile leaf should exist");

        let before = fx_leaf.widget_scroll_top;
        let scroll_col = (fx_tile.rect.col + fx_tile.rect.width * 0.5).floor() as u16;
        let scroll_row = (fx_tile.rect.row + fx_tile.rect.height * 0.5).floor() as u16;
        editor.handle_tiled_mouse_precise(
            mouse_event(MouseEventKind::ScrollDown, scroll_col, scroll_row),
            scroll_col as f32 + 0.5,
            scroll_row as f32 + 0.5,
            0,
        );
        let after = editor
            .tile_root
            .find_leaf(fx_tile_id)
            .expect("fx tile leaf should still exist")
            .widget_scroll_top;

        assert_eq!(
            after,
            before,
            "*fx* content fits but vertical scroll changed; tile_id={fx_tile_id}, content_bottom={content_bottom:.3}, viewport={:.3}",
            editor
                .tile_root
                .find_leaf(fx_tile_id)
                .expect("fx tile leaf should still exist")
                .widget_viewport_height
        );
    }

    #[test]
    fn metal_seq_full_grid_empty_sequencer_buffer_does_not_y_scroll_on_load() {
        use crossterm::event::{KeyModifiers, MouseEvent, MouseEventKind};

        fn mouse_event(kind: MouseEventKind, column: u16, row: u16) -> MouseEvent {
            MouseEvent {
                kind,
                column,
                row,
                modifiers: KeyModifiers::NONE,
            }
        }

        let mut editor = full_grid_editor_for_scroll_tests();
        editor
            .runtime_mut()
            .set_reactive("SEQ", "num-tracks", Value::Number(0.0));
        editor.runtime_mut().run_reactive_cycle();
        editor.refresh_runtime_side_effects();
        if let Some(status) = editor.runtime_mut().take_status_message() {
            panic!("empty startup grid refresh should not report status: {status}");
        }

        let frame = eseqlisp::frame::build_tiled_render_frame_borderless(&mut editor, 180, 90);
        let sequencer_tile = frame
            .tiles
            .iter()
            .find(|tile| tile.frame.buffer_name == "*sequencer*")
            .expect("full grid layout should default to a *sequencer* tile");
        let sequencer_tile_id = sequencer_tile.tile_id;
        let layout = sequencer_tile
            .frame
            .widget_layout
            .as_ref()
            .expect("sequencer tile should have widget layout");
        let viewport_height = editor
            .tile_root
            .find_leaf(sequencer_tile_id)
            .expect("sequencer tile leaf should exist")
            .widget_viewport_height;
        let content_bottom = layout_bottom(layout);

        assert!(
            content_bottom <= viewport_height + 0.01,
            "empty *sequencer* content should fit tiled viewport without scroll overflow; content_bottom={content_bottom:.3}, viewport={viewport_height:.3}"
        );

        let before = editor
            .tile_root
            .find_leaf(sequencer_tile_id)
            .expect("sequencer tile leaf should exist")
            .widget_scroll_top;
        let scroll_col = (sequencer_tile.rect.col + sequencer_tile.rect.width * 0.5).floor() as u16;
        let scroll_row =
            (sequencer_tile.rect.row + sequencer_tile.rect.height * 0.5).floor() as u16;
        editor.handle_tiled_mouse_precise(
            mouse_event(MouseEventKind::ScrollDown, scroll_col, scroll_row),
            scroll_col as f32 + 0.5,
            scroll_row as f32 + 0.5,
            0,
        );
        let after = editor
            .tile_root
            .find_leaf(sequencer_tile_id)
            .expect("sequencer tile leaf should still exist")
            .widget_scroll_top;

        assert_eq!(
            after, before,
            "empty *sequencer* tiled startup layout fits but wheel scroll changed; tile_id={sequencer_tile_id}, content_bottom={content_bottom:.3}, viewport={viewport_height:.3}"
        );
    }

    #[test]
    fn metal_seq_empty_metal_buffer_centers_prompt_without_overflow() {
        let mut editor = full_grid_editor_for_scroll_tests();
        editor
            .runtime_mut()
            .set_reactive("SEQ", "num-tracks", Value::Number(0.0));
        editor.runtime_mut().run_reactive_cycle();
        editor.refresh_runtime_side_effects();
        if let Some(status) = editor.runtime_mut().take_status_message() {
            panic!("empty metal refresh should not report status: {status}");
        }

        let metal_id = editor
            .buffers
            .iter()
            .find(|buffer| buffer.name == "*metal*")
            .expect("metal buffer should exist")
            .id;
        editor.set_active_buffer(metal_id);
        editor.set_layout_viewport(120, 36);
        let layout = editor.widget_layout().expect("empty metal layout");
        let prompt = find_layout_node_by_text(&layout, "Select a sound to create a track")
            .expect("empty metal prompt should be present");
        let prompt_center = prompt.rect.row + prompt.rect.height * 0.5;

        assert!(
            (prompt_center - 18.0).abs() <= 2.0,
            "empty metal prompt should be vertically centered, got rect {:?}",
            prompt.rect
        );
        assert!(
            layout_bottom(&layout) <= 36.01,
            "empty metal fallback should fit viewport without scroll overflow; bottom={:.3}",
            layout_bottom(&layout)
        );
    }

    #[test]
    fn metal_seq_empty_fx_panel_centers_prompt_without_overflow() {
        let mut editor = full_grid_editor_for_scroll_tests();
        editor
            .runtime_mut()
            .set_reactive("SEQ", "num-tracks", Value::Number(0.0));
        editor.runtime_mut().run_reactive_cycle();
        editor.refresh_runtime_side_effects();
        if let Some(status) = editor.runtime_mut().take_status_message() {
            panic!("empty fx refresh should not report status: {status}");
        }

        let fx_id = editor
            .buffers
            .iter()
            .find(|buffer| buffer.name == "*fx*")
            .expect("fx buffer should exist")
            .id;
        editor.set_active_buffer(fx_id);
        editor.set_layout_viewport(120, 10);
        let layout = editor.widget_layout().expect("empty fx layout");
        let prompt = find_layout_node_by_text(&layout, "Instrument and effects appear here")
            .expect("empty fx prompt should be present");
        let prompt_center = prompt.rect.row + prompt.rect.height * 0.5;

        assert!(
            (prompt_center - 5.0).abs() <= 1.0,
            "empty fx prompt should be vertically centered, got rect {:?}",
            prompt.rect
        );
        assert!(
            layout_bottom(&layout) <= 10.01,
            "empty fx fallback should fit viewport without scroll overflow; bottom={:.3}",
            layout_bottom(&layout)
        );
    }

    #[test]
    fn metal_seq_track_panel_renders_selected_neuron_plock_rows() {
        let mut editor = full_grid_editor_for_scroll_tests();
        let mut plock = HashMap::new();
        plock.insert(
            "label".to_string(),
            Rc::new(RefCell::new(Value::String("N1".to_string()))),
        );
        plock.insert(
            "step-idx".to_string(),
            Rc::new(RefCell::new(Value::Number(0.0))),
        );
        plock.insert(
            "neuron-idx".to_string(),
            Rc::new(RefCell::new(Value::Number(0.0))),
        );
        plock.insert(
            "target".to_string(),
            Rc::new(RefCell::new(Value::String("neural-instrument".to_string()))),
        );
        plock.insert(
            "network-id".to_string(),
            Rc::new(RefCell::new(Value::Number(11.0))),
        );
        plock.insert(
            "target-track".to_string(),
            Rc::new(RefCell::new(Value::Number(0.0))),
        );
        plock.insert(
            "param-idx".to_string(),
            Rc::new(RefCell::new(Value::Number(3.0))),
        );
        plock.insert(
            "group".to_string(),
            Rc::new(RefCell::new(Value::String("T1 inst".to_string()))),
        );
        plock.insert(
            "name".to_string(),
            Rc::new(RefCell::new(Value::String("resonance".to_string()))),
        );
        plock.insert(
            "value".to_string(),
            Rc::new(RefCell::new(Value::Number(0.75))),
        );
        plock.insert("min".to_string(), Rc::new(RefCell::new(Value::Number(0.0))));
        plock.insert("max".to_string(), Rc::new(RefCell::new(Value::Number(1.0))));
        plock.insert(
            "source".to_string(),
            Rc::new(RefCell::new(Value::String("neuron".to_string()))),
        );
        editor.runtime_mut().set_reactive(
            "SEQ",
            "track-plocks",
            Value::List(vec![Rc::new(RefCell::new(Value::Map(plock)))]),
        );
        editor.runtime_mut().run_reactive_cycle();
        editor.refresh_runtime_side_effects();

        let track_id = editor
            .buffers
            .iter()
            .find(|buffer| buffer.name == "*track*")
            .expect("track buffer should exist")
            .id;
        editor.set_active_buffer(track_id);
        editor.set_layout_viewport(80, 20);
        let layout = editor.widget_layout().expect("track panel layout");
        let label = find_layout_node_by_text(&layout, "N1")
            .expect("selected neuron p-lock label should render");

        assert_finite_nonzero_rect(label, "selected neuron p-lock label");

        let clear = find_layout_node_by_text(&layout, "x")
            .expect("selected neuron p-lock clear button should render");
        assert_eq!(clear.widget_type, "button");
        assert_finite_nonzero_rect(clear, "selected neuron p-lock clear button");

        let _ = editor.drain_host_commands();
        let callback = clear
            .props
            .get("on-click")
            .cloned()
            .expect("selected neuron p-lock clear button on-click");
        editor
            .runtime_mut()
            .invoke(
                callback,
                vec![Value::Number(0.0), Value::Number(0.0), Value::Bool(false)],
            )
            .expect("invoke selected neuron p-lock clear button");
        let commands = editor.drain_host_commands();
        assert_eq!(commands.len(), 1);
        match &commands[0] {
            eseqlisp::host::HostCommand::Custom { name, payload } => {
                assert_eq!(name, "clear-track-plock-entry");
                let Value::Map(payload) = payload else {
                    panic!("expected clear-track-plock-entry map payload, got {payload:?}");
                };
                assert_eq!(
                    value_map_string(payload, "target").as_deref(),
                    Some("neural-instrument")
                );
                assert_eq!(value_map_number(payload, "network-id"), Some(11.0));
                assert_eq!(value_map_number(payload, "neuron-idx"), Some(0.0));
                assert_eq!(value_map_number(payload, "target-track"), Some(0.0));
                assert_eq!(value_map_number(payload, "param-idx"), Some(3.0));
            }
            other => panic!("expected clear-track-plock-entry host command, got {other:?}"),
        }
    }

    #[test]
    fn metal_seq_transport_playhead_is_render_bound() {
        let mut editor = full_grid_editor_for_scroll_tests();
        let _ = editor.runtime_mut().take_pending_buffer_widget_trees();

        editor
            .runtime_mut()
            .set_reactive("SEQ", "transport-playhead", Value::Number(12.0));
        editor.runtime_mut().run_reactive_cycle();

        assert!(
            editor
                .runtime_mut()
                .take_pending_buffer_widget_trees()
                .is_empty(),
            "bound transport playhead updates must not enqueue transport widget tree rebuilds"
        );
    }

    #[test]
    fn metal_seq_transport_master_record_button_has_visible_rect() {
        let mut editor = full_grid_editor_for_scroll_tests();
        editor
            .runtime_mut()
            .eval_str(r#"(set-window-buffer "*transport*")"#)
            .expect("switch to transport buffer");
        editor.refresh_runtime_side_effects();

        let layout = editor.widget_layout().expect("transport layout");
        let button = find_layout_node_by_stable_key(&layout, "transport-master-record-button")
            .expect("master recording button");
        assert_finite_nonzero_rect(button, "master recording button");
        assert!(
            button.rect.col >= layout.rect.col
                && button.rect.row >= layout.rect.row
                && button.rect.col + button.rect.width <= layout.rect.col + layout.rect.width
                && button.rect.row + button.rect.height <= layout.rect.row + layout.rect.height,
            "master recording button should be inside the transport panel: button={:?} panel={:?}",
            button.rect,
            layout.rect
        );

        let samples_button =
            find_layout_node_by_stable_key(&layout, "transport-samples-sidebar-button")
                .expect("samples sidebar button");
        let save_button =
            find_layout_node_by_stable_key(&layout, "transport-save-button").expect("save button");
        assert_finite_nonzero_rect(samples_button, "samples sidebar button");
        assert_finite_nonzero_rect(save_button, "save button");
        assert!(
            samples_button.rect.col + samples_button.rect.width <= save_button.rect.col,
            "samples sidebar button should be to the left of save; samples={:?} save={:?}",
            samples_button.rect,
            save_button.rect
        );
    }

    #[test]
    fn metal_seq_duration_mode_renders_all_steps_above_two() {
        let mut editor = full_grid_editor_for_scroll_tests();
        editor
            .runtime_mut()
            .set_reactive("SEQ", "durations", test_number_list(&[8.0; 16]));
        editor
            .runtime_mut()
            .eval_str("(set! param-mode 1)")
            .expect("switch to duration mode");
        editor.refresh_runtime_side_effects();
        if let Some(status) = editor.runtime_mut().take_status_message() {
            panic!("duration mode should render without status error: {status}");
        }

        let metal_id = editor
            .buffers
            .iter()
            .find(|buffer| buffer.name == "*metal*")
            .expect("metal buffer should exist")
            .id;
        editor.set_active_buffer(metal_id);
        editor.set_layout_viewport(120, 40);
        let layout = editor
            .widget_layout()
            .expect("duration mode metal layout should build");

        assert_eq!(
            count_widget_type(&layout, "vslider"),
            16,
            "duration mode should keep rendering one slider per visible step"
        );
    }

    #[test]
    fn metal_seq_sequencer_buffer_renders_step_cells() {
        let mut editor = full_grid_editor_for_scroll_tests();
        let sequencer_id = editor
            .buffers
            .iter()
            .find(|buffer| buffer.name == "*sequencer*")
            .expect("sequencer buffer should exist")
            .id;
        editor.set_active_buffer(sequencer_id);
        editor.set_layout_viewport(140, 20);
        let layout = editor
            .widget_layout()
            .expect("sequencer layout should build");

        assert_eq!(
            count_stable_key_prefix(&layout, "seqv-step-cell-"),
            16,
            "sequencer buffer should render one cell per visible step"
        );

        let mut layout_summaries = Vec::new();
        collect_layout_node_summaries(&layout, &mut layout_summaries);
        assert!(
            find_layout_node_by_stable_key(&layout, "seqv-timebase-0").is_none(),
            "collapsed sequencer rows should not render the removed right-side timebase dropdown: {layout_summaries:#?}"
        );

        let expand =
            find_layout_node_by_stable_key(&layout, "seqv-expand-0").unwrap_or_else(|| {
                panic!("sequencer row expand button missing: {layout_summaries:#?}")
            });
        assert_eq!(expand.widget_type, "box");
        assert!(
            expand.rect.width.is_finite()
                && expand.rect.width > 0.0
                && expand.rect.height.is_finite()
                && expand.rect.height > 0.0,
            "sequencer row expand button should have a finite nonzero rect: {:?}",
            expand.rect
        );
        assert_eq!(
            expand.props.get("background"),
            Some(&Value::String("seqv-ellipsis-button".to_string())),
            "sequencer row expand control should use the SDF ellipsis widget instead of text"
        );
    }

    #[test]
    fn metal_seq_collapsed_tracks_render_compact_mixer_strip_and_hide_sequencer_row() {
        struct TestTextMeasurer;
        impl eseqlisp::layout::TextMeasurer for TestTextMeasurer {
            fn measure_text_px(&self, text: &str, _font_size: f32) -> f32 {
                text.chars().count() as f32 * 8.0
            }

            fn line_height_px(&self, _font_size: f32) -> f32 {
                16.0
            }
        }

        let mut editor = eseqlisp::Editor::new(Runtime::new(), eseqlisp::EditorConfig::default());
        editor.set_text_measurer(Box::new(TestTextMeasurer), 8.0, 16.0);
        editor
            .runtime_mut()
            .register_native("seq-toggle-track-collapsed", |_args, _ctx| {
                Ok(Value::Bool(true))
            });
        editor.runtime_mut().register_reactive(
            "SEQ",
            vec![
                ("num-tracks", Value::Number(3.0)),
                ("track-ids", test_number_list(&[0.0, 1.0, 2.0])),
                ("track-names", test_string_list(&["kick", "snare", "hat"])),
                ("track-colors", test_multi_track_colors(3)),
                ("track-collapsed", test_bool_list(&[false, true, false])),
                ("current-track", Value::Number(0.0)),
                ("delete-target-version", Value::Number(0.0)),
                ("record-armed", test_repeated_bool_list(false, 3)),
                ("track-mutes", test_repeated_bool_list(false, 3)),
                ("track-solos", test_repeated_bool_list(false, 3)),
                ("track-muted-by-solo", test_repeated_bool_list(false, 3)),
                (
                    "track-instrument-types",
                    test_string_list(&["sampler", "sampler", "sampler"]),
                ),
                (
                    "track-mod-output-available",
                    test_repeated_bool_list(false, 3),
                ),
                ("mod-routes", test_list(vec![])),
                ("selected-mod-routes", test_list(vec![])),
                ("track-volumes", test_number_list(&[1.0, 1.0, 1.0])),
                ("track-mixer-pans", test_number_list(&[0.0, 0.0, 0.0])),
                ("track-outputs", test_string_list(&["main", "main", "main"])),
                (
                    "track-output-options",
                    test_string_list(&["main", "sends only", "Bus A", "Bus B"]),
                ),
                (
                    "track-bus-sends",
                    test_list(
                        (0..3)
                            .map(|_| {
                                test_list(vec![
                                    test_track_bus_send(1, "Bus A", 0.0),
                                    test_track_bus_send(2, "Bus B", 0.0),
                                ])
                            })
                            .collect(),
                    ),
                ),
                ("track-num-steps", test_number_list(&[16.0, 16.0, 16.0])),
                ("track-timebases", test_string_list(&["16", "16", "16"])),
                ("bus-names", test_string_list(&["Mix", "Bus A", "Bus B"])),
                ("bus-volumes", test_number_list(&[1.0, 1.0, 1.0])),
                ("bus-mutes", test_repeated_bool_list(false, 3)),
                ("bus-solos", test_repeated_bool_list(false, 3)),
                ("master-peak-l", Value::Number(0.0)),
                ("master-peak-r", Value::Number(0.0)),
                ("bus-peak-0", Value::Number(0.0)),
                ("bus-peak-1", Value::Number(0.0)),
                ("bus-peak-2", Value::Number(0.0)),
            ],
            true,
        );
        editor.runtime_mut().register_reactive("SEQV", vec![], true);
        {
            let rt = editor.runtime_mut();
            for track in 0..3 {
                rt.set_reactive("SEQ", &format!("track-{track}-volume"), Value::Number(1.0));
                rt.set_reactive("SEQ", &format!("track-{track}-pan"), Value::Number(0.0));
                rt.set_reactive("SEQ", &format!("track-peak-{track}"), Value::Number(0.0));
                rt.set_reactive(
                    "SEQ",
                    &format!("mixer-track-delete-target-{track}"),
                    Value::Bool(false),
                );
                rt.set_reactive(
                    "SEQ",
                    &format!("track-selected-{track}"),
                    Value::Bool(track == 0),
                );
                rt.set_reactive(
                    "SEQ",
                    &track_playhead_row_field(track, 0),
                    Value::Number(if track == 0 { 0.0 } else { -1.0 }),
                );
                for bus in 1..=2 {
                    rt.set_reactive(
                        "SEQ",
                        &format!("track-{track}-bus-{bus}-send"),
                        Value::Number(0.0),
                    );
                }
                for step in 0..16 {
                    rt.set_reactive(
                        "SEQ",
                        &track_step_active_field(track, step),
                        Value::Bool(step.is_multiple_of(2)),
                    );
                    rt.set_reactive(
                        "SEQ",
                        &track_step_plocked_field(track, step),
                        Value::Bool(false),
                    );
                    rt.set_reactive(
                        "SEQ",
                        &track_step_selected_field(track, step),
                        Value::Bool(false),
                    );
                    rt.set_reactive(
                        "SEQ",
                        &track_step_duration_field(track, step),
                        Value::Bool(false),
                    );
                }
            }
        }
        editor
            .runtime_mut()
            .eval_str(
                r#"
                  (load "metal-seq-themes.lisp")
                  (seq-theme-mac-osx-dark)
                  (load "metal-seq-materials.lisp")
                  (defstate selected-bus -1)
                  (load "metal-seq-mixer-v2.lisp")
                  (load "metal-seq-sequencer.lisp")
                "#,
            )
            .expect("load mixer and sequencer lisp");
        editor.refresh_runtime_side_effects();

        let mixer_id = editor
            .buffers
            .iter()
            .find(|buffer| buffer.name == "*mixer*")
            .expect("mixer buffer should exist")
            .id;
        editor.set_active_buffer(mixer_id);
        editor.set_layout_viewport(120, 16);
        let mixer_layout = editor
            .widget_layout()
            .expect("collapsed mixer layout should build");

        let compact_badge =
            find_layout_node_by_stable_key(&mixer_layout, "mixer-v2-track-collapsed-label-1")
                .expect("collapsed track badge should render");
        assert_finite_nonzero_rect(compact_badge, "collapsed mixer track badge");
        assert!(
            compact_badge.props.contains_key("on-double-click"),
            "collapsed mixer badge should keep double-click collapse toggling"
        );
        let compact_mute =
            find_layout_node_by_stable_key(&mixer_layout, "mixer-v2-track-collapsed-mute-1")
                .expect("collapsed track mute should render");
        assert_finite_nonzero_rect(compact_mute, "collapsed mixer mute");
        let compact_meter = find_layout_node_by_stable_key(&mixer_layout, "mixer-v2-track-meter-1")
            .expect("collapsed track meter should render");
        assert_finite_nonzero_rect(compact_meter, "collapsed mixer meter");
        assert!(
            find_layout_node_by_stable_key(&mixer_layout, "mixer-v2-track-label-1").is_none(),
            "collapsed mixer track should not render the full-width label"
        );

        let sequencer_id = editor
            .buffers
            .iter()
            .find(|buffer| buffer.name == "*sequencer*")
            .expect("sequencer buffer should exist")
            .id;
        editor.set_active_buffer(sequencer_id);
        editor.set_layout_viewport(140, 30);
        let sequencer_layout = editor
            .widget_layout()
            .expect("collapsed sequencer layout should build");

        assert!(
            find_layout_node_by_stable_key(&sequencer_layout, "sequencer-track-1").is_none(),
            "collapsed track should be omitted from the sequencer row list"
        );
        assert!(find_layout_node_by_stable_key(&sequencer_layout, "sequencer-track-0").is_some());
        assert!(find_layout_node_by_stable_key(&sequencer_layout, "sequencer-track-2").is_some());
        assert_eq!(
            count_stable_key_prefix(&sequencer_layout, "seqv-step-cell-"),
            32,
            "sequencer should render step cells only for visible tracks"
        );
    }

    #[test]
    fn metal_seq_sequencer_ellipsis_toggles_expanded_track_editor() {
        let mut editor = full_grid_editor_for_scroll_tests();
        let sequencer_id = editor
            .buffers
            .iter()
            .find(|buffer| buffer.name == "*sequencer*")
            .expect("sequencer buffer should exist")
            .id;
        editor.set_active_buffer(sequencer_id);
        editor.set_layout_viewport(180, 30);

        let initial_layout = editor
            .widget_layout()
            .expect("sequencer layout should build");
        assert_eq!(
            count_stable_key_prefix(&initial_layout, "seqv-expanded-step-slider-"),
            0,
            "collapsed sequencer rows should not render expanded metal sliders"
        );

        let expand = find_layout_node_by_stable_key(&initial_layout, "seqv-expand-0")
            .expect("sequencer row expand button should render");
        let callback = expand
            .props
            .get("on-click")
            .cloned()
            .expect("sequencer row expand button on-click");
        editor
            .runtime_mut()
            .invoke(
                callback,
                vec![Value::Number(0.0), Value::Number(0.0), Value::Bool(false)],
            )
            .expect("invoke sequencer row expand button");
        editor.refresh_runtime_side_effects();
        if let Some(status) = editor.runtime_mut().take_status_message() {
            if status.to_ascii_lowercase().contains("error") {
                panic!("sequencer row expansion status after click: {status}");
            }
        }

        assert_eq!(
            editor
                .runtime_mut()
                .eval_str("seqv-expanded-track-ids")
                .expect("read expanded track ids"),
            Some(test_number_list(&[0.0])),
            "ellipsis click should add the stable track id to expansion state"
        );

        let expanded_layout = editor
            .widget_layout()
            .expect("expanded sequencer layout should build");
        assert_eq!(
            count_stable_key_prefix(&expanded_layout, "seqv-step-cell-"),
            0,
            "expanded rows should replace the compact dot grid"
        );
        assert_eq!(
            count_stable_key_prefix(&expanded_layout, "seqv-expanded-step-slider-"),
            16,
            "expanded row should render the metal-style step sliders"
        );
        assert_eq!(
            count_stable_key_prefix(&expanded_layout, "seqv-expanded-step-toggle-"),
            16,
            "expanded row should render the metal-style step toggles"
        );
        for key in [
            "seqv-expanded-param-tab-0-0",
            "seqv-expanded-timebase-0",
            "seqv-expanded-param-number-picker-0",
            "seqv-expanded-half-0",
            "seqv-expanded-double-0",
            "seqv-expanded-page-0-0",
        ] {
            let node = find_layout_node_by_stable_key(&expanded_layout, key)
                .unwrap_or_else(|| panic!("missing expanded control {key}"));
            assert_finite_nonzero_rect(node, key);
        }

        let first_tab =
            find_layout_node_by_stable_key(&expanded_layout, "seqv-expanded-param-tab-0-0")
                .expect("expanded first tab should render");
        let track_name = find_layout_node_by_stable_key(&expanded_layout, "seqv-select-0")
            .expect("expanded row track-name block should render");
        assert!(
            first_tab.rect.col < track_name.rect.col,
            "expanded editor should start from the row's left edge, not after the track header"
        );
    }

    #[test]
    fn metal_seq_sequencer_selected_track_row_can_scroll_into_view() {
        let mut editor = full_grid_editor_for_scroll_tests();
        set_full_grid_track_count(&mut editor, 8, 16);
        editor.runtime_mut().run_reactive_cycle();
        editor.refresh_runtime_side_effects();
        let sequencer_id = editor
            .buffers
            .iter()
            .find(|buffer| buffer.name == "*sequencer*")
            .expect("sequencer buffer should exist")
            .id;
        editor.set_active_buffer(sequencer_id);
        editor.set_layout_viewport(140, 12);

        let layout = editor
            .widget_layout()
            .expect("multi-track sequencer layout should build");
        let row = find_layout_node_by_stable_key(&layout, "sequencer-track-7")
            .expect("last sequencer track row should render");
        assert!(
            row.rect.row + row.rect.height > editor.widget_scroll_top() + 12.0,
            "fixture should start with the last track below the viewport"
        );

        assert!(
            editor.ensure_widget_stable_key_visible("sequencer-track-7", 1.0),
            "ensuring the last track should adjust widget scroll"
        );
        let scroll_top = editor.widget_scroll_top();
        assert!(scroll_top > 0.0);
        assert!(
            row.rect.row + row.rect.height <= scroll_top + 12.0,
            "last track should be visible after scrolling, row={:?} scroll_top={scroll_top}",
            row.rect
        );
    }

    #[test]
    fn metal_seq_sequencer_expanded_selected_track_row_can_scroll_into_view_after_height_change() {
        let mut editor = full_grid_editor_for_scroll_tests();
        set_full_grid_track_count(&mut editor, 8, 16);
        editor.runtime_mut().run_reactive_cycle();
        editor.refresh_runtime_side_effects();
        let sequencer_id = editor
            .buffers
            .iter()
            .find(|buffer| buffer.name == "*sequencer*")
            .expect("sequencer buffer should exist")
            .id;
        editor.set_active_buffer(sequencer_id);
        editor.set_layout_viewport(140, 12);

        editor.ensure_widget_stable_key_visible("sequencer-track-7", 1.0);
        editor
            .runtime_mut()
            .eval_str("(seqv-track-menu-click 7)")
            .expect("expand selected bottom row");
        editor.refresh_runtime_side_effects();
        assert!(
            editor.ensure_widget_stable_key_visible("sequencer-track-7", 1.0),
            "expanding a bottom-row track should adjust scroll to keep the larger row visible"
        );

        let layout = editor
            .widget_layout()
            .expect("expanded bottom-row sequencer layout should build");
        let row = find_layout_node_by_stable_key(&layout, "sequencer-track-7")
            .expect("expanded last sequencer track row should render");
        let scroll_top = editor.widget_scroll_top();
        assert!(
            row.rect.row + row.rect.height <= scroll_top + 12.0,
            "expanded selected row should stay in view, row={:?} scroll_top={scroll_top}",
            row.rect
        );
    }

    #[test]
    fn metal_seq_sequencer_expanded_rows_keep_independent_tab_and_page_state() {
        let mut editor = full_grid_editor_for_scroll_tests();
        set_full_grid_track_count(&mut editor, 2, 32);
        editor.runtime_mut().run_reactive_cycle();
        editor.refresh_runtime_side_effects();
        let sequencer_id = editor
            .buffers
            .iter()
            .find(|buffer| buffer.name == "*sequencer*")
            .expect("sequencer buffer should exist")
            .id;
        editor.set_active_buffer(sequencer_id);
        editor.set_layout_viewport(220, 200);
        editor
            .runtime_mut()
            .eval_str("(do (seqv-track-menu-click 0) (seqv-track-menu-click 1))")
            .expect("expand two sequencer rows");
        editor.refresh_runtime_side_effects();

        let layout = editor
            .widget_layout()
            .expect("expanded two-track sequencer layout should build");
        assert_eq!(
            count_stable_key_prefix(&layout, "seqv-expanded-step-slider-"),
            32,
            "two expanded rows should render independent metal-style slider grids"
        );

        let tab_track_0 = find_layout_node_by_stable_key(&layout, "seqv-expanded-param-tab-0-2")
            .expect("track 0 aux tab");
        let tab_track_1 = find_layout_node_by_stable_key(&layout, "seqv-expanded-param-tab-1-4")
            .expect("track 1 pan tab");
        editor
            .runtime_mut()
            .invoke(
                tab_track_0
                    .props
                    .get("on-click")
                    .cloned()
                    .expect("track 0 tab callback"),
                vec![Value::Number(0.0), Value::Number(0.0), Value::Bool(false)],
            )
            .expect("select track 0 aux tab");
        editor
            .runtime_mut()
            .invoke(
                tab_track_1
                    .props
                    .get("on-click")
                    .cloned()
                    .expect("track 1 tab callback"),
                vec![Value::Number(0.0), Value::Number(0.0), Value::Bool(false)],
            )
            .expect("select track 1 pan tab");
        assert_eq!(
            editor
                .runtime_mut()
                .eval_str("(seqv-param-mode 0)")
                .unwrap(),
            Some(Value::Number(2.0))
        );
        assert_eq!(
            editor
                .runtime_mut()
                .eval_str("(seqv-param-mode 1)")
                .unwrap(),
            Some(Value::Number(4.0))
        );

        let paged_layout = editor
            .widget_layout()
            .expect("expanded sequencer layout should rebuild after tab clicks");
        let track_0_page_1 =
            find_layout_node_by_stable_key(&paged_layout, "seqv-expanded-page-0-1")
                .expect("track 0 page 2");
        editor
            .runtime_mut()
            .invoke(
                track_0_page_1
                    .props
                    .get("on-click")
                    .cloned()
                    .expect("track 0 page callback"),
                vec![Value::Number(0.0), Value::Number(0.0), Value::Bool(false)],
            )
            .expect("select track 0 second page");
        assert_eq!(
            editor
                .runtime_mut()
                .eval_str("(seqv-current-page 0 0)")
                .unwrap(),
            Some(Value::Number(1.0))
        );
        assert_eq!(
            editor
                .runtime_mut()
                .eval_str("(seqv-current-page 1 1)")
                .unwrap(),
            Some(Value::Number(0.0))
        );
    }

    #[test]
    fn metal_seq_sequencer_expanded_cursor_highlight_uses_bound_track_selection_and_cursor() {
        let mut editor = full_grid_editor_for_scroll_tests();
        set_full_grid_track_count(&mut editor, 2, 16);
        set_current_track_reactive(editor.runtime_mut(), 2, 0);
        editor.runtime_mut().run_reactive_cycle();
        editor.refresh_runtime_side_effects();
        let sequencer_id = editor
            .buffers
            .iter()
            .find(|buffer| buffer.name == "*sequencer*")
            .expect("sequencer buffer should exist")
            .id;
        editor.set_active_buffer(sequencer_id);
        editor.set_layout_viewport(220, 200);
        editor
            .runtime_mut()
            .eval_str(
                "(do
                  (seqv-track-menu-click 0)
                  (seqv-track-menu-click 1)
                  (seqv-set-cursor-step 0 6)
                  (seqv-set-cursor-step 1 3)
                  (set! cursor-step 3))",
            )
            .expect("expand rows and seed cursors");
        set_test_expanded_step_slot_projection(&mut editor, 0, 0, 0, 0, 16, 6, 0);
        set_test_expanded_step_slot_projection(&mut editor, 1, 1, 0, 0, 16, 3, 0);
        editor.refresh_runtime_side_effects();
        let _ = editor.runtime_mut().take_pending_buffer_widget_trees();
        let _ = editor.take_dirty_widget_ids();
        let before_revision = editor.widget_layout_revision();

        set_current_track_reactive(editor.runtime_mut(), 2, 1);
        editor.runtime_mut().run_reactive_cycle();
        let pending = editor.runtime_mut().take_pending_buffer_widget_trees();
        let sequencer_updates = pending
            .iter()
            .filter(|pending| {
                matches!(
                    pending.target(),
                    eseqlisp::vm::EffectTarget::BufferName(name) if name == "*sequencer*"
                )
            })
            .collect::<Vec<_>>();
        assert!(
            sequencer_updates.is_empty(),
            "track selection should update expanded cursor highlight bindings without rerendering sequencer rows; got {} sequencer updates out of {} pending updates",
            sequencer_updates.len(),
            pending.len()
        );
        assert_eq!(
            editor.widget_layout_revision(),
            before_revision,
            "track selection should not require a sequencer layout revision"
        );

        let layout = editor
            .widget_layout()
            .expect("expanded two-track sequencer layout should build");
        let current_track_cursor =
            find_layout_node_by_stable_key(&layout, "seqv-expanded-step-column-1-3")
                .expect("current track cursor column should render");
        assert_eq!(
            current_track_cursor.props.get("background"),
            Some(&Value::String("cursor-highlight".to_string())),
            "current expanded row should keep the cursor-highlight background widget"
        );
        assert_eq!(
            layout_prop_bool(current_track_cursor, "active"),
            Some(true),
            "current expanded row should activate its bound cursor step"
        );
        assert_eq!(
            layout_prop_bool(current_track_cursor, "selected"),
            Some(true),
            "current expanded row should become visible through the bound selected state"
        );

        let inactive_track_cursor =
            find_layout_node_by_stable_key(&layout, "seqv-expanded-step-column-0-6")
                .expect("inactive track cursor column should render");
        assert_eq!(
            layout_prop_bool(inactive_track_cursor, "active"),
            Some(true),
            "inactive expanded row should retain its independent cursor step"
        );
        assert_eq!(
            layout_prop_bool(inactive_track_cursor, "selected"),
            Some(false),
            "inactive expanded row should hide the cursor through bound track selection"
        );

        editor
            .runtime_mut()
            .eval_str("(cursor-right)")
            .expect("move sequencer cursor right");
        assert_eq!(
            editor
                .runtime_mut()
                .eval_str("(seqv-current-step 1 1)")
                .unwrap(),
            Some(Value::Number(4.0)),
            "arrow movement should move the expanded editor cursor for the current track"
        );
    }

    #[test]
    fn metal_seq_sequencer_keyboard_shortcuts_target_current_track() {
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

        let mut editor = full_grid_editor_for_scroll_tests();
        set_full_grid_track_count(&mut editor, 2, 16);
        let sequencer_id = editor
            .buffers
            .iter()
            .find(|buffer| buffer.name == "*sequencer*")
            .expect("sequencer buffer should exist")
            .id;
        editor.set_active_buffer(sequencer_id);

        editor
            .runtime_mut()
            .set_reactive("SEQ", "current-track", Value::Number(1.0));
        editor
            .runtime_mut()
            .eval_str(
                r#"
                (do
                  (seqv-set-param-mode 0 4)
                  (seqv-set-param-mode 1 0)
                  (set! selected-bus 1)
                  (seqv-set-track-expanded 0 true)
                  (seqv-set-track-expanded 1 true))
                "#,
            )
            .expect("seed sequencer shortcut state");
        editor.runtime_mut().run_reactive_cycle();
        editor.refresh_runtime_side_effects();

        editor.handle_key(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE));
        assert_eq!(
            editor
                .runtime_mut()
                .eval_str("(seqv-param-mode 0)")
                .unwrap(),
            Some(Value::Number(4.0)),
            "parameter shortcuts should not mutate inactive expanded rows"
        );
        assert_eq!(
            editor
                .runtime_mut()
                .eval_str("(seqv-param-mode 1)")
                .unwrap(),
            Some(Value::Number(1.0)),
            "duration shortcut should select duration for the current track"
        );

        assert_eq!(
            editor.runtime_mut().eval_str("selected-bus").unwrap(),
            Some(Value::Number(1.0)),
            "plain parameter shortcuts should not disturb bus selection"
        );
    }

    #[test]
    fn metal_seq_sequencer_expanded_row_controls_activate_target_track_before_mutating() {
        let mut editor = full_grid_editor_for_scroll_tests();
        set_full_grid_track_count(&mut editor, 2, 16);
        editor.runtime_mut().run_reactive_cycle();
        editor.refresh_runtime_side_effects();
        let sequencer_id = editor
            .buffers
            .iter()
            .find(|buffer| buffer.name == "*sequencer*")
            .expect("sequencer buffer should exist")
            .id;
        editor.set_active_buffer(sequencer_id);
        editor.set_layout_viewport(220, 200);

        let calls = Arc::new(Mutex::new(Vec::<String>::new()));
        {
            let calls = Arc::clone(&calls);
            editor
                .runtime_mut()
                .register_native("seq-clear-selection", move |_args, _ctx| {
                    calls.lock().unwrap().push("clear".to_string());
                    Ok(Value::Bool(true))
                });
        }
        {
            let calls = Arc::clone(&calls);
            editor
                .runtime_mut()
                .register_native("seq-set-track", move |args, _ctx| {
                    let track = match args.first() {
                        Some(Value::Number(track)) => *track as usize,
                        _ => usize::MAX,
                    };
                    calls.lock().unwrap().push(format!("track:{track}"));
                    Ok(Value::Bool(true))
                });
        }
        {
            let calls = Arc::clone(&calls);
            editor
                .runtime_mut()
                .register_native("seq-set-step-param", move |args, _ctx| {
                    let step = match args.first() {
                        Some(Value::Number(step)) => *step as usize,
                        _ => usize::MAX,
                    };
                    let param = match args.get(1) {
                        Some(Value::Keyword(param)) => param.clone(),
                        other => format!("{other:?}"),
                    };
                    calls.lock().unwrap().push(format!("param:{step}:{param}"));
                    Ok(Value::Bool(true))
                });
        }
        {
            let calls = Arc::clone(&calls);
            editor
                .runtime_mut()
                .register_native("seq-double-track-pattern", move |_args, _ctx| {
                    calls.lock().unwrap().push("double".to_string());
                    Ok(Value::Bool(true))
                });
        }
        {
            let calls = Arc::clone(&calls);
            editor
                .runtime_mut()
                .register_native("seq-halve-track-pattern", move |_args, _ctx| {
                    calls.lock().unwrap().push("halve".to_string());
                    Ok(Value::Bool(true))
                });
        }

        editor
            .runtime_mut()
            .eval_str("(seqv-track-menu-click 1)")
            .expect("expand second sequencer row");
        editor.refresh_runtime_side_effects();
        calls.lock().unwrap().clear();

        let layout = editor
            .widget_layout()
            .expect("expanded second-row sequencer layout should build");
        let slider = find_layout_node_by_stable_key(&layout, "seqv-expanded-step-slider-1-0")
            .expect("second row first step slider");
        editor
            .runtime_mut()
            .invoke(
                slider
                    .props
                    .get("on-change")
                    .cloned()
                    .expect("expanded row slider on-change"),
                vec![Value::Number(0.42)],
            )
            .expect("change inactive expanded row slider");
        assert_eq!(
            calls.lock().unwrap().as_slice(),
            ["clear", "track:1", "param:0:velocity"],
            "slider edits on an inactive expanded row should activate that row before mutating"
        );

        calls.lock().unwrap().clear();
        let double_button = find_layout_node_by_stable_key(&layout, "seqv-expanded-double-1")
            .expect("second row double button");
        editor
            .runtime_mut()
            .invoke(
                double_button
                    .props
                    .get("on-click")
                    .cloned()
                    .expect("expanded row double on-click"),
                vec![Value::Number(0.0), Value::Number(0.0), Value::Bool(false)],
            )
            .expect("click expanded row double");
        assert_eq!(
            calls.lock().unwrap().as_slice(),
            ["clear", "track:1", "double"],
            "double pattern should target the expanded row's track"
        );

        calls.lock().unwrap().clear();
        let half_button = find_layout_node_by_stable_key(&layout, "seqv-expanded-half-1")
            .expect("second row half button");
        editor
            .runtime_mut()
            .invoke(
                half_button
                    .props
                    .get("on-click")
                    .cloned()
                    .expect("expanded row half on-click"),
                vec![Value::Number(0.0), Value::Number(0.0), Value::Bool(false)],
            )
            .expect("click expanded row half");
        assert_eq!(
            calls.lock().unwrap().as_slice(),
            ["clear", "track:1", "halve"],
            "halve pattern should target the expanded row's track"
        );
    }

    #[test]
    fn metal_seq_sequencer_header_controls_activate_target_track_before_mutating() {
        let mut editor = full_grid_editor_for_scroll_tests();
        set_full_grid_track_count(&mut editor, 2, 16);
        editor.runtime_mut().run_reactive_cycle();
        editor.refresh_runtime_side_effects();
        let sequencer_id = editor
            .buffers
            .iter()
            .find(|buffer| buffer.name == "*sequencer*")
            .expect("sequencer buffer should exist")
            .id;
        editor.set_active_buffer(sequencer_id);
        editor.set_layout_viewport(220, 40);

        let calls = Arc::new(Mutex::new(Vec::<String>::new()));
        for name in [
            "seq-clear-selection",
            "seq-set-track",
            "seq-toggle-record-arm",
        ] {
            let calls = Arc::clone(&calls);
            editor
                .runtime_mut()
                .register_native(name, move |args, _ctx| {
                    calls.lock().unwrap().push(format!("{name}:{args:?}"));
                    Ok(Value::Bool(true))
                });
        }

        let layout = editor
            .widget_layout()
            .expect("sequencer layout should build");
        let arm = find_layout_node_by_stable_key(&layout, "seqv-arm-1")
            .expect("second sequencer row record-arm control");
        editor
            .runtime_mut()
            .invoke(
                arm.props
                    .get("on-click")
                    .cloned()
                    .expect("record-arm on-click"),
                vec![Value::Number(0.0), Value::Number(0.0), Value::Bool(false)],
            )
            .expect("invoke second-row record-arm");

        assert_eq!(
            calls.lock().unwrap().as_slice(),
            [
                "seq-clear-selection:[]",
                "seq-set-track:[1]",
                "seq-toggle-record-arm:[1]",
            ],
            "sequencer header controls should select their row before mutating it"
        );
    }

    #[test]
    fn metal_seq_sequencer_expanded_current_row_slider_uses_plock_path_for_selected_step() {
        let mut editor = full_grid_editor_for_scroll_tests();
        let sequencer_id = editor
            .buffers
            .iter()
            .find(|buffer| buffer.name == "*sequencer*")
            .expect("sequencer buffer should exist")
            .id;
        editor.set_active_buffer(sequencer_id);
        editor.set_layout_viewport(180, 30);
        editor.runtime_mut().set_reactive(
            "SEQ",
            "selected-steps",
            test_bool_list(&[
                true, false, false, false, false, false, false, false, false, false, false, false,
                false, false, false, false,
            ]),
        );

        let calls = Arc::new(Mutex::new(Vec::<String>::new()));
        editor
            .runtime_mut()
            .register_native("seq-has-selection?", |_args, _ctx| Ok(Value::Bool(true)));
        {
            let calls = Arc::clone(&calls);
            editor
                .runtime_mut()
                .register_native("seq-set-step-param-plock", move |args, _ctx| {
                    let param = match args.first() {
                        Some(Value::Keyword(param)) => param.clone(),
                        other => format!("{other:?}"),
                    };
                    calls.lock().unwrap().push(format!("plock:{param}"));
                    Ok(Value::Bool(true))
                });
        }

        editor
            .runtime_mut()
            .eval_str("(seqv-track-menu-click 0)")
            .expect("expand current sequencer row");
        editor.refresh_runtime_side_effects();
        let layout = editor
            .widget_layout()
            .expect("expanded current-row sequencer layout should build");
        let slider = find_layout_node_by_stable_key(&layout, "seqv-expanded-step-slider-0-0")
            .expect("current row selected step slider");
        editor
            .runtime_mut()
            .invoke(
                slider
                    .props
                    .get("on-change")
                    .cloned()
                    .expect("expanded current row slider on-change"),
                vec![Value::Number(0.77)],
            )
            .expect("change selected current row slider");
        assert_eq!(
            calls.lock().unwrap().as_slice(),
            ["plock:velocity"],
            "selected current-row slider edits should use the p-lock path"
        );
    }

    #[test]
    fn metal_seq_sequencer_expanded_playhead_uses_per_track_active_fields() {
        let mut editor = full_grid_editor_for_scroll_tests();
        let sequencer_id = editor
            .buffers
            .iter()
            .find(|buffer| buffer.name == "*sequencer*")
            .expect("sequencer buffer should exist")
            .id;
        editor.set_active_buffer(sequencer_id);
        editor.set_layout_viewport(180, 30);
        editor
            .runtime_mut()
            .set_reactive("SEQ", "playing", Value::Bool(true));
        editor
            .runtime_mut()
            .eval_str("(seqv-track-menu-click 0)")
            .expect("expand current sequencer row");
        editor.refresh_runtime_side_effects();

        let initial_layout = editor
            .widget_layout()
            .expect("expanded current-row sequencer layout should build");
        let step_0 = find_layout_node_by_stable_key(
            &initial_layout,
            "seqv-expanded-step-playhead-probe-0-0",
        )
        .expect("expanded step 0 playhead probe");
        let step_1 = find_layout_node_by_stable_key(
            &initial_layout,
            "seqv-expanded-step-playhead-probe-0-1",
        )
        .expect("expanded step 1 playhead probe");
        assert!(layout_tree_has_bool_prop(step_0, "active", true));
        assert!(layout_tree_has_bool_prop(step_1, "active", false));

        editor.runtime_mut().set_reactive(
            "SEQ",
            &expanded_step_slot_playhead_field(0, 0),
            Value::Bool(false),
        );
        editor.runtime_mut().set_reactive(
            "SEQ",
            &expanded_step_slot_playhead_field(0, 1),
            Value::Bool(true),
        );
        editor.refresh_runtime_side_effects();
        let moved_layout = editor
            .widget_layout()
            .expect("expanded current-row sequencer layout should rebuild after playhead move");
        let moved_step_0 =
            find_layout_node_by_stable_key(&moved_layout, "seqv-expanded-step-playhead-probe-0-0")
                .expect("expanded step 0 playhead probe after move");
        let moved_step_1 =
            find_layout_node_by_stable_key(&moved_layout, "seqv-expanded-step-playhead-probe-0-1")
                .expect("expanded step 1 playhead probe after move");
        assert!(layout_tree_has_bool_prop(moved_step_0, "active", false));
        assert!(layout_tree_has_bool_prop(moved_step_1, "active", true));
    }

    #[test]
    fn metal_seq_sequencer_expanded_playhead_tick_does_not_rebuild_rows() {
        let mut editor = full_grid_editor_for_scroll_tests();
        set_full_grid_track_count(&mut editor, 2, 32);
        editor.runtime_mut().run_reactive_cycle();
        editor.refresh_runtime_side_effects();
        let sequencer_id = editor
            .buffers
            .iter()
            .find(|buffer| buffer.name == "*sequencer*")
            .expect("sequencer buffer should exist")
            .id;
        editor.set_active_buffer(sequencer_id);
        editor.set_layout_viewport(220, 200);
        editor
            .runtime_mut()
            .set_reactive("SEQ", "playing", Value::Bool(true));
        editor.runtime_mut().run_reactive_cycle();
        let _ = editor.runtime_mut().take_pending_buffer_widget_trees();
        editor
            .runtime_mut()
            .eval_str("(do (seqv-track-menu-click 0) (seqv-track-menu-click 1))")
            .expect("expand two sequencer rows");
        editor.refresh_runtime_side_effects();
        editor
            .widget_layout()
            .expect("expanded two-row sequencer layout should build");

        let _ = editor.runtime_mut().take_pending_buffer_widget_trees();
        for track in 0..2 {
            editor.runtime_mut().set_reactive(
                "SEQ",
                &expanded_step_slot_playhead_field(track, 0),
                Value::Bool(false),
            );
            editor.runtime_mut().set_reactive(
                "SEQ",
                &expanded_step_slot_playhead_field(track, 1),
                Value::Bool(true),
            );
        }
        editor.runtime_mut().run_reactive_cycle();
        let pending = editor.runtime_mut().take_pending_buffer_widget_trees();
        let pending_summary = pending
            .iter()
            .map(|pending| match pending {
                eseqlisp::vm::PendingUiUpdate::FullTree(tree) => {
                    let tree_debug = format!("{:?}", tree.tree);
                    let preview = tree_debug.chars().take(160).collect::<String>();
                    format!(
                        "full target={:?} expanded-row={} collapsed-row={} playhead-probe={} tree={preview}",
                        tree.target,
                        value_contains_string(&tree.tree, "seqv-expanded-step-slider-"),
                        value_contains_string(&tree.tree, "seqv-playhead-row-"),
                        value_contains_string(&tree.tree, "seqv-expanded-step-playhead-probe-")
                    )
                }
                eseqlisp::vm::PendingUiUpdate::ReplaceSubtree {
                    target,
                    subtree_root_id,
                    tree,
                    ..
                } => {
                    let tree_debug = format!("{tree:?}");
                    let preview = tree_debug.chars().take(160).collect::<String>();
                    format!(
                        "subtree#{subtree_root_id} target={target:?} expanded-row={} collapsed-row={} playhead-probe={} tree={preview}",
                        value_contains_string(tree, "seqv-expanded-step-slider-"),
                        value_contains_string(tree, "seqv-playhead-row-"),
                        value_contains_string(tree, "seqv-expanded-step-playhead-probe-")
                    )
                }
            })
            .collect::<Vec<_>>();
        let sequencer_updates = pending
            .iter()
            .filter(|pending| {
                matches!(
                    pending.target(),
                    eseqlisp::vm::EffectTarget::BufferName(name) if name == "*sequencer*"
                )
            })
            .collect::<Vec<_>>();
        assert!(
            sequencer_updates.is_empty(),
            "per-step playhead ticks should update bound props without rebuilding sequencer rows; got {} sequencer updates out of {} pending updates: {pending_summary:?}",
            sequencer_updates.len(),
            pending.len()
        );
    }

    #[test]
    fn metal_seq_sequencer_expanded_selection_recolors_after_empty_selection() {
        let mut editor = full_grid_editor_for_scroll_tests();
        let sequencer_id = editor
            .buffers
            .iter()
            .find(|buffer| buffer.name == "*sequencer*")
            .expect("sequencer buffer should exist")
            .id;
        editor.set_active_buffer(sequencer_id);
        editor.set_layout_viewport(180, 30);
        editor
            .runtime_mut()
            .eval_str("(seqv-track-menu-click 0)")
            .expect("expand current sequencer row");
        editor.refresh_runtime_side_effects();

        let initial_layout = editor
            .widget_layout()
            .expect("expanded current-row sequencer layout should build");
        let slot_0_label =
            find_layout_node_by_stable_key(&initial_layout, "seqv-expanded-step-label-0-0")
                .expect("initial slot 0 label");
        assert_eq!(
            slot_0_label.props.get("color"),
            Some(&Value::Keyword("dim".to_string())),
            "empty selection should render the step label as dim"
        );
        let _ = editor.take_dirty_widget_ids();
        let before_revision = editor.widget_layout_revision();

        editor.runtime_mut().set_reactive(
            "SEQ",
            &expanded_step_slot_selected_field(0, 0),
            Value::Bool(true),
        );
        editor.runtime_mut().run_reactive_cycle();

        let pending = editor.runtime_mut().take_pending_buffer_widget_trees();
        assert!(
            pending.is_empty(),
            "bound selection recolor should not enqueue widget tree updates; got {}",
            pending.len()
        );
        assert_eq!(
            editor.widget_layout_revision(),
            before_revision,
            "selection recolor should not require a layout revision"
        );

        let selected_layout = editor
            .widget_layout()
            .expect("expanded current-row sequencer layout should still exist");
        let slot_0_label =
            find_layout_node_by_stable_key(&selected_layout, "seqv-expanded-step-label-0-0")
                .expect("selected slot 0 label");
        assert_eq!(
            layout_prop_bool(slot_0_label, "active"),
            Some(true),
            "selection changes from an initially empty selection should activate the bound step label"
        );
        assert_eq!(
            slot_0_label.props.get("active-color"),
            Some(&Value::Keyword("yellow".to_string())),
            "active selected labels should render with the selected color"
        );
        let slot_0_toggle =
            find_layout_node_by_stable_key(&selected_layout, "seqv-expanded-step-toggle-0-0")
                .expect("selected slot 0 toggle");
        assert!(
            layout_tree_has_bool_prop(slot_0_toggle, "selected", true),
            "selection changes should also update the expanded toggle selected prop"
        );
    }

    #[test]
    fn metal_seq_sequencer_expanded_step_hot_state_uses_widget_bindings() {
        let mut editor = full_grid_editor_for_scroll_tests();
        let sequencer_id = editor
            .buffers
            .iter()
            .find(|buffer| buffer.name == "*sequencer*")
            .expect("sequencer buffer should exist")
            .id;
        editor.set_active_buffer(sequencer_id);
        editor.set_layout_viewport(180, 30);
        editor
            .runtime_mut()
            .eval_str("(seqv-track-menu-click 0)")
            .expect("expand current sequencer row");
        editor.refresh_runtime_side_effects();

        let layout = editor
            .widget_layout()
            .expect("expanded current-row sequencer layout should build");
        let row_id = find_layout_node_by_stable_key(&layout, "sequencer-track-0")
            .expect("expanded row")
            .widget_id;
        let slider_id = find_layout_node_by_stable_key(&layout, "seqv-expanded-step-slider-0-0")
            .expect("expanded step slider")
            .widget_id;
        let toggle_id = find_layout_node_by_stable_key(&layout, "seqv-expanded-step-toggle-0-0")
            .expect("expanded step toggle")
            .widget_id;
        let label_id = find_layout_node_by_stable_key(&layout, "seqv-expanded-step-label-0-0")
            .expect("expanded step label")
            .widget_id;
        let _ = editor.take_dirty_widget_ids();
        let _ = editor.runtime_mut().take_pending_buffer_widget_trees();

        editor.runtime_mut().set_reactive(
            "SEQ",
            &expanded_step_slot_selected_field(0, 0),
            Value::Bool(true),
        );
        let selected_dirty = editor.take_dirty_widget_ids();
        assert!(
            selected_dirty.contains(&toggle_id) && selected_dirty.contains(&label_id),
            "selected field should dirty only bound step widgets, got {selected_dirty:?}"
        );
        assert!(
            !selected_dirty.contains(&row_id),
            "selected binding must not dirty the expanded row container"
        );
        assert!(
            editor
                .runtime_mut()
                .take_pending_buffer_widget_trees()
                .is_empty(),
            "selected binding must not enqueue expanded row subtree updates"
        );

        editor.runtime_mut().set_reactive(
            "SEQ",
            &expanded_step_slot_param_slider_field(0, 0, 0),
            Value::Number(0.37),
        );
        let slider_dirty = editor.take_dirty_widget_ids();
        assert!(
            slider_dirty.contains(&slider_id),
            "param slider field should dirty the bound vslider, got {slider_dirty:?}"
        );
        assert!(
            !slider_dirty.contains(&row_id),
            "param slider binding must not dirty the expanded row container"
        );
        assert!(
            editor
                .runtime_mut()
                .take_pending_buffer_widget_trees()
                .is_empty(),
            "param slider binding must not enqueue expanded row subtree updates"
        );

        editor.runtime_mut().set_reactive(
            "SEQ",
            &expanded_step_slot_active_field(0, 0),
            Value::Bool(false),
        );
        let active_dirty = editor.take_dirty_widget_ids();
        assert!(
            active_dirty.contains(&slider_id) && active_dirty.contains(&toggle_id),
            "active field should dirty bound slider/toggle widgets, got {active_dirty:?}"
        );
        assert!(
            !active_dirty.contains(&row_id),
            "active binding must not dirty the expanded row container"
        );
        assert!(
            editor
                .runtime_mut()
                .take_pending_buffer_widget_trees()
                .is_empty(),
            "active binding must not enqueue expanded row subtree updates"
        );
    }

    #[test]
    fn metal_seq_sequencer_expanded_selection_drag_does_not_rerender_rows() {
        let mut editor = full_grid_editor_for_scroll_tests();
        editor
            .runtime_mut()
            .register_native("seq-select-step-range", |_args, _ctx| Ok(Value::Nil));
        set_full_grid_track_count(&mut editor, 2, 32);
        editor.runtime_mut().run_reactive_cycle();
        editor.refresh_runtime_side_effects();
        let sequencer_id = editor
            .buffers
            .iter()
            .find(|buffer| buffer.name == "*sequencer*")
            .expect("sequencer buffer should exist")
            .id;
        editor.set_active_buffer(sequencer_id);
        editor.set_layout_viewport(220, 200);
        editor
            .runtime_mut()
            .eval_str("(do (seqv-track-menu-click 0) (seqv-track-menu-click 1))")
            .expect("expand two sequencer rows");
        editor.refresh_runtime_side_effects();
        let layout = editor
            .widget_layout()
            .expect("expanded two-row sequencer layout should build");
        let down = find_layout_node_by_stable_key(&layout, "seqv-expanded-step-toggle-0-0")
            .expect("drag start toggle")
            .props
            .get("on-mouse-down")
            .cloned()
            .expect("drag start callback");
        let drag = find_layout_node_by_stable_key(&layout, "seqv-expanded-step-toggle-0-4")
            .expect("drag target toggle")
            .props
            .get("on-drag")
            .cloned()
            .expect("drag target callback");
        let selection_evt = map_value([("shift", Value::Bool(true))]);

        editor
            .runtime_mut()
            .invoke(down, vec![selection_evt.clone()])
            .expect("start expanded selection drag");
        let _ = editor.runtime_mut().take_pending_buffer_widget_trees();
        let _ = editor.take_dirty_widget_ids();

        editor
            .runtime_mut()
            .invoke(drag, vec![selection_evt])
            .expect("continue expanded selection drag");
        let pending = editor.runtime_mut().take_pending_buffer_widget_trees();
        let sequencer_updates = pending
            .iter()
            .filter(|pending| {
                matches!(
                    pending.target(),
                    eseqlisp::vm::EffectTarget::BufferName(name) if name == "*sequencer*"
                )
            })
            .collect::<Vec<_>>();
        assert!(
            sequencer_updates.is_empty(),
            "expanded selection drag should not rerender sequencer rows; got {} sequencer updates out of {} pending updates",
            sequencer_updates.len(),
            pending.len()
        );
    }

    #[test]
    fn metal_seq_sequencer_expanded_page_boundary_updates_stable_slots_without_rerendering_rows() {
        let mut editor = full_grid_editor_for_scroll_tests();
        set_full_grid_track_count(&mut editor, 2, 32);
        editor.runtime_mut().run_reactive_cycle();
        editor.refresh_runtime_side_effects();
        let sequencer_id = editor
            .buffers
            .iter()
            .find(|buffer| buffer.name == "*sequencer*")
            .expect("sequencer buffer should exist")
            .id;
        editor.set_active_buffer(sequencer_id);
        editor.set_layout_viewport(220, 200);
        editor
            .runtime_mut()
            .set_reactive("SEQ", "playing", Value::Bool(true));
        editor
            .runtime_mut()
            .set_reactive("SEQ", "auto-follow", Value::Bool(true));
        editor.runtime_mut().run_reactive_cycle();
        editor
            .runtime_mut()
            .eval_str("(do (seqv-track-menu-click 0) (seqv-track-menu-click 1))")
            .expect("expand two sequencer rows");
        editor.refresh_runtime_side_effects();
        let initial_layout = editor
            .widget_layout()
            .expect("expanded two-row sequencer layout should build");
        assert_eq!(
            find_layout_node_by_stable_key(&initial_layout, "seqv-expanded-step-label-0-0")
                .and_then(|node| layout_prop_number(node, "value")),
            Some(1.0),
            "slot 0 should initially show absolute step 1"
        );
        assert_eq!(
            find_layout_node_by_stable_key(&initial_layout, "seqv-expanded-step-label-0-0")
                .and_then(|node| node.props.get("h-align")),
            Some(&Value::Keyword("center".to_string())),
            "fixed-width step labels should center their text under the toggle"
        );
        let _ = editor.take_dirty_widget_ids();
        let before_revision = editor.widget_layout_revision();

        for track in 0..2 {
            set_test_expanded_step_slot_projection(&mut editor, track, track, 1, 0, 32, 16, 16);
        }
        editor.runtime_mut().run_reactive_cycle();

        let pending = editor.runtime_mut().take_pending_buffer_widget_trees();
        let sequencer_updates = pending
            .iter()
            .filter(|pending| {
                matches!(
                    pending.target(),
                    eseqlisp::vm::EffectTarget::BufferName(name) if name == "*sequencer*"
                )
            })
            .count();
        assert_eq!(
            sequencer_updates,
            0,
            "page-boundary projection should update bound slot props without rebuilding sequencer rows; got {} pending updates",
            pending.len()
        );
        assert_eq!(
            editor.widget_layout_revision(),
            before_revision,
            "page-boundary slot projection should not bump the active layout revision"
        );

        let paged_layout = editor
            .widget_layout()
            .expect("paged expanded sequencer layout should still exist");
        let slot_0_label =
            find_layout_node_by_stable_key(&paged_layout, "seqv-expanded-step-label-0-0")
                .expect("track 0 slot 0 label after page flip");
        assert_finite_nonzero_rect(slot_0_label, "track 0 slot 0 label after page flip");
        assert_eq!(
            layout_prop_number(slot_0_label, "value"),
            Some(17.0),
            "stable slot 0 should now display absolute step 17"
        );
        let slot_0_playhead =
            find_layout_node_by_stable_key(&paged_layout, "seqv-expanded-step-playhead-probe-0-0")
                .expect("track 0 slot 0 playhead after page flip");
        assert!(
            layout_tree_has_reactive_prop_field(
                slot_0_playhead,
                "active",
                "SEQ",
                &expanded_step_slot_playhead_field(0, 0),
            ),
            "slot 0 playhead should keep the stable slot playhead binding"
        );
        assert_eq!(
            editor
                .runtime_mut()
                .eval_str(r#"(reactive-get "SEQ" "seqv-slot-playhead-active-0-0")"#)
                .unwrap(),
            Some(Value::Bool(true)),
            "stable slot 0 playhead binding should reflect absolute step 16"
        );
    }

    #[test]
    fn metal_seq_sequencer_expanded_timebase_uses_default_or_selected_step_plock_path() {
        let mut editor = full_grid_editor_for_scroll_tests();
        let sequencer_id = editor
            .buffers
            .iter()
            .find(|buffer| buffer.name == "*sequencer*")
            .expect("sequencer buffer should exist")
            .id;
        editor.set_active_buffer(sequencer_id);
        editor.set_layout_viewport(140, 20);

        let calls = Arc::new(Mutex::new(Vec::<String>::new()));
        let has_selection = Arc::new(std::sync::atomic::AtomicBool::new(false));

        {
            let calls = Arc::clone(&calls);
            editor
                .runtime_mut()
                .register_native("seq-set-track", move |args, _ctx| {
                    let track = match args.first() {
                        Some(Value::Number(track)) => track.to_string(),
                        other => format!("{other:?}"),
                    };
                    calls.lock().unwrap().push(format!("track:{track}"));
                    Ok(Value::Bool(true))
                });
        }
        editor
            .runtime_mut()
            .register_native("seq-pause-auto-follow", |_args, _ctx| Ok(Value::Bool(true)));
        {
            let has_selection = Arc::clone(&has_selection);
            editor
                .runtime_mut()
                .register_native("seq-has-selection?", move |_args, _ctx| {
                    Ok(Value::Bool(has_selection.load(Ordering::Relaxed)))
                });
        }
        {
            let calls = Arc::clone(&calls);
            editor
                .runtime_mut()
                .register_native("seq-set-timebase", move |args, _ctx| {
                    let label = match args.first() {
                        Some(Value::String(label)) => label.clone(),
                        other => format!("{other:?}"),
                    };
                    calls.lock().unwrap().push(format!("default:{label}"));
                    Ok(Value::Bool(true))
                });
        }
        {
            let calls = Arc::clone(&calls);
            editor
                .runtime_mut()
                .register_native("seq-plock-timebase", move |args, _ctx| {
                    let label = match args.first() {
                        Some(Value::String(label)) => label.clone(),
                        other => format!("{other:?}"),
                    };
                    calls.lock().unwrap().push(format!("plock:{label}"));
                    Ok(Value::Bool(true))
                });
        }

        editor
            .runtime_mut()
            .eval_str("(seqv-track-menu-click 0)")
            .expect("expand sequencer row");
        editor.refresh_runtime_side_effects();
        calls.lock().unwrap().clear();
        let layout = editor
            .widget_layout()
            .expect("expanded sequencer layout should build");
        assert!(
            find_layout_node_by_stable_key(&layout, "seqv-timebase-0").is_none(),
            "collapsed-row timebase dropdown should stay removed"
        );
        let timebase = find_layout_node_by_stable_key(&layout, "seqv-expanded-timebase-0")
            .expect("expanded sequencer row timebase should render");
        let callback = timebase
            .props
            .get("on-change")
            .cloned()
            .expect("expanded sequencer row timebase on-change");

        editor
            .runtime_mut()
            .invoke(callback.clone(), vec![Value::String("8".to_string())])
            .expect("invoke expanded sequencer row default timebase");
        assert_eq!(
            calls.lock().unwrap().as_slice(),
            ["track:0", "default:8"],
            "without selected steps the expanded row dropdown should update the track default"
        );

        calls.lock().unwrap().clear();
        has_selection.store(true, Ordering::Relaxed);
        editor
            .runtime_mut()
            .invoke(callback, vec![Value::String("4T".to_string())])
            .expect("invoke expanded sequencer row selected-step timebase");
        assert_eq!(
            calls.lock().unwrap().as_slice(),
            ["track:0", "plock:4T"],
            "with selected steps on the current expanded row the dropdown should p-lock timebase"
        );
    }

    #[test]
    fn metal_seq_sequencer_pattern_switch_updates_existing_step_bindings() {
        let track_count = 10;
        let mut editor = sequencer_perf_editor(track_count, 32);

        let initial_layout = editor
            .widget_layout()
            .expect("initial sequencer layout should build");
        let initial_step = find_layout_node_by_stable_key(&initial_layout, "seqv-step-cell-0-0")
            .expect("initial step cell should render");
        assert_eq!(
            layout_prop_bool(initial_step, "active"),
            Some(true),
            "fixture generation 0 should render track 0 step 0 active"
        );

        apply_sequencer_perf_pattern(&mut editor, track_count, 48, 1);
        editor.runtime_mut().run_reactive_cycle();
        editor.refresh_runtime_side_effects();
        if let Some(status) = editor.runtime_mut().take_status_message() {
            panic!("sequencer pattern switch binding status: {status}");
        }

        let switched_layout = editor
            .widget_layout()
            .expect("switched sequencer layout should build");
        assert_eq!(
            count_stable_key_prefix(&switched_layout, "seqv-step-cell-"),
            track_count * 48,
            "pattern switch should render the new pattern length"
        );
        let switched_step = find_layout_node_by_stable_key(&switched_layout, "seqv-step-cell-0-0")
            .expect("existing keyed step cell should still render after switch");
        assert_eq!(
            layout_prop_bool(switched_step, "active"),
            Some(false),
            "track 0 step 0 active binding should reflect the switched pattern"
        );
    }

    #[test]
    fn metal_seq_mixer_v2_pattern_cell_state_updates_existing_bindings() {
        let track_count = 10;
        let cell_count = 8;
        let mut editor = mixer_v2_perf_editor(track_count, cell_count);

        let initial_layout = editor
            .widget_layout()
            .expect("initial mixer layout should build");
        let mut pattern_widget_ids = std::collections::HashSet::new();
        for track in 0..track_count {
            for cell in 0..cell_count {
                let pattern_id = track * 100 + cell + 1;
                let key = format!("mixer-v2-track-pattern-cell-{track}-{pattern_id}");
                pattern_widget_ids.insert(
                    find_layout_node_by_stable_key(&initial_layout, &key)
                        .unwrap_or_else(|| panic!("missing mixer pattern cell {key}"))
                        .widget_id,
                );
            }
        }
        let initial_cell =
            find_layout_node_by_stable_key(&initial_layout, "mixer-v2-track-pattern-cell-0-1")
                .expect("initial active pattern cell");
        assert_eq!(
            layout_prop_bool(initial_cell, "active"),
            Some(true),
            "fixture generation 0 should render track 0 pattern 1 active"
        );

        let _ = editor.take_dirty_widget_ids();
        apply_mixer_v2_perf_pattern(&mut editor, track_count, cell_count, 1);
        let dirty_widgets = editor.take_dirty_widget_ids();
        assert!(
            !dirty_widgets.is_empty(),
            "pattern-cell binding updates should dirty concrete widgets"
        );
        assert!(
            dirty_widgets
                .iter()
                .all(|widget_id| pattern_widget_ids.contains(widget_id)),
            "pattern-cell binding updates should dirty only pattern-cell widgets: {dirty_widgets:?}"
        );

        editor.runtime_mut().run_reactive_cycle();
        editor.refresh_runtime_side_effects();
        if let Some(trace) = editor.runtime().last_ui_invalidation_trace() {
            assert!(
                !trace
                    .affected_buffers
                    .iter()
                    .any(|buffer| buffer == "*mixer*"),
                "pattern-cell binding updates should not rerun the mixer tree: {trace:?}"
            );
        }

        let switched_layout = editor
            .widget_layout()
            .expect("switched mixer layout should still build");
        let switched_old_cell =
            find_layout_node_by_stable_key(&switched_layout, "mixer-v2-track-pattern-cell-0-1")
                .expect("old active pattern cell");
        let switched_new_cell =
            find_layout_node_by_stable_key(&switched_layout, "mixer-v2-track-pattern-cell-0-2")
                .expect("new active pattern cell");
        assert_eq!(
            layout_prop_bool(switched_old_cell, "active"),
            Some(false),
            "track 0 pattern 1 active binding should reflect the switched pattern"
        );
        assert_eq!(
            layout_prop_bool(switched_new_cell, "active"),
            Some(true),
            "track 0 pattern 2 active binding should reflect the switched pattern"
        );
    }

    #[test]
    #[ignore = "performance benchmark; run with --ignored --nocapture"]
    fn metal_seq_sequencer_pattern_switch_perf_10_tracks_variable_lengths() {
        #[derive(Clone, Copy)]
        struct Sample {
            update_ms: f64,
            reactive_ms: f64,
            side_effects_ms: f64,
            frame_ms: f64,
            total_ms: f64,
        }

        fn percentile(sorted: &[f64], pct: f64) -> f64 {
            let index = ((sorted.len().saturating_sub(1) as f64) * pct).round() as usize;
            sorted[index.min(sorted.len().saturating_sub(1))]
        }

        fn summarize(label: &str, values: &[f64]) {
            let mut sorted = values.to_vec();
            sorted.sort_by(|a, b| a.total_cmp(b));
            let min = sorted.first().copied().unwrap_or(0.0);
            let median = percentile(&sorted, 0.50);
            let p95 = percentile(&sorted, 0.95);
            let max = sorted.last().copied().unwrap_or(0.0);
            println!(
                "{label:>12}: min={min:7.3}ms median={median:7.3}ms p95={p95:7.3}ms max={max:7.3}ms"
            );
        }

        let track_count = 10;
        let short_pattern_steps = 32;
        let long_pattern_steps = 48;
        let warmup_iterations = 3;
        let measured_iterations = 10;
        let mut editor = sequencer_perf_editor(track_count, short_pattern_steps);

        let mut samples = Vec::with_capacity(measured_iterations);
        for generation in 1..=(warmup_iterations + measured_iterations) {
            let step_count = if generation % 2 == 0 {
                short_pattern_steps
            } else {
                long_pattern_steps
            };
            let total_start = std::time::Instant::now();

            let update_start = std::time::Instant::now();
            apply_sequencer_perf_pattern(&mut editor, track_count, step_count, generation);
            let update_ms = update_start.elapsed().as_secs_f64() * 1000.0;

            let reactive_start = std::time::Instant::now();
            editor.runtime_mut().run_reactive_cycle();
            let reactive_ms = reactive_start.elapsed().as_secs_f64() * 1000.0;

            let side_effects_start = std::time::Instant::now();
            editor.refresh_runtime_side_effects();
            let side_effects_ms = side_effects_start.elapsed().as_secs_f64() * 1000.0;
            if let Some(status) = editor.runtime_mut().take_status_message() {
                panic!("sequencer pattern switch perf status: {status}");
            }

            let frame_start = std::time::Instant::now();
            let frame = eseqlisp::frame::build_render_frame(&mut editor, 150, 44);
            let frame_ms = frame_start.elapsed().as_secs_f64() * 1000.0;
            let layout = frame
                .widget_layout
                .as_ref()
                .expect("sequencer pattern switch layout should build");
            assert_eq!(
                count_stable_key_prefix(layout, "seqv-step-cell-"),
                track_count * step_count,
                "pattern switch should keep every sequencer cell rendered"
            );

            let total_ms = total_start.elapsed().as_secs_f64() * 1000.0;
            if generation > warmup_iterations {
                samples.push(Sample {
                    update_ms,
                    reactive_ms,
                    side_effects_ms,
                    frame_ms,
                    total_ms,
                });
            }
        }

        println!(
            "sequencer pattern switch perf: {track_count} tracks, alternating {short_pattern_steps}/{long_pattern_steps} steps, {} measured iterations after {} warmups",
            measured_iterations, warmup_iterations
        );
        summarize(
            "update",
            &samples
                .iter()
                .map(|sample| sample.update_ms)
                .collect::<Vec<_>>(),
        );
        summarize(
            "reactive",
            &samples
                .iter()
                .map(|sample| sample.reactive_ms)
                .collect::<Vec<_>>(),
        );
        summarize(
            "side-effects",
            &samples
                .iter()
                .map(|sample| sample.side_effects_ms)
                .collect::<Vec<_>>(),
        );
        summarize(
            "frame",
            &samples
                .iter()
                .map(|sample| sample.frame_ms)
                .collect::<Vec<_>>(),
        );
        summarize(
            "total",
            &samples
                .iter()
                .map(|sample| sample.total_ms)
                .collect::<Vec<_>>(),
        );
    }

    #[test]
    #[ignore = "performance benchmark; run with --ignored --nocapture"]
    fn metal_seq_mixer_v2_track_pattern_grid_switch_perf_10_tracks_8_cells() {
        #[derive(Clone)]
        struct Sample {
            update_ms: f64,
            reactive_ms: f64,
            side_effects_ms: f64,
            frame_ms: f64,
            total_ms: f64,
            trace: Option<eseqlisp::runtime::UiInvalidationTrace>,
            layout_timings: Vec<eseqlisp::editor::LayoutRefreshTiming>,
        }

        fn percentile(sorted: &[f64], pct: f64) -> f64 {
            let index = ((sorted.len().saturating_sub(1) as f64) * pct).round() as usize;
            sorted[index.min(sorted.len().saturating_sub(1))]
        }

        fn summarize(label: &str, values: &[f64]) {
            let mut sorted = values.to_vec();
            sorted.sort_by(|a, b| a.total_cmp(b));
            let min = sorted.first().copied().unwrap_or(0.0);
            let median = percentile(&sorted, 0.50);
            let p95 = percentile(&sorted, 0.95);
            let max = sorted.last().copied().unwrap_or(0.0);
            println!(
                "{label:>12}: min={min:7.3}ms median={median:7.3}ms p95={p95:7.3}ms max={max:7.3}ms"
            );
        }

        let track_count = 10;
        let cell_count = 8;
        let warmup_iterations = 3;
        let measured_iterations = 10;
        let mut editor = mixer_v2_perf_editor(track_count, cell_count);

        let mut samples = Vec::with_capacity(measured_iterations);
        for generation in 1..=(warmup_iterations + measured_iterations) {
            let total_start = std::time::Instant::now();

            let update_start = std::time::Instant::now();
            apply_mixer_v2_perf_pattern(&mut editor, track_count, cell_count, generation);
            let update_ms = update_start.elapsed().as_secs_f64() * 1000.0;

            let reactive_start = std::time::Instant::now();
            editor.runtime_mut().run_reactive_cycle();
            let reactive_ms = reactive_start.elapsed().as_secs_f64() * 1000.0;

            let side_effects_start = std::time::Instant::now();
            editor.refresh_runtime_side_effects();
            let side_effects_ms = side_effects_start.elapsed().as_secs_f64() * 1000.0;
            if let Some(status) = editor.runtime_mut().take_status_message() {
                panic!("mixer pattern grid switch perf status: {status}");
            }

            let frame_start = std::time::Instant::now();
            let frame = eseqlisp::frame::build_render_frame(&mut editor, 160, 34);
            let frame_ms = frame_start.elapsed().as_secs_f64() * 1000.0;
            let layout = frame
                .widget_layout
                .as_ref()
                .expect("mixer pattern grid switch layout should build");
            assert_eq!(
                count_stable_key_prefix(layout, "mixer-v2-track-pattern-cell-"),
                track_count * cell_count,
                "pattern switch should keep every mixer pattern cell rendered"
            );

            let total_ms = total_start.elapsed().as_secs_f64() * 1000.0;
            if generation > warmup_iterations {
                samples.push(Sample {
                    update_ms,
                    reactive_ms,
                    side_effects_ms,
                    frame_ms,
                    total_ms,
                    trace: editor.runtime().last_ui_invalidation_trace(),
                    layout_timings: editor.last_layout_refresh_timings().to_vec(),
                });
            }
        }

        println!(
            "mixer v2 pattern grid switch perf: {track_count} tracks, {cell_count} cells/track, {} measured iterations after {} warmups",
            measured_iterations, warmup_iterations
        );
        summarize(
            "update",
            &samples
                .iter()
                .map(|sample| sample.update_ms)
                .collect::<Vec<_>>(),
        );
        summarize(
            "reactive",
            &samples
                .iter()
                .map(|sample| sample.reactive_ms)
                .collect::<Vec<_>>(),
        );
        summarize(
            "side-effects",
            &samples
                .iter()
                .map(|sample| sample.side_effects_ms)
                .collect::<Vec<_>>(),
        );
        summarize(
            "frame",
            &samples
                .iter()
                .map(|sample| sample.frame_ms)
                .collect::<Vec<_>>(),
        );
        summarize(
            "total",
            &samples
                .iter()
                .map(|sample| sample.total_ms)
                .collect::<Vec<_>>(),
        );

        for (idx, sample) in samples.iter().enumerate() {
            if let Some(trace) = sample.trace.as_ref() {
                println!(
                    "sample {idx:02}: dirty={:?} affected={:?} flushes={} full_reruns={} subtree_reruns={} relayout={:?} relayout_ms={:.3} failure={:?} layout_timings={:?}",
                    trace.dirty_fields,
                    trace.affected_buffers,
                    trace.widget_tree_flushes,
                    trace.full_buffer_reruns,
                    trace.subtree_reruns,
                    trace.relayout_mode,
                    trace.relayout_duration.as_secs_f64() * 1000.0,
                    trace.relayout_failure_reason,
                    sample.layout_timings,
                );
            }
        }
    }

    #[test]
    #[ignore = "performance benchmark; run with --ignored --nocapture"]
    fn metal_seq_mixer_v2_scene_switch_perf_10_tracks_8_cells() {
        #[derive(Clone)]
        struct Sample {
            update_ms: f64,
            reactive_ms: f64,
            side_effects_ms: f64,
            frame_ms: f64,
            total_ms: f64,
            trace: Option<eseqlisp::runtime::UiInvalidationTrace>,
            layout_timings: Vec<eseqlisp::editor::LayoutRefreshTiming>,
        }

        fn percentile(sorted: &[f64], pct: f64) -> f64 {
            let index = ((sorted.len().saturating_sub(1) as f64) * pct).round() as usize;
            sorted[index.min(sorted.len().saturating_sub(1))]
        }

        fn summarize(label: &str, values: &[f64]) {
            let mut sorted = values.to_vec();
            sorted.sort_by(|a, b| a.total_cmp(b));
            let min = sorted.first().copied().unwrap_or(0.0);
            let median = percentile(&sorted, 0.50);
            let p95 = percentile(&sorted, 0.95);
            let max = sorted.last().copied().unwrap_or(0.0);
            println!(
                "{label:>12}: min={min:7.3}ms median={median:7.3}ms p95={p95:7.3}ms max={max:7.3}ms"
            );
        }

        let track_count = 10;
        let cell_count = 8;
        let warmup_iterations = 3;
        let measured_iterations = 10;
        let mut editor = mixer_v2_perf_editor(track_count, cell_count);

        let mut samples = Vec::with_capacity(measured_iterations);
        for generation in 1..=(warmup_iterations + measured_iterations) {
            let total_start = std::time::Instant::now();

            let update_start = std::time::Instant::now();
            apply_mixer_v2_perf_scene_switch(&mut editor, track_count, cell_count, generation);
            let update_ms = update_start.elapsed().as_secs_f64() * 1000.0;

            let reactive_start = std::time::Instant::now();
            editor.runtime_mut().run_reactive_cycle();
            let reactive_ms = reactive_start.elapsed().as_secs_f64() * 1000.0;

            let side_effects_start = std::time::Instant::now();
            editor.refresh_runtime_side_effects();
            let side_effects_ms = side_effects_start.elapsed().as_secs_f64() * 1000.0;
            if let Some(status) = editor.runtime_mut().take_status_message() {
                panic!("mixer scene switch perf status: {status}");
            }

            let frame_start = std::time::Instant::now();
            let frame = eseqlisp::frame::build_render_frame(&mut editor, 160, 34);
            let frame_ms = frame_start.elapsed().as_secs_f64() * 1000.0;
            let layout = frame
                .widget_layout
                .as_ref()
                .expect("mixer scene switch layout should build");
            assert_eq!(
                count_stable_key_prefix(layout, "mixer-v2-track-pattern-cell-"),
                track_count * cell_count,
                "scene switch should keep every mixer pattern cell rendered"
            );

            let total_ms = total_start.elapsed().as_secs_f64() * 1000.0;
            if generation > warmup_iterations {
                samples.push(Sample {
                    update_ms,
                    reactive_ms,
                    side_effects_ms,
                    frame_ms,
                    total_ms,
                    trace: editor.runtime().last_ui_invalidation_trace(),
                    layout_timings: editor.last_layout_refresh_timings().to_vec(),
                });
            }
        }

        println!(
            "mixer v2 scene switch perf: {track_count} tracks, {cell_count} cells/track, {} measured iterations after {} warmups",
            measured_iterations, warmup_iterations
        );
        summarize(
            "update",
            &samples
                .iter()
                .map(|sample| sample.update_ms)
                .collect::<Vec<_>>(),
        );
        summarize(
            "reactive",
            &samples
                .iter()
                .map(|sample| sample.reactive_ms)
                .collect::<Vec<_>>(),
        );
        summarize(
            "side-effects",
            &samples
                .iter()
                .map(|sample| sample.side_effects_ms)
                .collect::<Vec<_>>(),
        );
        summarize(
            "frame",
            &samples
                .iter()
                .map(|sample| sample.frame_ms)
                .collect::<Vec<_>>(),
        );
        summarize(
            "total",
            &samples
                .iter()
                .map(|sample| sample.total_ms)
                .collect::<Vec<_>>(),
        );

        for (idx, sample) in samples.iter().enumerate() {
            if let Some(trace) = sample.trace.as_ref() {
                println!(
                    "sample {idx:02}: dirty={:?} affected={:?} flushes={} full_reruns={} subtree_reruns={} relayout={:?} relayout_ms={:.3} failure={:?} layout_timings={:?}",
                    trace.dirty_fields,
                    trace.affected_buffers,
                    trace.widget_tree_flushes,
                    trace.full_buffer_reruns,
                    trace.subtree_reruns,
                    trace.relayout_mode,
                    trace.relayout_duration.as_secs_f64() * 1000.0,
                    trace.relayout_failure_reason,
                    sample.layout_timings,
                );
            }
        }
    }

    #[test]
    fn metal_seq_full_grid_fx_panel_does_not_smooth_y_scroll_when_content_fits() {
        fn layout_bottom(node: &eseqlisp::layout::LayoutNode) -> f32 {
            node.children
                .iter()
                .map(layout_bottom)
                .fold(node.rect.row + node.rect.height, f32::max)
        }

        let mut editor = full_grid_editor_for_scroll_tests();
        let frame = eseqlisp::frame::build_tiled_render_frame_borderless(&mut editor, 180, 90);
        let fx_tile = frame
            .tiles
            .iter()
            .find(|tile| tile.frame.buffer_name == "*fx*")
            .expect("full grid layout should contain *fx* tile");
        let fx_tile_id = fx_tile.tile_id;
        let content_bottom = fx_tile
            .frame
            .widget_layout
            .as_ref()
            .map(|layout| layout_bottom(layout))
            .expect("fx tile should have widget layout");
        let before = editor
            .tile_root
            .find_leaf(fx_tile_id)
            .expect("fx tile leaf should exist")
            .widget_scroll_top;
        let precise_col = fx_tile.rect.col + fx_tile.rect.width * 0.5;
        let precise_row = fx_tile.rect.row + fx_tile.rect.height * 0.5;

        let widget_handled =
            editor.handle_tiled_touchpad_scroll(precise_col, precise_row, 0, 0.0, -100.0);
        if !widget_handled && editor.is_ui_scroll_mode() {
            editor.apply_smooth_widget_scroll(0.0, -5.0);
        }

        let fx_leaf = editor
            .tile_root
            .find_leaf(fx_tile_id)
            .expect("fx tile leaf should still exist");
        assert_eq!(
            fx_leaf.widget_scroll_top, before,
            "*fx* content only has fractional border/padding overflow but smooth scroll changed; tile_id={fx_tile_id}, content_bottom={content_bottom:.3}, viewport={:.3}",
            fx_leaf.widget_viewport_height
        );
    }

    #[test]
    fn metal_seq_full_grid_fx_panel_does_not_smooth_y_scroll_before_first_frame() {
        let mut editor = full_grid_editor_for_scroll_tests();
        editor.update_tile_rects(180, 90);
        let fx_tile_id = editor
            .tile_root
            .leaf_ids()
            .into_iter()
            .find(|tile_id| {
                editor
                    .tile_root
                    .find_leaf(*tile_id)
                    .and_then(|leaf| editor.buffers.get(leaf.buffer_idx))
                    .is_some_and(|buffer| buffer.name == "*fx*")
            })
            .expect("full grid layout should contain *fx* tile");
        let fx_rect = editor
            .tile_rects()
            .iter()
            .find(|(tile_id, _)| *tile_id == fx_tile_id)
            .map(|(_, rect)| *rect)
            .expect("fx tile should have a screen rect");
        let before = editor
            .tile_root
            .find_leaf(fx_tile_id)
            .expect("fx tile leaf should exist")
            .widget_scroll_top;
        let precise_col = fx_rect.col + fx_rect.width * 0.5;
        let precise_row = fx_rect.row + fx_rect.height * 0.5;

        let widget_handled =
            editor.handle_tiled_touchpad_scroll(precise_col, precise_row, 0, 0.0, -100.0);
        if !widget_handled && editor.is_ui_scroll_mode() {
            editor.apply_smooth_widget_scroll(0.0, -5.0);
        }

        let fx_leaf = editor
            .tile_root
            .find_leaf(fx_tile_id)
            .expect("fx tile leaf should still exist");
        assert_eq!(
            fx_leaf.widget_scroll_top, before,
            "*fx* smooth scroll before the first rendered frame should use exact Metal tile height; viewport={:.3}",
            fx_leaf.widget_viewport_height
        );
    }

    #[test]
    fn metal_seq_mixer_lisp_loads_and_builds_widget_tree() {
        let src = std::fs::read_to_string("metal-seq-mixer-v2.lisp").expect("read mixer lisp");
        let mut editor = eseqlisp::Editor::new(Runtime::new(), eseqlisp::EditorConfig::default());
        editor.runtime_mut().register_reactive(
            "SEQ",
            vec![
                (
                    "track-names",
                    test_list(vec![Value::String("kick".to_string())]),
                ),
                ("track-colors", test_track_colors()),
                ("track-collapsed", test_bool_list(&[false])),
                ("num-tracks", Value::Number(1.0)),
                ("current-track", Value::Number(0.0)),
                ("delete-target-version", Value::Number(0.0)),
                (
                    "record-armed",
                    test_list(vec![Value::Bool(false), Value::Bool(false)]),
                ),
                ("track-mutes", test_list(vec![Value::Bool(false)])),
                ("track-solos", test_list(vec![Value::Bool(false)])),
                ("track-muted-by-solo", test_list(vec![Value::Bool(false)])),
                (
                    "track-instrument-types",
                    test_list(vec![Value::String("modulator".to_string())]),
                ),
                (
                    "track-mod-output-available",
                    test_list(vec![Value::Bool(true)]),
                ),
                (
                    "mod-routes",
                    test_list(vec![Value::Map({
                        let mut map = std::collections::HashMap::new();
                        map.insert(
                            "source".to_string(),
                            Rc::new(RefCell::new(Value::Number(0.0))),
                        );
                        map.insert(
                            "dest".to_string(),
                            Rc::new(RefCell::new(Value::Number(1.0))),
                        );
                        map.insert(
                            "input".to_string(),
                            Rc::new(RefCell::new(Value::Number(2.0))),
                        );
                        map
                    })]),
                ),
                ("selected-mod-routes", test_list(vec![])),
                ("track-volumes", test_list(vec![Value::Number(1.0)])),
                ("track-mixer-pans", test_list(vec![Value::Number(0.0)])),
                (
                    "track-outputs",
                    test_list(vec![Value::String("main".to_string())]),
                ),
                (
                    "track-output-options",
                    test_list(vec![
                        Value::String("main".to_string()),
                        Value::String("sends only".to_string()),
                        Value::String("Bus A".to_string()),
                        Value::String("Bus B".to_string()),
                    ]),
                ),
                (
                    "track-bus-sends",
                    test_list(vec![test_list(vec![
                        test_track_bus_send(0, "Bus A", 0.0),
                        test_track_bus_send(1, "Bus B", 0.0),
                    ])]),
                ),
                ("track-0-bus-0-send", Value::Number(0.0)),
                ("track-0-bus-1-send", Value::Number(0.0)),
                ("track-peak-0", Value::Number(0.0)),
                ("master-peak-l", Value::Number(0.0)),
                ("master-peak-r", Value::Number(0.0)),
                ("bus-peak-0", Value::Number(0.0)),
                ("bus-peak-1", Value::Number(0.0)),
                (
                    "bus-names",
                    test_list(vec![
                        Value::String("Bus A".to_string()),
                        Value::String("Bus B".to_string()),
                    ]),
                ),
                (
                    "bus-volumes",
                    test_list(vec![Value::Number(1.0), Value::Number(1.0)]),
                ),
                (
                    "bus-mutes",
                    test_list(vec![Value::Bool(false), Value::Bool(false)]),
                ),
                (
                    "bus-solos",
                    test_list(vec![Value::Bool(false), Value::Bool(false)]),
                ),
            ],
            true,
        );
        editor
            .runtime_mut()
            .eval_str(
                "(defmacro aqua-slider-material () `(material :color (rgba 0.15 0.15 0.88 1.0)))",
            )
            .expect("install test slider material macro");
        editor
            .runtime_mut()
            .eval_str("(defstate selected-bus -1)")
            .expect("install shared mixer selection state");
        register_test_delete_target_natives(&mut editor, 1);
        editor
            .runtime_mut()
            .eval_str(&src)
            .expect("load mixer lisp");
        editor.refresh_runtime_side_effects();
        if let Some(status) = editor.runtime_mut().take_status_message() {
            panic!("mixer lisp status after refresh: {status}");
        }
        let mixer = editor
            .buffers
            .iter()
            .find(|buffer| buffer.name == "*mixer*")
            .expect("mixer lisp should create the *mixer* buffer");
        assert!(
            mixer.widget_tree.is_some(),
            "mixer buffer should have a widget tree"
        );
        let tree = mixer.widget_tree.as_ref().unwrap();
        assert!(
            value_contains_string(tree, "kick"),
            "mixer widget tree should contain the track row"
        );
        assert!(
            value_contains_string(tree, "Bus A") && value_contains_string(tree, "Bus B"),
            "mixer widget tree should contain bus rows"
        );
        let _ = editor.runtime_mut().take_pending_buffer_widget_trees();
        editor
            .runtime_mut()
            .set_reactive("SEQ", "track-peak-0", Value::Number(0.5));
        editor.runtime_mut().run_reactive_cycle();
        assert!(
            editor
                .runtime_mut()
                .take_pending_buffer_widget_trees()
                .is_empty(),
            "bound track peak updates must not enqueue mixer widget tree rebuilds"
        );

        let _ = editor.runtime_mut().take_pending_buffer_widget_trees();
        editor
            .runtime_mut()
            .set_reactive("SEQ", "bus-peak-1", Value::Number(0.5));
        editor.runtime_mut().run_reactive_cycle();
        assert!(
            editor
                .runtime_mut()
                .take_pending_buffer_widget_trees()
                .is_empty(),
            "bound bus peak updates must not enqueue mixer widget tree rebuilds"
        );

        let _ = editor.runtime_mut().take_pending_buffer_widget_trees();
        editor.runtime_mut().set_reactive(
            "SEQ",
            "track-volumes",
            test_list(vec![Value::Number(0.42)]),
        );
        editor.runtime_mut().run_reactive_cycle();
        assert!(
            editor
                .runtime_mut()
                .take_pending_buffer_widget_trees()
                .is_empty(),
            "bound track volume updates must not enqueue mixer widget tree rebuilds"
        );

        let _ = editor.runtime_mut().take_pending_buffer_widget_trees();
        editor.runtime_mut().set_reactive(
            "SEQ",
            "track-mixer-pans",
            test_list(vec![Value::Number(-0.35)]),
        );
        editor.runtime_mut().run_reactive_cycle();
        assert!(
            editor
                .runtime_mut()
                .take_pending_buffer_widget_trees()
                .is_empty(),
            "bound track pan updates must not enqueue mixer widget tree rebuilds"
        );

        let _ = editor.runtime_mut().take_pending_buffer_widget_trees();
        editor.runtime_mut().set_reactive(
            "SEQ",
            "bus-volumes",
            test_list(vec![Value::Number(1.0), Value::Number(0.37)]),
        );
        editor.runtime_mut().run_reactive_cycle();
        assert!(
            editor
                .runtime_mut()
                .take_pending_buffer_widget_trees()
                .is_empty(),
            "bound bus volume updates must not enqueue mixer widget tree rebuilds"
        );
    }

    #[test]
    fn metal_seq_fx_lisp_renders_track_and_bus_panels() {
        let src = std::fs::read_to_string("metal-seq-fx.lisp").expect("read fx lisp");
        let mut editor = eseqlisp::Editor::new(Runtime::new(), eseqlisp::EditorConfig::default());
        editor.runtime_mut().register_reactive(
            "SEQ",
            vec![
                ("num-tracks", Value::Number(1.0)),
                ("compiling", Value::Bool(false)),
                ("delete-target-version", Value::Number(0.0)),
                (
                    "available-effects",
                    test_list(vec![Value::String("limiter".to_string())]),
                ),
                (
                    "available-builtin-effects",
                    test_list(vec![Value::String("Filter".to_string())]),
                ),
                ("available-midi-effects", test_list(vec![])),
                (
                    "bus-names",
                    test_list(vec![
                        Value::String("Mix".to_string()),
                        Value::String("Bus A".to_string()),
                        Value::String("Bus B".to_string()),
                    ]),
                ),
                (
                    "effects",
                    test_list(vec![
                        Value::Map(test_fx_map("Filter", 0, test_filter_params())),
                        Value::Map(test_fx_map(
                            "track-fx",
                            2,
                            vec![Value::Map(test_param_map("gain", 0, 0.5, 0.0, 1.0))],
                        )),
                    ]),
                ),
                ("midi-effects", test_list(vec![])),
                (
                    "instrument-panel",
                    test_list(vec![Value::Map(test_instrument_map())]),
                ),
                (
                    "bus-effects",
                    test_list(vec![
                        test_list(vec![]),
                        test_list(vec![Value::Map(test_bus_fx_map(
                            "bus-fx",
                            1,
                            0,
                            vec![
                                Value::Map(test_param_map("rate", 0, 0.32, 0.05, 1.2)),
                                Value::Map(test_param_map("depth", 1, 8.5, 1.0, 20.0)),
                                Value::Map(test_param_map("base", 2, 12.5, 6.0, 28.0)),
                                Value::Map(test_param_map("spread", 3, 6.0, 0.0, 14.0)),
                                Value::Map(test_param_map("mix", 4, 0.68, 0.0, 1.0)),
                                Value::Map(test_param_map("tone", 5, 10500.0, 2000.0, 18000.0)),
                                Value::Map(test_param_map("width", 6, 1.0, 0.0, 1.0)),
                                Value::Map(test_param_map("shimmer", 7, 0.28, 0.0, 1.0)),
                            ],
                        ))]),
                        test_list(vec![]),
                    ]),
                ),
            ],
            true,
        );
        editor
            .runtime_mut()
            .eval_str(
                r#"
                (def selected-bus-name () "Mix")
                (def seq-has-selection? () false)
                (def sbrowser-editor-name "")
                (defmacro aqua-slider-material () `(material :color (rgba 0.15 0.15 0.88 1.0)))
                (def custom-instrument-synth-ui (inst) false)
                (def custom-midi-fx-ui (fx) false)
                (def custom-audio-fx-ui (fx) false)
                (defstate selected-bus -1)
                "#,
            )
            .expect("install fx test helpers");
        register_test_delete_target_natives(&mut editor, 1);
        editor.runtime_mut().eval_str(&src).expect("load fx lisp");
        editor
            .runtime_mut()
            .set_reactive("SEQ", "current-track", Value::Number(0.0));
        editor
            .runtime_mut()
            .eval_str(
                r#"(fx-drop-on-effect
                    (dict
                      :payload (dict :kind "builtin-audio-effect" :name "Filter")
                      :target (dict :chain "audio" :track 0 :slot 2)))"#,
            )
            .expect("drop builtin audio effect before slot");
        let commands = editor.drain_host_commands();
        assert_eq!(commands.len(), 1);
        match &commands[0] {
            eseqlisp::host::HostCommand::Custom { name, payload } => {
                assert_eq!(name, "insert-builtin-effect-before-slot");
                let Value::Map(payload) = payload else {
                    panic!(
                        "insert-builtin-effect-before-slot payload should be a dict: {payload:?}"
                    );
                };
                assert_eq!(
                    payload.get("track").map(|value| value.borrow().clone()),
                    Some(Value::Number(0.0))
                );
                assert_eq!(
                    payload.get("slot").map(|value| value.borrow().clone()),
                    Some(Value::Number(2.0))
                );
                assert_eq!(
                    payload.get("name").map(|value| value.borrow().clone()),
                    Some(Value::String("Filter".to_string()))
                );
            }
            other => panic!("expected insert-builtin-effect-before-slot, got {other:?}"),
        }
        editor
            .runtime_mut()
            .eval_str(
                r#"(fx-drop-on-effect
                    (dict
                      :payload (dict :kind "audio-effect-instance" :chain "audio" :track 0 :slot 3 :name "Delay")
                      :target (dict :chain "audio" :track 0 :slot 2)))"#,
            )
            .expect("drop audio effect instance before slot");
        let commands = editor.drain_host_commands();
        assert_eq!(commands.len(), 1);
        match &commands[0] {
            eseqlisp::host::HostCommand::Custom { name, payload } => {
                assert_eq!(name, "move-effect-slot");
                let Value::Map(payload) = payload else {
                    panic!("move-effect-slot payload should be a dict: {payload:?}");
                };
                assert_eq!(
                    payload
                        .get("source-slot")
                        .map(|value| value.borrow().clone()),
                    Some(Value::Number(3.0))
                );
                assert_eq!(
                    payload
                        .get("target-slot")
                        .map(|value| value.borrow().clone()),
                    Some(Value::Number(2.0))
                );
            }
            other => panic!("expected move-effect-slot, got {other:?}"),
        }
        editor
            .runtime_mut()
            .eval_str(
                r#"(fx-drop-on-effect
                    (dict
                      :payload (dict :kind "midi-effect" :name "Arp")
                      :target (dict :chain "midi" :track 0 :slot 1)))"#,
            )
            .expect("drop MIDI effect before slot");
        let commands = editor.drain_host_commands();
        assert_eq!(commands.len(), 1);
        match &commands[0] {
            eseqlisp::host::HostCommand::Custom { name, payload } => {
                assert_eq!(name, "insert-midi-fx-before-slot");
                let Value::Map(payload) = payload else {
                    panic!("insert-midi-fx-before-slot payload should be a dict: {payload:?}");
                };
                assert_eq!(
                    payload.get("slot").map(|value| value.borrow().clone()),
                    Some(Value::Number(1.0))
                );
                assert_eq!(
                    payload.get("name").map(|value| value.borrow().clone()),
                    Some(Value::String("Arp".to_string()))
                );
            }
            other => panic!("expected insert-midi-fx-before-slot, got {other:?}"),
        }
        editor
            .runtime_mut()
            .eval_str(
                r#"(fx-drop-on-effect
                    (dict
                      :payload (dict :kind "builtin-audio-effect" :name "Filter")
                      :target (dict :chain "bus" :bus 1 :slot 0)))"#,
            )
            .expect("drop builtin audio effect before bus slot");
        let commands = editor.drain_host_commands();
        assert_eq!(commands.len(), 1);
        match &commands[0] {
            eseqlisp::host::HostCommand::Custom { name, payload } => {
                assert_eq!(name, "insert-builtin-bus-effect-before-slot");
                let Value::Map(payload) = payload else {
                    panic!(
                        "insert-builtin-bus-effect-before-slot payload should be a dict: {payload:?}"
                    );
                };
                assert_eq!(
                    payload.get("bus").map(|value| value.borrow().clone()),
                    Some(Value::Number(1.0))
                );
                assert_eq!(
                    payload.get("slot").map(|value| value.borrow().clone()),
                    Some(Value::Number(0.0))
                );
            }
            other => panic!("expected insert-builtin-bus-effect-before-slot, got {other:?}"),
        }
        editor
            .runtime_mut()
            .eval_str(
                r#"(fx-drop-on-effect
                    (dict
                      :payload (dict :kind "bus-effect-instance" :chain "bus" :bus 1 :slot 2 :name "Delay")
                      :target (dict :chain "bus" :bus 1 :slot 0)))"#,
            )
            .expect("drop bus effect instance before bus slot");
        let commands = editor.drain_host_commands();
        assert_eq!(commands.len(), 1);
        match &commands[0] {
            eseqlisp::host::HostCommand::Custom { name, payload } => {
                assert_eq!(name, "move-bus-effect-slot");
                let Value::Map(payload) = payload else {
                    panic!("move-bus-effect-slot payload should be a dict: {payload:?}");
                };
                assert_eq!(
                    payload.get("bus").map(|value| value.borrow().clone()),
                    Some(Value::Number(1.0))
                );
                assert_eq!(
                    payload
                        .get("source-slot")
                        .map(|value| value.borrow().clone()),
                    Some(Value::Number(2.0))
                );
                assert_eq!(
                    payload
                        .get("target-slot")
                        .map(|value| value.borrow().clone()),
                    Some(Value::Number(0.0))
                );
            }
            other => panic!("expected move-bus-effect-slot, got {other:?}"),
        }
        editor
            .runtime_mut()
            .eval_str(
                r#"(fx-drop-on-effect
                    (dict
                      :payload (dict :kind "custom-audio-effect" :name "verb")
                      :target (dict :chain "append" :bus 1 :track 0 :slot -1)))"#,
            )
            .expect("drop custom audio effect onto bus append target");
        let commands = editor.drain_host_commands();
        assert_eq!(commands.len(), 1);
        match &commands[0] {
            eseqlisp::host::HostCommand::Custom { name, payload } => {
                assert_eq!(name, "add-bus-effect");
                let Value::Map(payload) = payload else {
                    panic!("add-bus-effect payload should be a dict: {payload:?}");
                };
                assert_eq!(
                    payload.get("bus").map(|value| value.borrow().clone()),
                    Some(Value::Number(1.0))
                );
                assert_eq!(
                    payload.get("name").map(|value| value.borrow().clone()),
                    Some(Value::String("verb".to_string()))
                );
            }
            other => panic!("expected add-bus-effect, got {other:?}"),
        }
        editor
            .runtime_mut()
            .eval_str(
                "(do (set! sampler-view-start 3.5)
                     (set! sampler-view-duration 1.25)
                     (set! sampler-cursor-time 3.75)
                     (set! sampler-active-marker \"start\")
                     (sampler-reset-view))",
            )
            .expect("reset sampler waveform viewport");
        assert_eq!(
            editor
                .runtime_mut()
                .eval_str("sampler-view-start")
                .expect("read sampler view start"),
            Some(Value::Number(0.0))
        );
        assert_eq!(
            editor
                .runtime_mut()
                .eval_str("sampler-view-duration")
                .expect("read sampler view duration"),
            Some(Value::Number(0.0))
        );
        assert_eq!(
            editor
                .runtime_mut()
                .eval_str("sampler-cursor-time")
                .expect("read sampler cursor time"),
            Some(Value::Number(0.0))
        );
        assert_eq!(
            editor
                .runtime_mut()
                .eval_str("sampler-active-marker")
                .expect("read sampler active marker"),
            Some(Value::String("none".to_string()))
        );
        let filter_ui_probe = editor
            .runtime_mut()
            .eval_str("(builtin-audio-fx-ui (nth SEQ.effects 0))")
            .expect("probe filter ui")
            .expect("filter ui probe value");
        assert!(
            value_contains_keyword(&filter_ui_probe, "response-curve-editor"),
            "filter custom UI probe did not contain response editor: {filter_ui_probe:?}"
        );
        editor.refresh_runtime_side_effects();
        if let Some(status) = editor.runtime_mut().take_status_message() {
            panic!("track fx lisp status after refresh: {status}");
        }
        let fx = editor
            .buffers
            .iter()
            .find(|buffer| buffer.name == "*fx*")
            .expect("fx lisp should create the *fx* buffer");
        let tree = fx.widget_tree.as_ref().expect("track fx tree");
        assert!(value_contains_keyword(tree, "response-curve-editor"));
        assert!(value_contains_string(tree, "track-fx"));
        assert!(value_contains_string(tree, "test-instru"));
        assert!(
            !value_contains_string(tree, "Add Effect"),
            "fx buffer should not render the old add-effect dropdown panel"
        );
        assert!(value_contains_string(
            tree,
            "Drop Audio or Midi Effect Here"
        ));
        editor
            .runtime_mut()
            .eval_str("(set! selected-bus 1)")
            .expect("select bus");
        editor.refresh_runtime_side_effects();
        if let Some(status) = editor.runtime_mut().take_status_message() {
            panic!("bus fx lisp status after refresh: {status}");
        }
        let fx = editor
            .buffers
            .iter()
            .find(|buffer| buffer.name == "*fx*")
            .expect("fx buffer exists");
        let tree = fx.widget_tree.as_ref().expect("bus fx tree");
        assert!(value_contains_string(tree, "bus-fx"));
        assert!(
            !value_contains_string(tree, "Add Bus FX"),
            "fx buffer should not render the old bus add-effect dropdown panel"
        );
        assert!(value_contains_string(
            tree,
            "Drop Audio or Midi Effect Here"
        ));
        editor.set_active_buffer(fx.id);
        editor.set_layout_viewport(92, 42);
        let layout = editor.widget_layout().expect("bus fx layout");
        let placeholder = find_layout_node_by_debug_name(&layout, "fx-drop-placeholder-panel")
            .expect("bus fx drop placeholder panel");
        assert!(
            placeholder.rect.width >= 30.0
                && placeholder.rect.height > 0.0
                && placeholder.props.contains_key("on-drop"),
            "bus fx drop placeholder should be the visible append drop target, got rect={:?} props={:?}",
            placeholder.rect,
            placeholder.props.keys().collect::<Vec<_>>()
        );
    }

    #[test]
    fn metal_seq_fx_filter_layout_contains_response_curve_editor() {
        let src = std::fs::read_to_string("metal-seq-fx.lisp").expect("read fx lisp");
        let mut editor = eseqlisp::Editor::new(Runtime::new(), eseqlisp::EditorConfig::default());
        editor.runtime_mut().register_reactive(
            "SEQ",
            vec![
                ("num-tracks", Value::Number(1.0)),
                ("compiling", Value::Bool(false)),
                ("available-effects", test_list(vec![])),
                (
                    "available-builtin-effects",
                    test_list(vec![Value::String("Filter".to_string())]),
                ),
                ("available-midi-effects", test_list(vec![])),
                (
                    "bus-names",
                    test_list(vec![Value::String("Mix".to_string())]),
                ),
                (
                    "effects",
                    test_list(vec![Value::Map(test_fx_map(
                        "Filter",
                        0,
                        test_filter_params(),
                    ))]),
                ),
                ("midi-effects", test_list(vec![])),
                (
                    "instrument-panel",
                    test_list(vec![Value::Map(test_instrument_map())]),
                ),
                ("bus-effects", test_list(vec![test_list(vec![])])),
            ],
            true,
        );
        editor
            .runtime_mut()
            .eval_str(
                r#"
                (def selected-bus-name () "Mix")
                (def seq-has-selection? () false)
                (def sbrowser-editor-name "")
                (defmacro aqua-slider-material () `(material :color (rgba 0.15 0.15 0.88 1.0)))
                (def custom-instrument-synth-ui (inst) false)
                (def custom-midi-fx-ui (fx) false)
                (def custom-audio-fx-ui (fx) false)
                (defstate selected-bus -1)
                "#,
            )
            .expect("install fx test helpers");
        register_test_delete_target_natives(&mut editor, 1);
        editor.runtime_mut().eval_str(&src).expect("load fx lisp");
        let filter_ui_probe = editor
            .runtime_mut()
            .eval_str("(builtin-audio-fx-ui (nth SEQ.effects 0))")
            .expect("probe filter ui")
            .expect("filter ui probe value");
        assert!(
            value_contains_keyword(&filter_ui_probe, "response-curve-editor"),
            "filter custom UI probe did not contain response editor: {filter_ui_probe:?}"
        );
        editor.refresh_runtime_side_effects();
        if let Some(status) = editor.runtime_mut().take_status_message() {
            panic!("filter fx lisp status after refresh: {status}");
        }
        let fx_id = editor
            .buffers
            .iter()
            .find(|buffer| buffer.name == "*fx*")
            .expect("fx lisp should create the *fx* buffer")
            .id;
        editor.set_active_buffer(fx_id);
        editor.set_layout_viewport(120, 18);
        let layout = editor.widget_layout().expect("filter fx layout");
        assert!(
            layout_contains_debug_name(&layout, "audio-fx-panel-root-0-Filter"),
            "filter layout should contain the built-in Filter panel"
        );
        assert!(
            layout_contains_widget_type(&layout, "response-curve-editor"),
            "filter layout should contain the response curve editor"
        );
    }

    #[test]
    fn metal_seq_fx_panel_headers_share_height_and_visible_content() {
        fn assert_visible_inside(
            node: &eseqlisp::layout::LayoutNode,
            parent: &eseqlisp::layout::LayoutNode,
            label: &str,
        ) {
            let eps = 0.05;
            assert!(
                node.rect.width.is_finite()
                    && node.rect.height.is_finite()
                    && node.rect.width > 0.0
                    && node.rect.height > 0.0
                    && node.rect.col + eps >= parent.rect.col
                    && node.rect.row + eps >= parent.rect.row
                    && node.rect.col + node.rect.width <= parent.rect.col + parent.rect.width + eps
                    && node.rect.row + node.rect.height
                        <= parent.rect.row + parent.rect.height + eps,
                "{label} should have a finite nonzero rect inside its parent; node={:?}; parent={:?}",
                node.rect,
                parent.rect
            );
        }

        let src = std::fs::read_to_string("metal-seq-fx.lisp").expect("read fx lisp");
        let mut editor = eseqlisp::Editor::new(Runtime::new(), eseqlisp::EditorConfig::default());
        editor.set_layout_viewport(180, 18);
        editor.runtime_mut().register_reactive(
            "SEQ",
            vec![
                ("num-tracks", Value::Number(1.0)),
                ("compiling", Value::Bool(false)),
                ("tp-gate", Value::Bool(false)),
                ("available-effects", test_list(vec![])),
                (
                    "available-builtin-effects",
                    test_list(vec![Value::String("Filter".to_string())]),
                ),
                ("available-midi-effects", test_list(vec![])),
                (
                    "bus-names",
                    test_list(vec![Value::String("Mix".to_string())]),
                ),
                (
                    "effects",
                    test_list(vec![Value::Map(test_fx_map(
                        "Filter",
                        0,
                        test_filter_params(),
                    ))]),
                ),
                ("midi-effects", test_list(vec![])),
                (
                    "instrument-panel",
                    test_list(vec![Value::Map(test_sampler_instrument_map(0))]),
                ),
                ("sampler-playhead", Value::Number(0.0)),
                ("bus-effects", test_list(vec![test_list(vec![])])),
            ],
            true,
        );
        editor
            .runtime_mut()
            .eval_str(
                r#"
                (def selected-bus-name () "Mix")
                (def seq-has-selection? () false)
                (def sbrowser-editor-name "")
                (defmacro aqua-slider-material () `(material :color (rgba 0.15 0.15 0.88 1.0)))
                (def custom-instrument-synth-ui (inst) false)
                (def custom-midi-fx-ui (fx) false)
                (def custom-audio-fx-ui (fx) false)
                (defstate selected-bus -1)
                "#,
            )
            .expect("install fx test helpers");
        register_test_delete_target_natives(&mut editor, 1);
        editor.runtime_mut().eval_str(&src).expect("load fx lisp");
        editor.refresh_runtime_side_effects();
        if let Some(status) = editor.runtime_mut().take_status_message() {
            panic!("fx lisp status after refresh: {status}");
        }

        let fx_id = editor
            .buffers
            .iter()
            .find(|buffer| buffer.name == "*fx*")
            .expect("fx lisp should create the *fx* buffer")
            .id;
        editor.set_active_buffer(fx_id);
        let layout = editor.widget_layout().expect("fx panel layout");
        assert_finite_layout_tree(&layout);

        let mut layout_summaries = Vec::new();
        collect_layout_node_summaries(&layout, &mut layout_summaries);
        let sampler_panel = find_layout_node_by_debug_name(&layout, "sampler-panel")
            .unwrap_or_else(|| panic!("sampler panel; layout={layout_summaries:#?}"));
        let filter_panel = find_layout_node_by_debug_name(&layout, "audio-fx-panel-root-0-Filter")
            .unwrap_or_else(|| panic!("filter panel; layout={layout_summaries:#?}"));
        let sampler_header = find_layout_node_by_debug_name(&layout, "sampler-header-box")
            .unwrap_or_else(|| panic!("sampler header; layout={layout_summaries:#?}"));
        let filter_header = find_layout_node_by_debug_name(&layout, "audio-fx-panel-header")
            .unwrap_or_else(|| panic!("filter header; layout={layout_summaries:#?}"));
        let sampler_body = find_layout_node_by_debug_name(&layout, "sampler-panel-content")
            .unwrap_or_else(|| panic!("sampler body; layout={layout_summaries:#?}"));
        let filter_body = find_layout_node_by_debug_name(&layout, "audio-fx-panel-content")
            .unwrap_or_else(|| panic!("filter body; layout={layout_summaries:#?}"));
        let sampler_label = find_layout_node_by_text(&layout, "Sampler")
            .unwrap_or_else(|| panic!("sampler label; layout={layout_summaries:#?}"));
        let filter_label = find_layout_node_by_text(&layout, "Filter")
            .unwrap_or_else(|| panic!("filter label; layout={layout_summaries:#?}"));

        assert!(
            (sampler_panel.rect.height - filter_panel.rect.height).abs() < 0.01,
            "instrument and effect panels should share the same fixed height; sampler={:?}; filter={:?}",
            sampler_panel.rect,
            filter_panel.rect
        );
        assert!(
            (sampler_header.rect.height - filter_header.rect.height).abs() < 0.01,
            "sampler and effect headers should reserve the same height; sampler={:?}; filter={:?}",
            sampler_header.rect,
            filter_header.rect
        );
        assert!(
            (sampler_header.rect.height - 0.75).abs() < 0.01,
            "shared panel header height should stay on the compact instrument contract; got {:?}",
            sampler_header.rect
        );
        assert_visible_inside(sampler_header, sampler_panel, "sampler header");
        assert_visible_inside(filter_header, filter_panel, "filter header");
        assert_visible_inside(sampler_body, sampler_panel, "sampler body");
        assert_visible_inside(filter_body, filter_panel, "filter body");
        assert_visible_inside(sampler_label, sampler_header, "sampler title");
        assert_visible_inside(filter_label, filter_header, "filter title");
    }

    #[test]
    fn metal_seq_sampler_panel_accepts_sample_drops_for_current_track() {
        let src = std::fs::read_to_string("metal-seq-fx.lisp").expect("read fx lisp");
        let mut editor = eseqlisp::Editor::new(Runtime::new(), eseqlisp::EditorConfig::default());
        editor.set_layout_viewport(160, 18);
        editor.runtime_mut().register_reactive(
            "SEQ",
            vec![
                ("num-tracks", Value::Number(3.0)),
                ("compiling", Value::Bool(false)),
                ("tp-gate", Value::Bool(false)),
                ("available-effects", test_list(vec![])),
                ("available-builtin-effects", test_list(vec![])),
                ("available-midi-effects", test_list(vec![])),
                ("bus-names", test_list(vec![])),
                ("effects", test_list(vec![])),
                ("midi-effects", test_list(vec![])),
                (
                    "instrument-panel",
                    test_list(vec![Value::Map(test_sampler_instrument_map(2))]),
                ),
                ("sampler-playhead", Value::Number(0.0)),
                ("bus-effects", test_list(vec![])),
            ],
            true,
        );
        editor
            .runtime_mut()
            .eval_str(
                r#"
                (def selected-bus-name () "Mix")
                (def seq-has-selection? () false)
                (def sbrowser-editor-name "")
                (defmacro aqua-slider-material () `(material :color (rgba 0.15 0.15 0.88 1.0)))
                (def custom-instrument-synth-ui (inst) false)
                (def custom-midi-fx-ui (fx) false)
                (def custom-audio-fx-ui (fx) false)
                (defstate selected-bus -1)
                "#,
            )
            .expect("install fx test helpers");
        register_test_delete_target_natives(&mut editor, 1);
        editor.runtime_mut().eval_str(&src).expect("load fx lisp");
        editor.refresh_runtime_side_effects();
        if let Some(status) = editor.runtime_mut().take_status_message() {
            panic!("sampler fx lisp status after refresh: {status}");
        }

        let fx_id = editor
            .buffers
            .iter()
            .find(|buffer| buffer.name == "*fx*")
            .expect("fx lisp should create the *fx* buffer")
            .id;
        editor.set_active_buffer(fx_id);
        let layout = editor.widget_layout().expect("sampler panel layout");
        let panel = find_layout_node_by_debug_name(&layout, "sampler-panel")
            .expect("sampler panel should render");
        assert!(
            panel.props.contains_key("on-drop"),
            "sampler panel should expose an on-drop callback"
        );
        assert!(
            matches!(
                panel.props.get("drop-types"),
                Some(Value::List(items))
                    if items.iter().any(|item| matches!(&*item.borrow(), Value::String(value) if value == "sample"))
            ),
            "sampler panel should accept sample drops: {:?}",
            panel.props.get("drop-types")
        );
        assert!(
            matches!(
                panel.props.get("drop-meta"),
                Some(Value::Map(map))
                    if map
                        .get("track")
                        .is_some_and(|value| matches!(*value.borrow(), Value::Number(track) if (track - 2.0).abs() < f64::EPSILON))
            ),
            "sampler panel drop metadata should target track 2: {:?}",
            panel.props.get("drop-meta")
        );

        editor
            .runtime_mut()
            .eval_str(
                r#"(sampler-panel-drop-sample
                    (dict :payload (dict :path "samples/snare.wav")
                          :target (dict :track 2)))"#,
            )
            .expect("drop sample on sampler panel");
        let commands = editor.drain_host_commands();
        assert_eq!(commands.len(), 1);
        match &commands[0] {
            eseqlisp::host::HostCommand::Custom { name, payload } => {
                assert_eq!(name, "load-sample-into-track");
                let Value::Map(payload) = payload else {
                    panic!("sampler panel drop payload should be a dict: {payload:?}");
                };
                assert_eq!(
                    payload.get("track").map(|value| value.borrow().clone()),
                    Some(Value::Number(2.0))
                );
                assert_eq!(
                    payload.get("path").map(|value| value.borrow().clone()),
                    Some(Value::String("samples/snare.wav".to_string()))
                );
                assert_eq!(
                    payload
                        .get("preserve-browser-context")
                        .map(|value| value.borrow().clone()),
                    Some(Value::Bool(true))
                );
            }
            other => panic!("expected load-sample-into-track host command, got {other:?}"),
        }
    }

    #[test]
    fn metal_seq_fx_str8_delay_layout_contains_curve_and_offsets() {
        let src = std::fs::read_to_string("metal-seq-fx.lisp").expect("read fx lisp");
        let mut editor = eseqlisp::Editor::new(Runtime::new(), eseqlisp::EditorConfig::default());
        editor.runtime_mut().register_reactive(
            "SEQ",
            vec![
                ("num-tracks", Value::Number(1.0)),
                ("compiling", Value::Bool(false)),
                ("available-effects", test_list(vec![])),
                (
                    "available-builtin-effects",
                    test_list(vec![Value::String("Str8 Delay".to_string())]),
                ),
                ("available-midi-effects", test_list(vec![])),
                (
                    "bus-names",
                    test_list(vec![Value::String("Mix".to_string())]),
                ),
                (
                    "effects",
                    test_list(vec![Value::Map(test_fx_map(
                        "Str8 Delay",
                        0,
                        test_str8_delay_params(),
                    ))]),
                ),
                ("midi-effects", test_list(vec![])),
                (
                    "instrument-panel",
                    test_list(vec![Value::Map(test_instrument_map())]),
                ),
                ("bus-effects", test_list(vec![test_list(vec![])])),
            ],
            true,
        );
        editor
            .runtime_mut()
            .eval_str(
                r#"
                (def selected-bus-name () "Mix")
                (def seq-has-selection? () false)
                (def sbrowser-editor-name "")
                (defmacro aqua-slider-material () `(material :color (rgba 0.15 0.15 0.88 1.0)))
                (def custom-instrument-synth-ui (inst) false)
                (def custom-midi-fx-ui (fx) false)
                (def custom-audio-fx-ui (fx) false)
                (defstate selected-bus -1)
                "#,
            )
            .expect("install fx test helpers");
        register_test_delete_target_natives(&mut editor, 1);
        editor.runtime_mut().eval_str(&src).expect("load fx lisp");
        let delay_ui_probe = editor
            .runtime_mut()
            .eval_str("(builtin-audio-fx-ui (nth SEQ.effects 0))")
            .expect("probe str8 delay ui")
            .expect("str8 delay ui probe value");
        assert!(
            value_contains_keyword(&delay_ui_probe, "response-curve-editor"),
            "str8 delay custom UI probe did not contain response editor: {delay_ui_probe:?}"
        );
        assert!(value_contains_string(&delay_ui_probe, "ofs"));
        editor.refresh_runtime_side_effects();
        if let Some(status) = editor.runtime_mut().take_status_message() {
            panic!("str8 delay fx lisp status after refresh: {status}");
        }
        let fx_id = editor
            .buffers
            .iter()
            .find(|buffer| buffer.name == "*fx*")
            .expect("fx lisp should create the *fx* buffer")
            .id;
        editor.set_active_buffer(fx_id);
        editor.set_layout_viewport(128, 20);
        let layout = editor.widget_layout().expect("str8 delay fx layout");
        assert!(
            layout_contains_debug_name(&layout, "audio-fx-panel-root-0-Str8 Delay"),
            "layout should contain the built-in Str8 Delay panel"
        );
        assert!(
            layout_contains_widget_type(&layout, "response-curve-editor"),
            "layout should contain the response curve editor"
        );
        let mut layout_summaries = Vec::new();
        collect_layout_node_summaries(&layout, &mut layout_summaries);
        let panel = find_layout_node_by_debug_name(&layout, "audio-fx-panel-root-0-Str8 Delay")
            .unwrap_or_else(|| panic!("str8 delay panel; layout={layout_summaries:#?}"));
        let header = find_layout_node_by_debug_name(&layout, "audio-fx-panel-header")
            .unwrap_or_else(|| panic!("str8 delay header; layout={layout_summaries:#?}"));
        let body = find_layout_node_by_debug_name(&layout, "audio-fx-panel-content")
            .unwrap_or_else(|| panic!("str8 delay body; layout={layout_summaries:#?}"));
        let response = find_layout_node_by_widget_type(&layout, "response-curve-editor")
            .unwrap_or_else(|| panic!("str8 delay response curve; layout={layout_summaries:#?}"));
        assert!(
            !panel.props.contains_key("on-click"),
            "only the FX panel header should select the effect"
        );
        assert!(
            header.props.contains_key("on-click"),
            "FX panel header should remain clickable for effect selection"
        );
        assert!(
            header.rect.width >= panel.rect.width - 0.05,
            "FX panel header hit area should span the full panel width, panel={:?} header={:?}",
            panel.rect,
            header.rect
        );
        assert!(
            body.props.contains_key("on-click"),
            "FX panel body should clear effect selection when blank/non-control space is clicked"
        );
        let hit = eseqlisp::layout::hit_test_layout(
            &layout,
            response.rect.row + response.rect.height * 0.5,
            response.rect.col + response.rect.width * 0.5,
        )
        .unwrap_or_else(|| panic!("response curve hit test missed; layout={layout_summaries:#?}"));
        assert_eq!(
            hit.widget_type, "response-curve-editor",
            "dragging the response curve should hit the curve editor, not the FX panel/body"
        );
    }

    #[test]
    fn metal_seq_fx_builtin_without_custom_ui_falls_back_to_param_grid() {
        let src = std::fs::read_to_string("metal-seq-fx.lisp").expect("read fx lisp");
        let mut editor = eseqlisp::Editor::new(Runtime::new(), eseqlisp::EditorConfig::default());
        editor.runtime_mut().register_reactive(
            "SEQ",
            vec![
                ("num-tracks", Value::Number(1.0)),
                ("compiling", Value::Bool(false)),
                ("available-effects", test_list(vec![])),
                (
                    "available-builtin-effects",
                    test_list(vec![Value::String("Reverb".to_string())]),
                ),
                ("available-midi-effects", test_list(vec![])),
                (
                    "bus-names",
                    test_list(vec![Value::String("Mix".to_string())]),
                ),
                (
                    "effects",
                    test_list(vec![Value::Map(test_fx_map(
                        "Reverb",
                        0,
                        test_reverb_params(),
                    ))]),
                ),
                ("midi-effects", test_list(vec![])),
                (
                    "instrument-panel",
                    test_list(vec![Value::Map(test_instrument_map())]),
                ),
                ("bus-effects", test_list(vec![test_list(vec![])])),
            ],
            true,
        );
        editor
            .runtime_mut()
            .eval_str(
                r#"
                (def selected-bus-name () "Mix")
                (def seq-has-selection? () false)
                (def sbrowser-editor-name "")
                (defmacro aqua-slider-material () `(material :color (rgba 0.15 0.15 0.88 1.0)))
                (def custom-instrument-synth-ui (inst) false)
                (def custom-midi-fx-ui (fx) false)
                (def custom-audio-fx-ui (fx) false)
                (defstate selected-bus -1)
                "#,
            )
            .expect("install fx test helpers");
        register_test_delete_target_natives(&mut editor, 1);
        editor.runtime_mut().eval_str(&src).expect("load fx lisp");
        let reverb_ui_probe = editor
            .runtime_mut()
            .eval_str("(builtin-audio-fx-ui (nth SEQ.effects 0))")
            .expect("probe reverb ui");
        assert!(
            matches!(reverb_ui_probe, Some(Value::Bool(false))),
            "Reverb should not have a bespoke built-in UI: {reverb_ui_probe:?}"
        );
        editor.refresh_runtime_side_effects();
        if let Some(status) = editor.runtime_mut().take_status_message() {
            panic!("reverb fx lisp status after refresh: {status}");
        }
        let fx_id = editor
            .buffers
            .iter()
            .find(|buffer| buffer.name == "*fx*")
            .expect("fx lisp should create the *fx* buffer")
            .id;
        editor.set_active_buffer(fx_id);
        editor.set_layout_viewport(120, 18);
        let layout = editor.widget_layout().expect("reverb fx layout");
        assert!(
            layout_contains_debug_name(&layout, "audio-fx-panel-root-0-Reverb"),
            "layout should contain the Reverb panel"
        );
        let mut layout_summaries = Vec::new();
        collect_layout_node_summaries(&layout, &mut layout_summaries);
        let mix = find_layout_node_by_text(&layout, "mix")
            .unwrap_or_else(|| panic!("Reverb mix parameter label; layout={layout_summaries:#?}"));
        let size = find_layout_node_by_text(&layout, "size")
            .unwrap_or_else(|| panic!("Reverb size parameter label; layout={layout_summaries:#?}"));
        assert!(
            mix.rect.width > 0.5 && size.rect.width > 0.5,
            "Reverb fallback param grid should have visible labels, mix={:?} size={:?}",
            mix.rect,
            size.rect
        );
    }

    #[test]
    fn param_grid_without_metadata_uses_flat_fallback() {
        let editor = param_grid_test_editor(vec![
            Value::Map(test_param_map("mix", 0, 0.35, 0.0, 1.0)),
            Value::Map(test_param_map("size", 1, 0.2, 0.0, 1.0)),
            Value::Map(test_param_map("enabled", 2, 1.0, 0.0, 1.0)),
        ]);
        let layout = editor.widget_layout().expect("flat param grid layout");
        assert_finite_layout_tree(&layout);
        assert!(
            find_layout_node_by_debug_name(&layout, "fx-param-group-").is_none(),
            "flat fallback should not render metadata group panels"
        );
        let mut layout_summaries = Vec::new();
        collect_layout_node_summaries(&layout, &mut layout_summaries);
        let mix = find_layout_node_by_text(&layout, "mix")
            .unwrap_or_else(|| panic!("mix label; layout={layout_summaries:#?}"));
        let size = find_layout_node_by_text(&layout, "size")
            .unwrap_or_else(|| panic!("size label; layout={layout_summaries:#?}"));
        assert!(
            mix.rect.width > 0.5 && size.rect.width > 0.5,
            "flat fallback labels should remain visible, mix={:?} size={:?}",
            mix.rect,
            size.rect
        );
    }

    #[test]
    fn param_grid_groups_metadata_params_into_visible_panels() {
        let editor = param_grid_test_editor(vec![
            Value::Map(test_param_map_with_ui_metadata(
                "osc_gain",
                0,
                0.5,
                0.0,
                1.0,
                Some("osc"),
                None,
                None,
            )),
            Value::Map(test_param_map_with_ui_metadata(
                "filter_cutoff",
                1,
                1000.0,
                20.0,
                20000.0,
                Some("filter"),
                None,
                None,
            )),
            Value::Map(test_param_map("mix", 2, 0.35, 0.0, 1.0)),
            Value::Map(test_param_map("enabled", 3, 1.0, 0.0, 1.0)),
        ]);
        let layout = editor.widget_layout().expect("metadata param grid layout");
        assert_finite_layout_tree(&layout);
        let mut layout_summaries = Vec::new();
        collect_layout_node_summaries(&layout, &mut layout_summaries);
        for (debug_name, control_name) in [
            ("fx-param-group-osc", "fx-param-compact-knob-osc_gain"),
            (
                "fx-param-group-filter",
                "fx-param-compact-knob-filter_cutoff",
            ),
            ("fx-param-group-misc", "fx-param-compact-knob-mix"),
        ] {
            let panel = find_layout_node_by_debug_name(&layout, debug_name)
                .unwrap_or_else(|| panic!("{debug_name}; layout={layout_summaries:#?}"));
            assert_eq!(
                panel.props.get("background-color").cloned(),
                Some(Value::Keyword("instrument-group-bg".to_string())),
                "{debug_name} should use the softer group background rather than the dark control background"
            );
            let group_label =
                find_layout_node_by_text(panel, debug_name.trim_start_matches("fx-param-group-"))
                    .unwrap_or_else(|| {
                        panic!("group label inside {debug_name}; layout={layout_summaries:#?}")
                    });
            assert!(
                group_label.rect.col > panel.rect.col + 0.25,
                "{debug_name} label should be inset from the panel edge, panel={:?} label={:?}",
                panel.rect,
                group_label.rect
            );
            let control =
                find_layout_node_by_debug_name(panel, control_name).unwrap_or_else(|| {
                    panic!("{control_name} inside {debug_name}; layout={layout_summaries:#?}")
                });
            let knob =
                find_layout_node_by_widget_type(control, "knob-number").unwrap_or_else(|| {
                    panic!("knob-number inside {control_name}; layout={layout_summaries:#?}")
                });
            assert!(
                panel.rect.width > 1.0
                    && panel.rect.width <= 13.0
                    && panel.rect.height > 1.0
                    && panel.rect.height <= 3.1
                    && control.rect.width > 1.0
                    && control.rect.height > 1.0
                    && knob.rect.width > 1.0
                    && knob.rect.height > 1.0,
                "{debug_name} and {control_name} should have compact visible measured rects, panel={:?} control={:?} knob={:?}",
                panel.rect,
                control.rect,
                knob.rect
            );
        }
        assert_eq!(
            count_widget_type(&layout, "knob-number"),
            3,
            "metadata grouped numeric params should render as compact knob rows"
        );
    }

    #[test]
    fn param_grid_group_rows_fit_control_count_width() {
        let editor = param_grid_test_editor(vec![
            Value::Map(test_param_map_with_ui_metadata(
                "filter_env",
                0,
                1.0,
                0.0,
                1.0,
                Some("filter"),
                None,
                None,
            )),
            Value::Map(test_param_map_with_ui_metadata(
                "hires",
                1,
                2.0,
                0.0,
                4.0,
                Some("filter"),
                None,
                None,
            )),
            Value::Map(test_param_map_with_ui_metadata(
                "hicut",
                2,
                100.0,
                0.0,
                127.0,
                Some("filter"),
                None,
                None,
            )),
            Value::Map(test_param_map_with_ui_metadata(
                "res",
                3,
                2.24,
                0.0,
                8.0,
                Some("filter"),
                None,
                None,
            )),
            Value::Map(test_param_map_with_ui_metadata(
                "cutoff",
                4,
                7037.0,
                20.0,
                20000.0,
                Some("filter"),
                None,
                None,
            )),
            Value::Map(test_param_map("gain", 5, 0.5, 0.0, 1.0)),
        ]);
        let layout = editor.widget_layout().expect("fit-width row layout");
        assert_finite_layout_tree(&layout);
        let filter_panel = find_layout_node_by_debug_name(&layout, "fx-param-group-filter")
            .expect("filter group panel");
        let misc_panel =
            find_layout_node_by_debug_name(&layout, "fx-param-group-misc").expect("misc panel");
        assert!(
            filter_panel.rect.width > 38.0 && filter_panel.rect.width < 40.0,
            "five-control group row should fit its controls instead of using a wide default, got {:?}",
            filter_panel.rect
        );
        assert!(
            misc_panel.rect.width > 12.0 && misc_panel.rect.width <= 13.0,
            "one-control misc row should use the compact minimum width, got {:?}",
            misc_panel.rect
        );
    }

    #[test]
    fn param_grid_group_rows_wrap_to_at_most_two_control_rows() {
        let mut params = Vec::new();
        for idx in 0..9 {
            let name = format!("wide_{idx}");
            params.push(Value::Map(test_param_map_with_ui_metadata(
                &name,
                idx,
                idx as f64,
                0.0,
                10.0,
                Some("wide"),
                None,
                None,
            )));
        }
        let editor = param_grid_test_editor(params);
        let layout = editor.widget_layout().expect("two-row group layout");
        assert_finite_layout_tree(&layout);
        let panel =
            find_layout_node_by_debug_name(&layout, "fx-param-group-wide").expect("wide group");
        assert!(
            panel.rect.height > 5.4 && panel.rect.height < 5.9,
            "nine controls should wrap into two larger knob rows, not three or more, got {:?}",
            panel.rect
        );
        assert!(
            panel.rect.width > 38.0 && panel.rect.width < 40.0,
            "nine controls should use the widest five-control row width, got {:?}",
            panel.rect
        );
    }

    #[test]
    fn param_grid_packs_group_panels_into_three_row_columns() {
        let editor = param_grid_test_editor(vec![
            Value::Map(test_param_map_with_ui_metadata(
                "filter_cutoff",
                0,
                500.0,
                20.0,
                20000.0,
                Some("filter"),
                None,
                None,
            )),
            Value::Map(test_param_map_with_ui_metadata(
                "fm_amount",
                1,
                2.0,
                0.0,
                4.0,
                Some("fm"),
                None,
                None,
            )),
            Value::Map(test_param_map_with_ui_metadata(
                "osc_level",
                2,
                0.7,
                0.0,
                1.0,
                Some("osc"),
                None,
                None,
            )),
            Value::Map(test_param_map("gain", 3, 0.5, 0.0, 1.0)),
        ]);
        let layout = editor.widget_layout().expect("three-row panel layout");
        assert_finite_layout_tree(&layout);
        let filter_panel =
            find_layout_node_by_debug_name(&layout, "fx-param-group-filter").expect("filter panel");
        let fm_panel =
            find_layout_node_by_debug_name(&layout, "fx-param-group-fm").expect("fm panel");
        let osc_panel =
            find_layout_node_by_debug_name(&layout, "fx-param-group-osc").expect("osc panel");
        let misc_panel =
            find_layout_node_by_debug_name(&layout, "fx-param-group-misc").expect("misc panel");
        assert!(
            (filter_panel.rect.col - fm_panel.rect.col).abs() < 0.1
                && fm_panel.rect.row > filter_panel.rect.row,
            "first two panels should stack in the first column, filter={:?} fm={:?}",
            filter_panel.rect,
            fm_panel.rect
        );
        assert!(
            (filter_panel.rect.col - osc_panel.rect.col).abs() < 0.1
                && osc_panel.rect.row > fm_panel.rect.row,
            "third panel should stay in the first column, fm={:?} osc={:?}",
            fm_panel.rect,
            osc_panel.rect
        );
        assert!(
            misc_panel.rect.col > filter_panel.rect.col + filter_panel.rect.width
                && (misc_panel.rect.row - filter_panel.rect.row).abs() < 0.1,
            "fourth panel should start a new column, filter={:?} misc={:?}",
            filter_panel.rect,
            misc_panel.rect
        );
    }

    #[test]
    fn custom_audio_effect_fallback_groups_metadata_params_without_envelope_ui() {
        let editor = custom_audio_fx_body_test_editor(vec![
            Value::Map(test_param_map_with_ui_metadata(
                "drive",
                0,
                0.4,
                0.0,
                1.0,
                Some("tone"),
                None,
                None,
            )),
            Value::Map(test_param_map_with_ui_metadata(
                "cutoff",
                1,
                1200.0,
                20.0,
                20000.0,
                Some("filter"),
                None,
                None,
            )),
            Value::Map(test_param_map_with_ui_metadata(
                "res",
                2,
                0.35,
                0.0,
                1.0,
                Some("filter"),
                None,
                None,
            )),
            Value::Map(test_param_map("mix", 3, 0.5, 0.0, 1.0)),
            Value::Map(test_param_map("enabled", 4, 1.0, 0.0, 1.0)),
        ]);
        let layout = editor
            .widget_layout()
            .expect("custom audio effect metadata fallback layout");
        assert_finite_layout_tree(&layout);
        let mut layout_summaries = Vec::new();
        collect_layout_node_summaries(&layout, &mut layout_summaries);
        for (debug_name, control_name) in [
            ("fx-param-group-tone", "fx-param-compact-knob-drive"),
            ("fx-param-group-filter", "fx-param-compact-knob-cutoff"),
            ("fx-param-group-misc", "fx-param-compact-knob-mix"),
        ] {
            let panel = find_layout_node_by_debug_name(&layout, debug_name)
                .unwrap_or_else(|| panic!("{debug_name}; layout={layout_summaries:#?}"));
            let control =
                find_layout_node_by_debug_name(panel, control_name).unwrap_or_else(|| {
                    panic!("{control_name} inside {debug_name}; layout={layout_summaries:#?}")
                });
            assert_eq!(
                panel.props.get("background-color").cloned(),
                Some(Value::Keyword("instrument-group-bg".to_string())),
                "{debug_name} should use the same subtle grouped panel surface in custom effects"
            );
            assert!(
                panel.rect.width > 1.0
                    && panel.rect.height > 1.0
                    && control.rect.width > 1.0
                    && control.rect.height > 1.0,
                "{debug_name} and {control_name} should have visible measured rects, panel={:?} control={:?}",
                panel.rect,
                control.rect
            );
        }
        assert_eq!(
            count_widget_type(&layout, "adsr-editor"),
            0,
            "custom effect metadata grouping should not introduce envelope UI"
        );
        assert!(
            find_layout_node_by_debug_name(&layout, "custom-audio-fx-wrapper").is_none(),
            "custom effect without ui.lisp should use the metadata fallback"
        );
    }

    #[test]
    fn param_grid_complete_adsr_metadata_renders_editor_and_updates_roles() {
        let mut editor = param_grid_test_editor(vec![
            Value::Map(test_param_map_with_ui_metadata(
                "amp_attack",
                0,
                5.0,
                1.0,
                1000.0,
                Some("amp"),
                Some("amp_env"),
                Some("attack"),
            )),
            Value::Map(test_param_map_with_ui_metadata(
                "amp_decay",
                1,
                120.0,
                1.0,
                2000.0,
                Some("amp"),
                Some("amp_env"),
                Some("decay"),
            )),
            Value::Map(test_param_map_with_ui_metadata(
                "amp_sustain",
                2,
                0.7,
                0.0,
                1.0,
                Some("amp"),
                Some("amp_env"),
                Some("sustain"),
            )),
            Value::Map(test_param_map_with_ui_metadata(
                "amp_release",
                3,
                120.0,
                1.0,
                3000.0,
                Some("amp"),
                Some("amp_env"),
                Some("release"),
            )),
            Value::Map(test_param_map_with_ui_metadata(
                "amp_level",
                4,
                0.8,
                0.0,
                1.0,
                Some("amp"),
                None,
                None,
            )),
            Value::Map(test_param_map("enabled", 5, 1.0, 0.0, 1.0)),
        ]);
        let layout = editor.widget_layout().expect("complete ADSR grid layout");
        assert_finite_layout_tree(&layout);
        assert_eq!(
            count_widget_type(&layout, "adsr-editor"),
            1,
            "complete envelope should render exactly one adsr-editor"
        );
        assert!(
            find_layout_node_by_text(&layout, "amp_attac").is_none()
                && find_layout_node_by_text(&layout, "amp_decay").is_none()
                && find_layout_node_by_text(&layout, "amp_susta").is_none()
                && find_layout_node_by_text(&layout, "amp_relea").is_none(),
            "complete envelope role params should be consumed from standalone rows"
        );
        let env_editor = find_layout_node_by_widget_type(&layout, "adsr-editor")
            .expect("complete envelope adsr-editor");
        let env_panel = find_layout_node_by_debug_name(&layout, "fx-param-env-amp_env")
            .expect("complete envelope panel");
        assert!(
            env_panel.rect.width > 8.0
                && env_panel.rect.height > 8.0
                && env_editor.rect.width > 8.0
                && env_editor.rect.height > 2.0
                && env_editor.rect.height < 4.0,
            "ADSR panel should fill the fallback UI height while keeping a compact visible editor, panel={:?} editor={:?}",
            env_panel.rect,
            env_editor.rect
        );
        let env_label = find_layout_node_by_text(&layout, "amp_env")
            .expect("selected envelope label should identify the rendered ADSR");
        assert!(
            env_label.rect.width > 1.0 && env_label.rect.height > 0.2,
            "selected envelope label should be visible, got {:?}",
            env_label.rect
        );
        for debug_name in [
            "fx-param-adsr-number-atk",
            "fx-param-adsr-number-dec",
            "fx-param-adsr-number-sus",
            "fx-param-adsr-number-rel",
        ] {
            let number = find_layout_node_by_debug_name(&layout, debug_name)
                .unwrap_or_else(|| panic!("{debug_name} should render below ADSR editor"));
            assert!(
                number.rect.width > 1.0 && number.rect.height > 0.5,
                "{debug_name} should have a visible measured rect, got {:?}",
                number.rect
            );
        }
        let callback = env_editor
            .props
            .get("on-change")
            .cloned()
            .expect("adsr-editor on-change");
        editor
            .runtime_mut()
            .invoke(
                callback,
                vec![Value::Map(HashMap::from([
                    (
                        "attack".to_string(),
                        Rc::new(RefCell::new(Value::Number(11.0))),
                    ),
                    (
                        "decay".to_string(),
                        Rc::new(RefCell::new(Value::Number(220.0))),
                    ),
                    (
                        "sustain".to_string(),
                        Rc::new(RefCell::new(Value::Number(0.42))),
                    ),
                    (
                        "release".to_string(),
                        Rc::new(RefCell::new(Value::Number(330.0))),
                    ),
                ]))],
            )
            .expect("invoke ADSR metadata on-change");
        let commands = editor.drain_host_commands();
        assert_eq!(commands.len(), 4, "commands={commands:?}");
        for (command, (expected_idx, expected_value)) in
            commands
                .iter()
                .zip([(0.0, 11.0), (1.0, 220.0), (2.0, 0.42), (3.0, 330.0)])
        {
            match command {
                eseqlisp::host::HostCommand::Custom { name, payload } => {
                    assert_eq!(name, "set-effect-param");
                    let Value::Map(payload) = payload else {
                        panic!("set-effect-param payload should be a dict: {payload:?}");
                    };
                    assert_eq!(
                        payload.get("slot-idx").map(|value| value.borrow().clone()),
                        Some(Value::Number(0.0))
                    );
                    assert_eq!(
                        payload.get("param-idx").map(|value| value.borrow().clone()),
                        Some(Value::Number(expected_idx))
                    );
                    assert_eq!(
                        payload.get("value").map(|value| value.borrow().clone()),
                        Some(Value::Number(expected_value))
                    );
                }
                other => panic!("expected set-effect-param host command, got {other:?}"),
            }
        }
    }

    #[test]
    fn param_grid_multiple_complete_envelopes_shows_selected_group_envelope_only() {
        let mut editor = param_grid_test_editor(vec![
            Value::Map(test_param_map_with_ui_metadata(
                "filter_attack",
                0,
                0.0,
                0.0,
                1000.0,
                Some("filter"),
                Some("filter_env"),
                Some("attack"),
            )),
            Value::Map(test_param_map_with_ui_metadata(
                "filter_decay",
                1,
                42.0,
                0.0,
                2000.0,
                Some("filter"),
                Some("filter_env"),
                Some("decay"),
            )),
            Value::Map(test_param_map_with_ui_metadata(
                "filter_sustain",
                2,
                0.0,
                0.0,
                1.0,
                Some("filter"),
                Some("filter_env"),
                Some("sustain"),
            )),
            Value::Map(test_param_map_with_ui_metadata(
                "filter_release",
                3,
                550.0,
                0.0,
                5000.0,
                Some("filter"),
                Some("filter_env"),
                Some("release"),
            )),
            Value::Map(test_param_map_with_ui_metadata(
                "filter_cutoff",
                4,
                900.0,
                20.0,
                20000.0,
                Some("filter"),
                None,
                None,
            )),
            Value::Map(test_param_map_with_ui_metadata(
                "filter_resonance",
                5,
                0.45,
                0.0,
                1.0,
                Some("filter"),
                None,
                None,
            )),
            Value::Map(test_param_map_with_ui_metadata(
                "amp_attack",
                6,
                5.0,
                1.0,
                1000.0,
                Some("amp"),
                Some("amp_env"),
                Some("attack"),
            )),
            Value::Map(test_param_map_with_ui_metadata(
                "amp_decay",
                7,
                120.0,
                1.0,
                2000.0,
                Some("amp"),
                Some("amp_env"),
                Some("decay"),
            )),
            Value::Map(test_param_map_with_ui_metadata(
                "amp_sustain",
                8,
                0.8,
                0.0,
                1.0,
                Some("amp"),
                Some("amp_env"),
                Some("sustain"),
            )),
            Value::Map(test_param_map_with_ui_metadata(
                "amp_release",
                9,
                180.0,
                1.0,
                5000.0,
                Some("amp"),
                Some("amp_env"),
                Some("release"),
            )),
            Value::Map(test_param_map_with_ui_metadata(
                "amp_level",
                10,
                0.8,
                0.0,
                1.0,
                Some("amp"),
                None,
                None,
            )),
            Value::Map(test_param_map("gain", 11, 0.5, 0.0, 1.0)),
        ]);

        let layout = editor.widget_layout().expect("multi envelope grid layout");
        assert_finite_layout_tree(&layout);
        assert_eq!(
            count_widget_type(&layout, "adsr-editor"),
            1,
            "only one metadata envelope should render at a time"
        );
        assert!(
            find_layout_node_by_debug_name(&layout, "fx-param-env-filter_env").is_none(),
            "non-default envelope should not be selected initially"
        );
        assert!(
            find_layout_node_by_debug_name(&layout, "fx-param-env-amp_env").is_some(),
            "amp envelope should be selected by default even when amp is not the first group"
        );
        assert!(
            find_layout_node_by_debug_name(&layout, "fx-param-compact-knob-filter_cutoff")
                .is_some()
                && find_layout_node_by_debug_name(
                    &layout,
                    "fx-param-compact-knob-filter_resonance"
                )
                .is_some()
                && find_layout_node_by_debug_name(&layout, "fx-param-compact-knob-amp_level")
                    .is_some()
                && find_layout_node_by_debug_name(&layout, "fx-param-compact-knob-gain").is_some(),
            "group panels should keep non-envelope controls visible"
        );
        assert!(
            find_layout_node_by_debug_name(&layout, "fx-param-compact-knob-filter_attack")
                .is_none()
                && find_layout_node_by_debug_name(&layout, "fx-param-compact-knob-amp_attack")
                    .is_none(),
            "envelope role params should be consumed by the shared selected ADSR"
        );

        let filter_panel = find_layout_node_by_debug_name(&layout, "fx-param-group-filter")
            .expect("filter group panel");
        let callback = filter_panel
            .props
            .get("on-click")
            .cloned()
            .expect("filter group panel click callback");
        editor
            .runtime_mut()
            .invoke(callback, vec![Value::Bool(false)])
            .expect("select filter group");
        editor.refresh_runtime_side_effects();
        let layout = editor
            .widget_layout()
            .expect("multi envelope grid layout after selecting filter");
        assert_eq!(
            count_widget_type(&layout, "adsr-editor"),
            1,
            "selecting another group should still render exactly one envelope"
        );
        assert!(
            find_layout_node_by_debug_name(&layout, "fx-param-env-filter_env").is_some(),
            "selected filter group envelope should render"
        );
        assert!(
            find_layout_node_by_debug_name(&layout, "fx-param-env-amp_env").is_none(),
            "default amp envelope should be hidden after selecting filter"
        );

        let misc_panel =
            find_layout_node_by_debug_name(&layout, "fx-param-group-misc").expect("misc panel");
        let callback = misc_panel
            .props
            .get("on-click")
            .cloned()
            .expect("misc panel click callback");
        editor
            .runtime_mut()
            .invoke(callback, vec![Value::Bool(false)])
            .expect("select misc group");
        editor.refresh_runtime_side_effects();
        let layout = editor
            .widget_layout()
            .expect("multi envelope grid layout after selecting misc");
        assert!(
            find_layout_node_by_debug_name(&layout, "fx-param-env-filter_env").is_none(),
            "clicking a non-envelope panel should leave the group envelope"
        );
        assert!(
            find_layout_node_by_debug_name(&layout, "fx-param-env-amp_env").is_some(),
            "clicking a non-envelope panel should return to the default amp envelope"
        );
    }

    #[test]
    fn param_grid_envelope_only_default_amp_group_hides_empty_panel() {
        let editor = param_grid_test_editor(vec![
            Value::Map(test_param_map_with_ui_metadata(
                "attack",
                0,
                5.0,
                0.0,
                1000.0,
                Some("amp"),
                Some("amp-env"),
                Some("attack"),
            )),
            Value::Map(test_param_map_with_ui_metadata(
                "decay",
                1,
                120.0,
                1.0,
                2000.0,
                Some("amp"),
                Some("amp-env"),
                Some("decay"),
            )),
            Value::Map(test_param_map_with_ui_metadata(
                "sustain",
                2,
                0.8,
                0.0,
                1.0,
                Some("amp"),
                Some("amp-env"),
                Some("sustain"),
            )),
            Value::Map(test_param_map_with_ui_metadata(
                "release",
                3,
                180.0,
                1.0,
                5000.0,
                Some("amp"),
                Some("amp-env"),
                Some("release"),
            )),
            Value::Map(test_param_map("gain", 4, 0.5, 0.0, 1.0)),
        ]);
        let layout = editor
            .widget_layout()
            .expect("envelope-only amp default layout");
        assert_finite_layout_tree(&layout);
        assert_eq!(
            count_widget_type(&layout, "adsr-editor"),
            1,
            "complete amp envelope should render as the default selected ADSR"
        );
        assert!(
            find_layout_node_by_debug_name(&layout, "fx-param-env-amp-env").is_some(),
            "default amp envelope should render"
        );
        assert!(
            find_layout_node_by_debug_name(&layout, "fx-param-group-amp").is_none(),
            "envelope-only amp group should not render an empty control panel"
        );
        assert!(
            find_layout_node_by_debug_name(&layout, "fx-param-group-misc").is_some()
                && find_layout_node_by_debug_name(&layout, "fx-param-compact-knob-gain").is_some(),
            "ungrouped controls should still render in the misc row"
        );
    }

    #[test]
    fn param_grid_incomplete_envelope_metadata_falls_back_to_rows() {
        let editor = param_grid_test_editor(vec![
            Value::Map(test_param_map_with_ui_metadata(
                "amp_attack",
                0,
                5.0,
                1.0,
                1000.0,
                Some("amp"),
                Some("amp_env"),
                Some("attack"),
            )),
            Value::Map(test_param_map_with_ui_metadata(
                "amp_decay",
                1,
                120.0,
                1.0,
                2000.0,
                Some("amp"),
                Some("amp_env"),
                Some("decay"),
            )),
            Value::Map(test_param_map_with_ui_metadata(
                "amp_sustain",
                2,
                0.7,
                0.0,
                1.0,
                Some("amp"),
                Some("amp_env"),
                Some("sustain"),
            )),
        ]);
        let layout = editor
            .widget_layout()
            .expect("incomplete envelope grid layout");
        assert_finite_layout_tree(&layout);
        assert_eq!(
            count_widget_type(&layout, "adsr-editor"),
            0,
            "incomplete envelope should not render an adsr-editor"
        );
        let mut layout_summaries = Vec::new();
        collect_layout_node_summaries(&layout, &mut layout_summaries);
        for control_name in [
            "fx-param-compact-knob-amp_attack",
            "fx-param-compact-knob-amp_decay",
            "fx-param-compact-knob-amp_sustain",
        ] {
            let node = find_layout_node_by_debug_name(&layout, control_name)
                .unwrap_or_else(|| panic!("{control_name}; layout={layout_summaries:#?}"));
            assert!(
                node.rect.width > 1.0 && node.rect.height > 1.0,
                "incomplete envelope role {control_name} should render as a compact control, got {:?}",
                node.rect
            );
        }
        assert_eq!(
            count_widget_type(&layout, "knob-number"),
            3,
            "incomplete envelope role params should render as compact knobs"
        );
    }

    #[test]
    fn custom_instrument_ui_still_wins_over_metadata_fallback() {
        let mut editor = eseqlisp::Editor::new(Runtime::new(), eseqlisp::EditorConfig::default());
        let mut inst = test_instrument_map();
        inst.insert(
            "synth".to_string(),
            Rc::new(RefCell::new(test_list(vec![Value::Map(
                test_param_map_with_ui_metadata(
                    "amp_attack",
                    0,
                    5.0,
                    1.0,
                    1000.0,
                    Some("amp"),
                    Some("amp_env"),
                    Some("attack"),
                ),
            )]))),
        );
        editor
            .runtime_mut()
            .register_reactive("TEST", vec![("inst", Value::Map(inst))], true);
        load_param_grid_test_lisp(&mut editor);
        editor
            .runtime_mut()
            .eval_str(
                r#"
                (def custom-instrument-synth-ui (inst)
                  (label "custom metadata ui" :font-size 10 :bg :transparent))
                (load "metal-seq-fx/panel-bodies.lisp")
                (effect-buffer "*custom-metadata-ui-test*" (instrument-synth-panel-body TEST.inst))
                "#,
            )
            .expect("load custom metadata ui test");
        editor.refresh_runtime_side_effects();
        if let Some(status) = editor.runtime_mut().take_status_message() {
            panic!("custom metadata ui test status after refresh: {status}");
        }
        let buffer_id = editor
            .buffers
            .iter()
            .find(|buffer| buffer.name == "*custom-metadata-ui-test*")
            .expect("custom metadata ui test buffer")
            .id;
        editor.set_active_buffer(buffer_id);
        editor.set_layout_viewport(80, 12);
        let layout = editor.widget_layout().expect("custom metadata ui layout");
        assert!(
            find_layout_node_by_debug_name(&layout, "custom-synth-wrapper").is_some(),
            "custom wrapper should render instead of fallback"
        );
        assert!(
            find_layout_node_by_text(&layout, "custom metadata ui").is_some(),
            "custom UI body should be visible"
        );
        assert!(
            find_layout_node_by_debug_name(&layout, "fallback-synth-wrapper").is_none()
                && find_layout_node_by_debug_name(&layout, "fx-param-group-").is_none(),
            "metadata fallback should not render when custom UI is present"
        );
    }

    #[test]
    fn metal_seq_fx_custom_audio_effect_ui_lays_out_body_content() {
        let src = std::fs::read_to_string("metal-seq-fx.lisp").expect("read fx lisp");
        let modum_ui =
            std::fs::read_to_string("effects/MODUM_DELAY/ui.lisp").expect("read MODUM_DELAY ui");
        let custom_audio_ui_source = build_custom_audio_fx_ui_source_with_overlay(Some((
            "MODUM_DELAY".to_string(),
            "effects/MODUM_DELAY/ui.lisp".to_string(),
            modum_ui,
        )));

        let mut editor = eseqlisp::Editor::new(Runtime::new(), eseqlisp::EditorConfig::default());
        editor.runtime_mut().register_reactive(
            "SEQ",
            vec![
                ("num-tracks", Value::Number(1.0)),
                ("compiling", Value::Bool(false)),
                ("delete-target-version", Value::Number(0.0)),
                (
                    "available-effects",
                    test_list(vec![Value::String("MODUM_DELAY".to_string())]),
                ),
                ("available-builtin-effects", test_list(vec![])),
                ("available-midi-effects", test_list(vec![])),
                (
                    "bus-names",
                    test_list(vec![Value::String("Mix".to_string())]),
                ),
                (
                    "effects",
                    test_list(vec![Value::Map(test_fx_map(
                        "MODUM_DELAY",
                        0,
                        test_modum_delay_params(),
                    ))]),
                ),
                ("midi-effects", test_list(vec![])),
                (
                    "instrument-panel",
                    test_list(vec![Value::Map(test_instrument_map())]),
                ),
                ("bus-effects", test_list(vec![test_list(vec![])])),
            ],
            true,
        );
        editor
            .runtime_mut()
            .eval_str(
                r#"
                (def selected-bus-name () "Mix")
                (def seq-has-selection? () false)
                (def sbrowser-editor-name "")
                (defmacro aqua-slider-material () `(material :color (rgba 0.15 0.15 0.88 1.0)))
                (def custom-instrument-synth-ui (inst) false)
                (def custom-midi-fx-ui (fx) false)
                (defstate selected-bus -1)
                "#,
            )
            .expect("install fx test helpers");
        register_test_delete_target_natives(&mut editor, 1);
        editor
            .runtime_mut()
            .eval_str(&custom_audio_ui_source)
            .expect("load initial custom audio FX UI");
        editor.runtime_mut().eval_str(&src).expect("load fx lisp");
        editor
            .runtime_mut()
            .eval_str(&custom_audio_ui_source)
            .expect("load custom audio FX UI");
        editor.refresh_runtime_side_effects();
        if let Some(status) = editor.runtime_mut().take_status_message() {
            panic!("MODUM_DELAY fx lisp status after refresh: {status}");
        }
        let fx_id = editor
            .buffers
            .iter()
            .find(|buffer| buffer.name == "*fx*")
            .expect("fx lisp should create the *fx* buffer")
            .id;
        editor.set_active_buffer(fx_id);
        editor.set_layout_viewport(120, 18);
        let layout = editor.widget_layout().expect("MODUM_DELAY fx layout");
        assert!(
            layout_contains_debug_name(&layout, "audio-fx-panel-root-0-MODUM_DELAY"),
            "layout should contain the MODUM_DELAY panel"
        );
        let mut layout_summaries = Vec::new();
        collect_layout_node_summaries(&layout, &mut layout_summaries);
        let delays = find_layout_node_by_text(&layout, "DELAYS")
            .unwrap_or_else(|| panic!("DELAYS label; layout={layout_summaries:#?}"));
        let cut = find_layout_node_by_text(&layout, "cut")
            .unwrap_or_else(|| panic!("cut label; layout={layout_summaries:#?}"));
        assert!(
            delays.rect.width > 1.0 && cut.rect.width > 1.0,
            "custom audio FX UI should have visible body labels, delays={:?} cut={:?}",
            delays.rect,
            cut.rect
        );
    }

    #[test]
    fn metal_seq_fx_sidechain_custom_ui_renders_route_dropdown() {
        let src = std::fs::read_to_string("metal-seq-fx.lisp").expect("read fx lisp");
        let custom_audio_ui_source = build_custom_audio_fx_ui_source_with_overlay(None);

        let mut editor = eseqlisp::Editor::new(Runtime::new(), eseqlisp::EditorConfig::default());
        editor.runtime_mut().register_reactive(
            "SEQ",
            vec![
                ("num-tracks", Value::Number(2.0)),
                ("compiling", Value::Bool(false)),
                ("delete-target-version", Value::Number(0.0)),
                (
                    "available-effects",
                    test_list(vec![Value::String("sidechain".to_string())]),
                ),
                ("available-builtin-effects", test_list(vec![])),
                ("available-midi-effects", test_list(vec![])),
                (
                    "bus-names",
                    test_list(vec![Value::String("Mix".to_string())]),
                ),
                (
                    "effects",
                    test_list(vec![Value::Map(test_fx_map(
                        "sidechain",
                        0,
                        vec![
                            Value::Map(test_param_map("threshold", 0, -20.0, -80.0, -2.0)),
                            Value::Map(test_param_map("ratio", 1, 10.0, 1.0, 20.0)),
                            Value::Map(test_enum_param_map(
                                "sidechain signal",
                                2,
                                1.0,
                                vec!["off", "kick", "snare"],
                            )),
                        ],
                    ))]),
                ),
                ("midi-effects", test_list(vec![])),
                (
                    "instrument-panel",
                    test_list(vec![Value::Map(test_instrument_map())]),
                ),
                ("bus-effects", test_list(vec![test_list(vec![])])),
            ],
            true,
        );
        editor
            .runtime_mut()
            .eval_str(
                r#"
                (def selected-bus-name () "Mix")
                (def seq-has-selection? () false)
                (def sbrowser-editor-name "")
                (defmacro aqua-slider-material () `(material :color (rgba 0.15 0.15 0.88 1.0)))
                (def custom-instrument-synth-ui (inst) false)
                (def custom-midi-fx-ui (fx) false)
                (defstate selected-bus -1)
                "#,
            )
            .expect("install fx test helpers");
        register_test_delete_target_natives(&mut editor, 2);
        editor
            .runtime_mut()
            .eval_str(&custom_audio_ui_source)
            .expect("load initial custom audio FX UI");
        editor.runtime_mut().eval_str(&src).expect("load fx lisp");
        editor
            .runtime_mut()
            .eval_str(&custom_audio_ui_source)
            .expect("load custom audio FX UI");
        editor.refresh_runtime_side_effects();
        if let Some(status) = editor.runtime_mut().take_status_message() {
            panic!("sidechain fx lisp status after refresh: {status}");
        }
        let fx_id = editor
            .buffers
            .iter()
            .find(|buffer| buffer.name == "*fx*")
            .expect("fx lisp should create the *fx* buffer")
            .id;
        editor.set_active_buffer(fx_id);
        editor.set_layout_viewport(120, 18);
        let layout = editor.widget_layout().expect("sidechain fx layout");
        let mut layout_summaries = Vec::new();
        collect_layout_node_summaries(&layout, &mut layout_summaries);
        let selector = find_layout_node_by_debug_name(&layout, "sidechain-route-selector")
            .unwrap_or_else(|| panic!("sidechain selector; layout={layout_summaries:#?}"));
        assert!(
            selector.rect.width > 1.0 && selector.rect.height > 1.0,
            "sidechain selector should have a finite visible rect: {:?}",
            selector.rect
        );
        let dropdown = find_layout_node_by_widget_type(selector, "dropdown").unwrap_or_else(|| {
            panic!("sidechain selector should contain dropdown: {layout_summaries:#?}")
        });
        assert!(
            dropdown.rect.width > 1.0 && dropdown.rect.height > 0.5,
            "sidechain dropdown should have a finite visible rect: {:?}",
            dropdown.rect
        );

        let callback = dropdown
            .props
            .get("on-change")
            .cloned()
            .expect("sidechain dropdown on-change");
        editor
            .runtime_mut()
            .invoke(callback, vec![Value::String("snare".to_string())])
            .expect("invoke sidechain dropdown on-change");

        let commands = editor.drain_host_commands();
        assert_eq!(commands.len(), 1, "commands={commands:?}");
        match &commands[0] {
            eseqlisp::host::HostCommand::Custom { name, payload } => {
                assert_eq!(name, "set-effect-param-option");
                let Value::Map(payload) = payload else {
                    panic!("set-effect-param-option payload should be a dict: {payload:?}");
                };
                assert_eq!(
                    payload.get("slot-idx").map(|value| value.borrow().clone()),
                    Some(Value::Number(0.0)),
                    "sidechain callback must target the rendered effect slot"
                );
                assert_eq!(
                    payload.get("param-idx").map(|value| value.borrow().clone()),
                    Some(Value::Number(2.0)),
                    "sidechain callback must target the sidechain selector parameter"
                );
                assert_eq!(
                    payload.get("label").map(|value| value.borrow().clone()),
                    Some(Value::String("snare".to_string())),
                    "sidechain callback must pass the selected track label"
                );
            }
            other => panic!("expected set-effect-param-option host command, got {other:?}"),
        }
    }

    #[test]
    fn metal_seq_fx_dimension_d_custom_ui_fits_below_header() {
        let src = std::fs::read_to_string("metal-seq-fx.lisp").expect("read fx lisp");
        let dimension_ui = std::fs::read_to_string("effects/dimension-d-chorus/ui.lisp")
            .expect("read dimension-d-chorus ui");
        let custom_audio_ui_source = build_custom_audio_fx_ui_source_with_overlay(Some((
            "dimension-d-chorus".to_string(),
            "effects/dimension-d-chorus/ui.lisp".to_string(),
            dimension_ui,
        )));

        let mut editor = eseqlisp::Editor::new(Runtime::new(), eseqlisp::EditorConfig::default());
        editor.runtime_mut().register_reactive(
            "SEQ",
            vec![
                ("num-tracks", Value::Number(1.0)),
                ("compiling", Value::Bool(false)),
                ("delete-target-version", Value::Number(0.0)),
                (
                    "available-effects",
                    test_list(vec![Value::String("dimension-d-chorus".to_string())]),
                ),
                ("available-builtin-effects", test_list(vec![])),
                ("available-midi-effects", test_list(vec![])),
                (
                    "bus-names",
                    test_list(vec![Value::String("Mix".to_string())]),
                ),
                (
                    "effects",
                    test_list(vec![Value::Map(test_fx_map(
                        "dimension-d-chorus",
                        0,
                        test_dimension_d_params(),
                    ))]),
                ),
                ("midi-effects", test_list(vec![])),
                (
                    "instrument-panel",
                    test_list(vec![Value::Map(test_instrument_map())]),
                ),
                ("bus-effects", test_list(vec![test_list(vec![])])),
            ],
            true,
        );
        editor
            .runtime_mut()
            .eval_str(
                r#"
                (def selected-bus-name () "Mix")
                (def seq-has-selection? () false)
                (def sbrowser-editor-name "")
                (defmacro aqua-slider-material () `(material :color (rgba 0.15 0.15 0.88 1.0)))
                (def custom-instrument-synth-ui (inst) false)
                (def custom-midi-fx-ui (fx) false)
                (defstate selected-bus -1)
                "#,
            )
            .expect("install fx test helpers");
        register_test_delete_target_natives(&mut editor, 1);
        editor
            .runtime_mut()
            .eval_str(&custom_audio_ui_source)
            .expect("load initial custom audio FX UI");
        editor.runtime_mut().eval_str(&src).expect("load fx lisp");
        editor
            .runtime_mut()
            .eval_str(&custom_audio_ui_source)
            .expect("load custom audio FX UI");
        editor.refresh_runtime_side_effects();
        if let Some(status) = editor.runtime_mut().take_status_message() {
            panic!("dimension-d-chorus fx lisp status after refresh: {status}");
        }
        let fx_id = editor
            .buffers
            .iter()
            .find(|buffer| buffer.name == "*fx*")
            .expect("fx lisp should create the *fx* buffer")
            .id;
        editor.set_active_buffer(fx_id);
        editor.set_layout_viewport(120, 18);
        let layout = editor
            .widget_layout()
            .expect("dimension-d-chorus fx layout");

        let mut layout_summaries = Vec::new();
        collect_layout_node_summaries(&layout, &mut layout_summaries);
        let panel =
            find_layout_node_by_debug_name(&layout, "audio-fx-panel-root-0-dimension-d-chorus")
                .unwrap_or_else(|| panic!("dimension panel; layout={layout_summaries:#?}"));
        let header = find_layout_node_by_debug_name(&layout, "audio-fx-panel-header")
            .unwrap_or_else(|| panic!("dimension header; layout={layout_summaries:#?}"));
        let title = find_layout_node_by_text(&layout, "dimension-d-chorus")
            .unwrap_or_else(|| panic!("dimension title; layout={layout_summaries:#?}"));
        let motion = find_layout_node_by_text(&layout, "MOTION")
            .unwrap_or_else(|| panic!("MOTION label; layout={layout_summaries:#?}"));
        let output = find_layout_node_by_text(&layout, "OUTPUT")
            .unwrap_or_else(|| panic!("OUTPUT label; layout={layout_summaries:#?}"));

        assert!(
            find_layout_node_by_text(&layout, "4 taps").is_none(),
            "dimension-d-chorus should omit the non-control VOICE readout"
        );
        assert!(
            title.rect.row >= header.rect.row
                && title.rect.row < header.rect.row + header.rect.height,
            "effect title should be measured inside the header, header={:?} title={:?}",
            header.rect,
            title.rect
        );
        assert!(
            motion.rect.row >= header.rect.row + header.rect.height,
            "custom effect body should start below the fixed effect header, header={:?} motion={:?}",
            header.rect,
            motion.rect
        );
        assert!(
            output.rect.row > motion.rect.row,
            "OUTPUT block should be below MOTION, motion={:?} output={:?}",
            motion.rect,
            output.rect
        );
        assert!(
            output.rect.row + 3.5 <= panel.rect.row + panel.rect.height,
            "dimension custom UI should fit within the fixed effect panel, panel={:?} output={:?}",
            panel.rect,
            output.rect
        );
    }

    #[test]
    fn custom_audio_fx_callbacks_keep_their_rendered_effect_scope() {
        let src = std::fs::read_to_string("metal-seq-fx.lisp").expect("read fx lisp");
        let custom_audio_ui_source = build_custom_audio_fx_ui_source_with_overlay(None);

        let mut editor = eseqlisp::Editor::new(Runtime::new(), eseqlisp::EditorConfig::default());
        editor.runtime_mut().register_reactive(
            "SEQ",
            vec![
                ("num-tracks", Value::Number(1.0)),
                ("compiling", Value::Bool(false)),
                ("delete-target-version", Value::Number(0.0)),
                (
                    "available-effects",
                    test_list(vec![
                        Value::String("dimension-d-chorus".to_string()),
                        Value::String("lexilush".to_string()),
                    ]),
                ),
                ("available-builtin-effects", test_list(vec![])),
                ("available-midi-effects", test_list(vec![])),
                (
                    "bus-names",
                    test_list(vec![Value::String("Mix".to_string())]),
                ),
                (
                    "effects",
                    test_list(vec![
                        Value::Map(test_fx_map(
                            "dimension-d-chorus",
                            0,
                            test_dimension_d_params(),
                        )),
                        Value::Map(test_fx_map("lexilush", 1, test_lexilush_params())),
                    ]),
                ),
                ("midi-effects", test_list(vec![])),
                (
                    "instrument-panel",
                    test_list(vec![Value::Map(test_instrument_map())]),
                ),
                ("bus-effects", test_list(vec![test_list(vec![])])),
            ],
            true,
        );
        editor
            .runtime_mut()
            .eval_str(
                r#"
                (def selected-bus-name () "Mix")
                (def seq-has-selection? () false)
                (def sbrowser-editor-name "")
                (defmacro aqua-slider-material () `(material :color (rgba 0.15 0.15 0.88 1.0)))
                (def custom-instrument-synth-ui (inst) false)
                (def custom-midi-fx-ui (fx) false)
                (defstate selected-bus -1)
                "#,
            )
            .expect("install fx test helpers");
        register_test_delete_target_natives(&mut editor, 1);
        editor
            .runtime_mut()
            .eval_str(&custom_audio_ui_source)
            .expect("load initial custom audio FX UI");
        editor.runtime_mut().eval_str(&src).expect("load fx lisp");
        editor
            .runtime_mut()
            .eval_str(&custom_audio_ui_source)
            .expect("load custom audio FX UI");
        editor.refresh_runtime_side_effects();
        if let Some(status) = editor.runtime_mut().take_status_message() {
            panic!("custom audio FX lisp status after refresh: {status}");
        }

        let fx_id = editor
            .buffers
            .iter()
            .find(|buffer| buffer.name == "*fx*")
            .expect("fx lisp should create the *fx* buffer")
            .id;
        editor.set_active_buffer(fx_id);
        editor.set_layout_viewport(120, 18);
        let layout = editor.widget_layout().expect("custom audio fx layout");
        let mut layout_summaries = Vec::new();
        collect_layout_node_summaries(&layout, &mut layout_summaries);
        let dimension_base = find_layout_node_by_stable_key(
            &layout,
            "custom-ui-lego-knob-dimension-d-chorus-slot-0-base",
        )
        .unwrap_or_else(|| panic!("dimension base knob; layout={layout_summaries:#?}"));
        assert!(
            find_layout_node_by_stable_key(&layout, "custom-ui-lego-num-lexilush-slot-1-damping")
                .is_some(),
            "lexilush damping control should also be rendered; layout={layout_summaries:#?}"
        );
        let callback = dimension_base
            .props
            .get("on-change")
            .cloned()
            .expect("dimension base on-change");
        editor
            .runtime_mut()
            .invoke(callback, vec![Value::Number(14.0)])
            .expect("invoke dimension base on-change");

        let commands = editor.drain_host_commands();
        assert_eq!(commands.len(), 1, "commands={commands:?}");
        match &commands[0] {
            eseqlisp::host::HostCommand::Custom { name, payload } => {
                assert_eq!(name, "set-effect-param");
                let Value::Map(payload) = payload else {
                    panic!("set-effect-param payload should be a dict: {payload:?}");
                };
                assert_eq!(
                    payload.get("slot-idx").map(|value| value.borrow().clone()),
                    Some(Value::Number(0.0)),
                    "dimension base callback must target the first effect slot"
                );
                assert_eq!(
                    payload.get("param-idx").map(|value| value.borrow().clone()),
                    Some(Value::Number(2.0)),
                    "dimension base callback must target the base parameter"
                );
                assert_eq!(
                    payload.get("value").map(|value| value.borrow().clone()),
                    Some(Value::Number(14.0))
                );
            }
            other => panic!("expected set-effect-param host command, got {other:?}"),
        }
    }

    #[test]
    fn custom_instrument_callbacks_survive_audio_fx_render_scope() {
        let src = std::fs::read_to_string("metal-seq-fx.lisp").expect("read fx lisp");
        let custom_audio_ui_source = build_custom_audio_fx_ui_source_with_overlay(None);
        let custom_instrument_ui_source = build_custom_instrument_ui_source_with_overlay(Some((
            "test-instrument".to_string(),
            "instruments/test-instrument/ui.lisp".to_string(),
            r#"
            (defsynth-ui
              (ui-panel "SYNTH" 0
                (h-stack :gap 0.2
                  (ui-param-knob "cutoff" "cut"))))
            "#
            .to_string(),
        )));

        let mut editor = eseqlisp::Editor::new(Runtime::new(), eseqlisp::EditorConfig::default());
        editor.runtime_mut().register_reactive(
            "SEQ",
            vec![
                ("num-tracks", Value::Number(1.0)),
                ("compiling", Value::Bool(false)),
                (
                    "available-effects",
                    test_list(vec![
                        Value::String("dimension-d-chorus".to_string()),
                        Value::String("lexilush".to_string()),
                    ]),
                ),
                ("available-builtin-effects", test_list(vec![])),
                ("available-midi-effects", test_list(vec![])),
                (
                    "bus-names",
                    test_list(vec![Value::String("Mix".to_string())]),
                ),
                (
                    "effects",
                    test_list(vec![
                        Value::Map(test_fx_map(
                            "dimension-d-chorus",
                            0,
                            test_dimension_d_params(),
                        )),
                        Value::Map(test_fx_map("lexilush", 1, test_lexilush_params())),
                    ]),
                ),
                ("midi-effects", test_list(vec![])),
                (
                    "instrument-panel",
                    test_list(vec![Value::Map(test_instrument_map())]),
                ),
                ("bus-effects", test_list(vec![test_list(vec![])])),
            ],
            true,
        );
        editor
            .runtime_mut()
            .eval_str(
                r#"
                (def selected-bus-name () "Mix")
                (def seq-has-selection? () false)
                (def sbrowser-editor-name "")
                (defmacro aqua-slider-material () `(material :color (rgba 0.15 0.15 0.88 1.0)))
                (def custom-midi-fx-ui (fx) false)
                (defstate selected-bus -1)
                "#,
            )
            .expect("install fx test helpers");
        register_test_delete_target_natives(&mut editor, 1);
        editor
            .runtime_mut()
            .eval_str(&custom_instrument_ui_source)
            .expect("load initial custom instrument UI");
        editor
            .runtime_mut()
            .eval_str(&custom_audio_ui_source)
            .expect("load initial custom audio FX UI");
        editor.runtime_mut().eval_str(&src).expect("load fx lisp");
        editor
            .runtime_mut()
            .eval_str(&custom_instrument_ui_source)
            .expect("load custom instrument UI");
        editor
            .runtime_mut()
            .eval_str(&custom_audio_ui_source)
            .expect("load custom audio FX UI");
        editor.refresh_runtime_side_effects();
        if let Some(status) = editor.runtime_mut().take_status_message() {
            panic!("custom instrument with audio FX lisp status after refresh: {status}");
        }

        let fx_id = editor
            .buffers
            .iter()
            .find(|buffer| buffer.name == "*fx*")
            .expect("fx lisp should create the *fx* buffer")
            .id;
        editor.set_active_buffer(fx_id);
        editor.set_layout_viewport(120, 18);
        let layout = editor
            .widget_layout()
            .expect("custom instrument with audio fx layout");
        let mut layout_summaries = Vec::new();
        collect_layout_node_summaries(&layout, &mut layout_summaries);
        let cutoff =
            find_layout_node_by_stable_key(&layout, "custom-ui-knob-test-instrument-cutoff")
                .unwrap_or_else(|| panic!("instrument cutoff knob; layout={layout_summaries:#?}"));
        assert!(
            find_layout_node_by_stable_key(&layout, "custom-ui-lego-num-lexilush-slot-1-damping")
                .is_some(),
            "audio FX should render after the instrument and leave the render globals in audio scope"
        );
        let callback = cutoff
            .props
            .get("on-change")
            .cloned()
            .expect("instrument cutoff on-change");
        editor
            .runtime_mut()
            .invoke(callback, vec![Value::Number(0.75)])
            .expect("invoke instrument cutoff on-change");

        let commands = editor.drain_host_commands();
        assert_eq!(commands.len(), 1, "commands={commands:?}");
        match &commands[0] {
            eseqlisp::host::HostCommand::Custom { name, payload } => {
                assert_eq!(name, "set-instrument-param");
                let Value::Map(payload) = payload else {
                    panic!("set-instrument-param payload should be a dict: {payload:?}");
                };
                assert_eq!(
                    payload.get("param-idx").map(|value| value.borrow().clone()),
                    Some(Value::Number(0.0)),
                    "instrument callback must target the cutoff parameter"
                );
                assert_eq!(
                    payload.get("value").map(|value| value.borrow().clone()),
                    Some(Value::Number(0.75))
                );
            }
            other => panic!("expected set-instrument-param host command, got {other:?}"),
        }
    }

    #[test]
    fn custom_instrument_option_callbacks_capture_scope_before_audio_fx_render() {
        let src = std::fs::read_to_string("metal-seq-fx.lisp").expect("read fx lisp");
        let custom_audio_ui_source = build_custom_audio_fx_ui_source_with_overlay(None);
        let custom_instrument_ui_source = build_custom_instrument_ui_source_with_overlay(Some((
            "test-instrument".to_string(),
            "instruments/test-instrument/ui.lisp".to_string(),
            r#"
            (defsynth-ui
              (ui-lego-micro-option-s
                0 "voice" "drum" 6.0
                '("kick" "snare" "lo tom")
                (ui-accent-cyan)))
            "#
            .to_string(),
        )));

        let mut inst = test_instrument_map();
        inst.insert(
            "synth".to_string(),
            Rc::new(RefCell::new(test_list(vec![
                Value::Map(test_param_map("cutoff", 0, 0.5, 0.0, 1.0)),
                Value::Map(test_param_map("voice", 1, 1.0, 1.0, 11.0)),
            ]))),
        );

        let shimmerpitch_params = vec![
            Value::Map(test_param_map("shift", 0, 12.0, -24.0, 24.0)),
            Value::Map(test_param_map("fine", 1, 0.0, -50.0, 50.0)),
            Value::Map(test_param_map("window_ms", 2, 90.0, 25.0, 220.0)),
            Value::Map(test_param_map("delay_ms", 3, 360.0, 20.0, 1400.0)),
            Value::Map(test_param_map("feedback", 4, 0.42, 0.0, 0.88)),
            Value::Map(test_param_map("damping", 5, 8500.0, 900.0, 18000.0)),
            Value::Map(test_param_map("width", 6, 0.85, 0.0, 1.4)),
            Value::Map(test_param_map("shimmer", 7, 1.0, 0.0, 1.0)),
            Value::Map(test_param_map("mix", 8, 0.42, 0.0, 1.0)),
            Value::Map(test_param_map("output", 9, 1.0, 0.25, 2.0)),
        ];

        let mut editor = eseqlisp::Editor::new(Runtime::new(), eseqlisp::EditorConfig::default());
        editor.runtime_mut().register_reactive(
            "SEQ",
            vec![
                ("num-tracks", Value::Number(1.0)),
                ("compiling", Value::Bool(false)),
                (
                    "available-effects",
                    test_list(vec![Value::String("shimmerpitch".to_string())]),
                ),
                ("available-builtin-effects", test_list(vec![])),
                ("available-midi-effects", test_list(vec![])),
                (
                    "bus-names",
                    test_list(vec![Value::String("Mix".to_string())]),
                ),
                (
                    "effects",
                    test_list(vec![Value::Map(test_fx_map(
                        "shimmerpitch",
                        0,
                        shimmerpitch_params,
                    ))]),
                ),
                ("midi-effects", test_list(vec![])),
                ("instrument-panel", test_list(vec![Value::Map(inst)])),
                ("bus-effects", test_list(vec![test_list(vec![])])),
            ],
            true,
        );
        editor
            .runtime_mut()
            .eval_str(
                r#"
                (def selected-bus-name () "Mix")
                (def seq-has-selection? () true)
                (def sbrowser-editor-name "")
                (defmacro aqua-slider-material () `(material :color (rgba 0.15 0.15 0.88 1.0)))
                (def custom-midi-fx-ui (fx) false)
                (defstate selected-bus -1)
                "#,
            )
            .expect("install fx test helpers");
        register_test_delete_target_natives(&mut editor, 1);
        editor
            .runtime_mut()
            .eval_str(&custom_instrument_ui_source)
            .expect("load initial custom instrument UI");
        editor
            .runtime_mut()
            .eval_str(&custom_audio_ui_source)
            .expect("load initial custom audio FX UI");
        editor.runtime_mut().eval_str(&src).expect("load fx lisp");
        editor
            .runtime_mut()
            .eval_str(&custom_instrument_ui_source)
            .expect("load custom instrument UI");
        editor
            .runtime_mut()
            .eval_str(&custom_audio_ui_source)
            .expect("load custom audio FX UI");
        editor.refresh_runtime_side_effects();

        let fx_id = editor
            .buffers
            .iter()
            .find(|buffer| buffer.name == "*fx*")
            .expect("fx lisp should create the *fx* buffer")
            .id;
        editor.set_active_buffer(fx_id);
        editor.set_layout_viewport(120, 18);
        let layout = editor
            .widget_layout()
            .expect("custom instrument option with audio fx layout");
        let mut layout_summaries = Vec::new();
        collect_layout_node_summaries(&layout, &mut layout_summaries);
        let voice_subtree = find_layout_node_by_stable_key(
            &layout,
            "custom-ui-lego-micro-option-test-instrument-voice",
        )
        .unwrap_or_else(|| panic!("instrument voice dropdown; layout={layout_summaries:#?}"));
        assert!(
            voice_subtree.rect.width > 0.0 && voice_subtree.rect.height > 0.0,
            "voice dropdown subtree should have a finite visible rect: {:?}",
            voice_subtree.rect
        );
        assert!(
            find_layout_node_by_stable_key(
                &layout,
                "custom-ui-lego-knob-shimmerpitch-slot-0-shift"
            )
            .is_some(),
            "shimmerpitch should render after the instrument and leave the render globals in audio scope"
        );

        let dropdown =
            find_layout_node_by_widget_type(voice_subtree, "dropdown").unwrap_or_else(|| {
                panic!("instrument voice subtree should contain dropdown: {layout_summaries:#?}")
            });
        let callback = dropdown
            .props
            .get("on-change")
            .cloned()
            .expect("instrument voice dropdown on-change");
        editor
            .runtime_mut()
            .invoke(callback, vec![Value::String("snare".to_string())])
            .expect("invoke instrument voice dropdown on-change");

        let commands = editor.drain_host_commands();
        assert_eq!(commands.len(), 1, "commands={commands:?}");
        match &commands[0] {
            eseqlisp::host::HostCommand::Custom { name, payload } => {
                assert_eq!(name, "set-instrument-plock");
                let Value::Map(payload) = payload else {
                    panic!("set-instrument-plock payload should be a dict: {payload:?}");
                };
                assert_eq!(
                    payload.get("param-idx").map(|value| value.borrow().clone()),
                    Some(Value::Number(1.0)),
                    "instrument option callback must target the voice parameter"
                );
                assert_eq!(
                    payload.get("value").map(|value| value.borrow().clone()),
                    Some(Value::Number(2.0)),
                    "snare should map to the second 1-based voice option"
                );
            }
            other => panic!("expected set-instrument-plock host command, got {other:?}"),
        }
    }

    #[test]
    fn custom_ui_section_selection_is_scoped_per_rendered_ui() {
        let src = std::fs::read_to_string("metal-seq-fx.lisp").expect("read fx lisp");
        let custom_instrument_ui_source = build_custom_instrument_ui_source_with_overlay(Some((
            "test-instrument".to_string(),
            "instruments/test-instrument/ui.lisp".to_string(),
            r#"
            (defsynth-ui
              (ui-panel "SYNTH" 2
                (label
                  (if (= custom-ui-selected-section 2) "INST_SELECTED" "INST_UNSELECTED")
                  :font-size 10 :color :white :bg :transparent)))
            "#
            .to_string(),
        )));
        let custom_audio_ui_source = build_custom_audio_fx_ui_source_with_overlay(Some((
            "dimension-d-chorus".to_string(),
            "effects/dimension-d-chorus/ui.lisp".to_string(),
            r#"
            (defeffect-ui
              (ui-control-block-medium-s "MOTION" (ui-accent-cyan) 1
                (label
                  (if (= custom-ui-selected-section 1) "FX_SELECTED" "FX_UNSELECTED")
                  :font-size 10 :color :white :bg :transparent)))
            "#
            .to_string(),
        )));

        let mut editor = eseqlisp::Editor::new(Runtime::new(), eseqlisp::EditorConfig::default());
        editor.runtime_mut().register_reactive(
            "SEQ",
            vec![
                ("num-tracks", Value::Number(1.0)),
                ("compiling", Value::Bool(false)),
                (
                    "available-effects",
                    test_list(vec![Value::String("dimension-d-chorus".to_string())]),
                ),
                ("available-builtin-effects", test_list(vec![])),
                ("available-midi-effects", test_list(vec![])),
                (
                    "bus-names",
                    test_list(vec![Value::String("Mix".to_string())]),
                ),
                (
                    "effects",
                    test_list(vec![Value::Map(test_fx_map(
                        "dimension-d-chorus",
                        0,
                        test_dimension_d_params(),
                    ))]),
                ),
                ("midi-effects", test_list(vec![])),
                (
                    "instrument-panel",
                    test_list(vec![Value::Map(test_instrument_map())]),
                ),
                ("bus-effects", test_list(vec![test_list(vec![])])),
            ],
            true,
        );
        editor
            .runtime_mut()
            .eval_str(
                r#"
                (def selected-bus-name () "Mix")
                (def seq-has-selection? () false)
                (def sbrowser-editor-name "")
                (defmacro aqua-slider-material () `(material :color (rgba 0.15 0.15 0.88 1.0)))
                (def custom-midi-fx-ui (fx) false)
                (defstate selected-bus -1)
                "#,
            )
            .expect("install fx test helpers");
        register_test_delete_target_natives(&mut editor, 1);
        editor
            .runtime_mut()
            .eval_str(&custom_instrument_ui_source)
            .expect("load initial custom instrument UI");
        editor
            .runtime_mut()
            .eval_str(&custom_audio_ui_source)
            .expect("load initial custom audio FX UI");
        editor.runtime_mut().eval_str(&src).expect("load fx lisp");
        editor
            .runtime_mut()
            .eval_str(&custom_instrument_ui_source)
            .expect("load custom instrument UI");
        editor
            .runtime_mut()
            .eval_str(&custom_audio_ui_source)
            .expect("load custom audio FX UI");

        editor
            .runtime_mut()
            .eval_str(
                r#"
                (do
                  (custom-instrument-synth-ui (nth SEQ.instrument-panel 0))
                  (def test-instrument-section-click (ui-section-select-callback 2))
                  (custom-audio-fx-ui (nth SEQ.effects 0))
                  (def test-fx-section-click (ui-section-select-callback 1))
                  (test-instrument-section-click false)
                  (test-fx-section-click false))
                "#,
            )
            .expect("select custom UI sections");

        let instrument_tree = editor
            .runtime_mut()
            .eval_str("(custom-instrument-synth-ui (nth SEQ.instrument-panel 0))")
            .expect("render selected custom instrument UI")
            .expect("instrument UI value");
        let fx_tree = editor
            .runtime_mut()
            .eval_str("(custom-audio-fx-ui (nth SEQ.effects 0))")
            .expect("render selected custom FX UI")
            .expect("FX UI value");

        assert!(
            value_contains_string(&instrument_tree, "INST_SELECTED"),
            "instrument render should keep its own selected section: {instrument_tree:?}"
        );
        assert!(
            value_contains_string(&fx_tree, "FX_SELECTED"),
            "FX render should keep its own selected section: {fx_tree:?}"
        );
    }

    #[test]
    fn metal_seq_fx_lisp_lays_out_custom_instrument_ui_without_unbounded_width() {
        fn assert_finite_layout(node: &eseqlisp::layout::LayoutNode) {
            assert!(
                node.rect.width.is_finite()
                    && node.rect.height.is_finite()
                    && node.rect.col.is_finite()
                    && node.rect.row.is_finite()
                    && node.rect.width < 10_000.0
                    && node.rect.col.abs() < 10_000.0,
                "non-finite or runaway layout node: type={} rect=({:.2},{:.2},{:.2},{:.2})",
                node.widget_type,
                node.rect.row,
                node.rect.col,
                node.rect.width,
                node.rect.height
            );
            for child in &node.children {
                assert_finite_layout(child);
            }
        }

        let src = std::fs::read_to_string("metal-seq-fx.lisp").expect("read fx lisp");
        let custom_ui_source = build_custom_instrument_ui_source_with_overlay(Some((
            "test-instrument/".to_string(),
            "test/ui.lisp".to_string(),
            r#"
            (defsynth-ui
              (h-stack :width :fill :gap 0.5
                (v-stack :width 16.0 :gap 0.10
                  (ui-panel "CUSTOM_OK" 0
                    (h-stack :gap 0.25
                      (ui-param-knob "cutoff" "cut"))))
                (ui-adsr "amp" "amp_attack" "amp_decay" "amp_sustain" "amp_release")))
            "#
            .to_string(),
        )));

        let mut editor = eseqlisp::Editor::new(Runtime::new(), eseqlisp::EditorConfig::default());
        editor.set_layout_viewport(120, 18);
        editor.runtime_mut().register_reactive(
            "SEQ",
            vec![
                ("num-tracks", Value::Number(1.0)),
                ("compiling", Value::Bool(false)),
                ("available-effects", test_list(vec![])),
                ("available-builtin-effects", test_list(vec![])),
                ("available-midi-effects", test_list(vec![])),
                ("bus-names", test_list(vec![])),
                ("effects", test_list(vec![])),
                ("midi-effects", test_list(vec![])),
                (
                    "instrument-panel",
                    test_list(vec![Value::Map(test_instrument_map())]),
                ),
                ("bus-effects", test_list(vec![])),
            ],
            true,
        );
        editor
            .runtime_mut()
            .eval_str(
                r#"
                (def selected-bus-name () "Mix")
                (def seq-has-selection? () false)
                (def sbrowser-editor-name "")
                (defmacro aqua-slider-material () `(material :color (rgba 0.15 0.15 0.88 1.0)))
                (def custom-midi-fx-ui (fx) false)
                (def custom-audio-fx-ui (fx) false)
                (defstate selected-bus -1)
                "#,
            )
            .expect("install fx test helpers");
        register_test_delete_target_natives(&mut editor, 1);
        editor
            .runtime_mut()
            .eval_str(&custom_ui_source)
            .expect("load custom instrument ui");
        editor.runtime_mut().eval_str(&src).expect("load fx lisp");
        editor.refresh_runtime_side_effects();
        if let Some(status) = editor.runtime_mut().take_status_message() {
            panic!("custom instrument fx lisp status after refresh: {status}");
        }
        let fx_id = editor
            .buffers
            .iter()
            .find(|buffer| buffer.name == "*fx*")
            .expect("fx lisp should create the *fx* buffer")
            .id;
        editor.set_active_buffer(fx_id);
        editor.set_layout_viewport(120, 18);
        let layout = editor.widget_layout().expect("custom instrument fx layout");
        assert_finite_layout_tree(&layout);
        let tree = editor
            .active_buffer()
            .widget_tree
            .as_ref()
            .expect("fx tree");
        assert!(value_contains_string(tree, "CUSTOM_OK"));
    }

    #[test]
    fn metal_seq_fx_lisp_lays_out_inline_custom_instrument_mod_selector() {
        fn layout_has_double_click(node: &eseqlisp::layout::LayoutNode) -> bool {
            node.props.contains_key("on-double-click")
                || node.children.iter().any(layout_has_double_click)
        }
        fn assert_set_instrument_param(
            command: &eseqlisp::host::HostCommand,
            expected_idx: f64,
            expected_value: f64,
        ) {
            match command {
                eseqlisp::host::HostCommand::Custom { name, payload } => {
                    assert_eq!(name, "set-instrument-param");
                    let Value::Map(payload) = payload else {
                        panic!("set-instrument-param payload should be a dict: {payload:?}");
                    };
                    assert_eq!(
                        payload.get("param-idx").map(|value| value.borrow().clone()),
                        Some(Value::Number(expected_idx))
                    );
                    assert_eq!(
                        payload.get("value").map(|value| value.borrow().clone()),
                        Some(Value::Number(expected_value))
                    );
                }
                other => panic!("expected set-instrument-param host command, got {other:?}"),
            }
        }
        fn assert_knob_measured(node: &eseqlisp::layout::LayoutNode, context: &str) {
            assert!(
                node.rect.width > 0.0 && node.rect.height > 0.0,
                "{context} knob should have a nonzero measured rect: {:?}",
                node.rect
            );
        }
        #[cfg(target_os = "macos")]
        fn knob_metal_instance_modes(node: &eseqlisp::layout::LayoutNode) -> Vec<f32> {
            use eseqlisp::widget_render::MetalPrimitive;

            let primitives =
                eseqlisp::widget_render::widget_primitives_for_node(node, test_widget_viewport());
            primitives
                .iter()
                .filter_map(|primitive| match primitive {
                    MetalPrimitive::WidgetInstance {
                        widget_type,
                        instance,
                        ..
                    } if widget_type == "knob-number" => Some(instance.uniform_b[0]),
                    _ => None,
                })
                .collect::<Vec<_>>()
        }
        #[cfg(target_os = "macos")]
        fn knob_mod_range_instance_count(node: &eseqlisp::layout::LayoutNode) -> usize {
            use eseqlisp::widget_render::MetalPrimitive;

            eseqlisp::widget_render::widget_primitives_for_node(node, test_widget_viewport())
                .iter()
                .filter(|primitive| {
                    matches!(
                        primitive,
                        MetalPrimitive::WidgetInstance { widget_type, .. }
                            if widget_type == "knob-number-mod-range"
                    )
                })
                .count()
        }
        #[cfg(target_os = "macos")]
        fn test_widget_viewport() -> eseqlisp::widget_render::WidgetViewport {
            eseqlisp::widget_render::WidgetViewport {
                cell_w: 10.0,
                cell_h: 10.0,
                vp_w: 1200.0,
                vp_h: 180.0,
                time_seconds: 0.0,
                focused_widget_id: None,
                focused_branch: false,
                tile_content_rows: 18.0,
                scroll_top: 0.0,
                scroll_left: 0.0,
                inherited_hover: false,
            }
        }
        #[cfg(target_os = "macos")]
        fn knob_proportional_texts(node: &eseqlisp::layout::LayoutNode) -> Vec<String> {
            use eseqlisp::widget_render::MetalPrimitive;

            eseqlisp::widget_render::widget_primitives_for_node(node, test_widget_viewport())
                .iter()
                .filter_map(|primitive| match primitive {
                    MetalPrimitive::ProportionalText(text) => Some(text.text.clone()),
                    _ => None,
                })
                .collect::<Vec<_>>()
        }
        #[cfg(target_os = "macos")]
        fn assert_knob_base_metal_instance(node: &eseqlisp::layout::LayoutNode, context: &str) {
            let modes = knob_metal_instance_modes(node);
            assert!(
                modes.iter().any(|mode| *mode == 0.0),
                "{context} knob-number should emit a base knob Metal instance: {modes:?}"
            );
        }

        let src = std::fs::read_to_string("metal-seq-fx.lisp").expect("read fx lisp");
        fn lisp_def_slice<'a>(src: &'a str, start_pattern: &str, end_pattern: &str) -> &'a str {
            let start = src
                .find(start_pattern)
                .unwrap_or_else(|| panic!("source should contain {start_pattern}"));
            let end = src[start..]
                .find(end_pattern)
                .map(|offset| start + offset)
                .unwrap_or_else(|| panic!("{start_pattern} should be followed by {end_pattern}"));
            &src[start..end]
        }
        fn test_mod_target(
            source_idx: f64,
            depth_idx: f64,
            source_slot: f64,
            depth: f64,
            source_value_field: Option<&str>,
            depth_value_field: Option<&str>,
        ) -> Value {
            let mut target = HashMap::from([
                (
                    "source-idx".to_string(),
                    Rc::new(RefCell::new(Value::Number(source_idx))),
                ),
                (
                    "depth-idx".to_string(),
                    Rc::new(RefCell::new(Value::Number(depth_idx))),
                ),
                (
                    "source-slot".to_string(),
                    Rc::new(RefCell::new(Value::Number(source_slot))),
                ),
                (
                    "depth".to_string(),
                    Rc::new(RefCell::new(Value::Number(depth))),
                ),
                (
                    "depth-min".to_string(),
                    Rc::new(RefCell::new(Value::Number(-1.0))),
                ),
                (
                    "depth-max".to_string(),
                    Rc::new(RefCell::new(Value::Number(1.0))),
                ),
            ]);
            if let Some(field) = source_value_field {
                target.insert(
                    "source-value-field".to_string(),
                    Rc::new(RefCell::new(Value::String(field.to_string()))),
                );
            }
            if let Some(field) = depth_value_field {
                target.insert(
                    "depth-value-field".to_string(),
                    Rc::new(RefCell::new(Value::String(field.to_string()))),
                );
            }
            Value::Map(target)
        }
        let custom_ui_source = build_custom_instrument_ui_source_with_overlay(Some((
            "test-instrument".to_string(),
            "instruments/test-instrument/ui.lisp".to_string(),
            r#"
            (defsynth-ui
              (ui-panel "SYNTH" 0
                (h-stack :gap 0.2
                  (ui-lego-knob-s 0 "cutoff" "cut" 4.8 (ui-accent-blue) 2))))
            "#
            .to_string(),
        )));
        let mut cutoff = test_param_map("cutoff", 0, 0.5, 0.0, 1.0);
        cutoff.insert(
            "modulatable".to_string(),
            Rc::new(RefCell::new(Value::Bool(true))),
        );
        cutoff.insert(
            "mod-targets".to_string(),
            Rc::new(RefCell::new(test_list(vec![
                test_mod_target(10.0, 11.0, 1.0, 0.25, None, None),
                test_mod_target(
                    12.0,
                    13.0,
                    0.0,
                    0.0,
                    Some("test-mod-source-12"),
                    Some("test-mod-depth-13"),
                ),
            ]))),
        );

        let mut inst = test_instrument_map();
        inst.insert(
            "synth".to_string(),
            Rc::new(RefCell::new(test_list(vec![
                Value::Map(cutoff),
                Value::Map(test_param_map("gain", 1, 0.8, 0.0, 1.0)),
            ]))),
        );
        inst.insert(
            "modulators".to_string(),
            Rc::new(RefCell::new(test_list(vec![
                Value::Map(HashMap::from([
                    (
                        "slot".to_string(),
                        Rc::new(RefCell::new(Value::Number(1.0))),
                    ),
                    (
                        "label".to_string(),
                        Rc::new(RefCell::new(Value::String("Mod 1".to_string()))),
                    ),
                ])),
                Value::Map(HashMap::from([
                    (
                        "slot".to_string(),
                        Rc::new(RefCell::new(Value::Number(2.0))),
                    ),
                    (
                        "label".to_string(),
                        Rc::new(RefCell::new(Value::String("Mod 2".to_string()))),
                    ),
                ])),
            ]))),
        );
        let mut lfo_sync = test_param_map("sync", 31, 1.0, 0.0, 1.0);
        lfo_sync.insert(
            "value-field".to_string(),
            Rc::new(RefCell::new(Value::String("test-lfo-sync".to_string()))),
        );
        let mut lfo_division = test_param_map("division", 32, 0.0, 0.0, 1.0);
        lfo_division.insert(
            "text-value".to_string(),
            Rc::new(RefCell::new(Value::String("1/8".to_string()))),
        );
        lfo_division.insert(
            "options".to_string(),
            Rc::new(RefCell::new(test_list(vec![
                Value::String("1/8".to_string()),
                Value::String("1/16".to_string()),
            ]))),
        );
        let mut lfo_shape = test_param_map("shape", 33, 0.0, 0.0, 1.0);
        lfo_shape.insert(
            "text-value".to_string(),
            Rc::new(RefCell::new(Value::String("sine".to_string()))),
        );
        lfo_shape.insert(
            "options".to_string(),
            Rc::new(RefCell::new(test_list(vec![
                Value::String("sine".to_string()),
                Value::String("square".to_string()),
            ]))),
        );
        inst.insert(
            "sources".to_string(),
            Rc::new(RefCell::new(test_list(vec![
                Value::Map(HashMap::from([
                    (
                        "name".to_string(),
                        Rc::new(RefCell::new(Value::String("Mod 1".to_string()))),
                    ),
                    (
                        "slot".to_string(),
                        Rc::new(RefCell::new(Value::Number(1.0))),
                    ),
                    (
                        "source-param".to_string(),
                        Rc::new(RefCell::new(Value::Map(test_enum_param_map(
                            "type",
                            29,
                            1.0,
                            vec![
                                "off", "lfo", "env", "rand", "drift", "ext1", "ext2", "ext3",
                                "ext4",
                            ],
                        )))),
                    ),
                    (
                        "params".to_string(),
                        Rc::new(RefCell::new(test_list(vec![
                            Value::Map(test_param_map("rate", 30, 2.0, 0.1, 20.0)),
                            Value::Map(lfo_sync),
                            Value::Map(lfo_division),
                            Value::Map(lfo_shape),
                            Value::Map(test_param_map("pulse width", 34, 0.5, 0.0, 1.0)),
                        ]))),
                    ),
                ])),
                Value::Map(HashMap::from([
                    (
                        "name".to_string(),
                        Rc::new(RefCell::new(Value::String("Mod 2".to_string()))),
                    ),
                    (
                        "slot".to_string(),
                        Rc::new(RefCell::new(Value::Number(2.0))),
                    ),
                    (
                        "source-param".to_string(),
                        Rc::new(RefCell::new(Value::Map(test_enum_param_map(
                            "type",
                            19,
                            2.0,
                            vec![
                                "off", "lfo", "env", "rand", "drift", "ext1", "ext2", "ext3",
                                "ext4",
                            ],
                        )))),
                    ),
                    (
                        "params".to_string(),
                        Rc::new(RefCell::new(test_list(vec![
                            Value::Map(test_param_map("attack", 20, 5.0, 1.0, 5000.0)),
                            Value::Map(test_param_map("decay", 21, 120.0, 1.0, 5000.0)),
                            Value::Map(test_param_map("sustain", 22, 0.7, 0.0, 1.0)),
                            Value::Map(test_param_map("release", 23, 240.0, 1.0, 5000.0)),
                        ]))),
                    ),
                ])),
            ]))),
        );

        let mut editor = eseqlisp::Editor::new(Runtime::new(), eseqlisp::EditorConfig::default());
        editor.set_layout_viewport(120, 18);
        editor.runtime_mut().register_reactive(
            "SEQ",
            vec![
                ("num-tracks", Value::Number(1.0)),
                ("compiling", Value::Bool(false)),
                ("available-effects", test_list(vec![])),
                ("available-builtin-effects", test_list(vec![])),
                ("available-midi-effects", test_list(vec![])),
                ("bus-names", test_list(vec![])),
                ("effects", test_list(vec![])),
                ("midi-effects", test_list(vec![])),
                ("instrument-panel", test_list(vec![Value::Map(inst)])),
                ("test-lfo-sync", Value::Number(1.0)),
                ("test-mod-source-12", Value::Number(0.0)),
                ("test-mod-depth-13", Value::Number(0.0)),
                ("bus-effects", test_list(vec![])),
            ],
            true,
        );
        editor
            .runtime_mut()
            .eval_str(
                r#"
                (def selected-bus-name () "Mix")
                (def seq-has-selection? () false)
                (def sbrowser-editor-name "")
                (defmacro aqua-slider-material () `(material :color (rgba 0.15 0.15 0.88 1.0)))
                (def custom-midi-fx-ui (fx) false)
                (def custom-audio-fx-ui (fx) false)
                (defstate selected-bus -1)
                "#,
            )
            .expect("install fx test helpers");
        register_test_delete_target_natives(&mut editor, 1);
        editor
            .runtime_mut()
            .eval_str(&custom_ui_source)
            .expect("load custom instrument UI");
        editor.runtime_mut().eval_str(&src).expect("load fx lisp");
        editor
            .runtime_mut()
            .eval_str("(do (set! instrument-panel-tab 0) (set! instrument-mods-open false) (set! instrument-selected-mod-slot 1))")
            .expect("show custom synth panel");
        editor.refresh_runtime_side_effects();
        if let Some(status) = editor.runtime_mut().take_status_message() {
            panic!("custom synth fx lisp status after refresh: {status}");
        }
        let fx_id = editor
            .buffers
            .iter()
            .find(|buffer| buffer.name == "*fx*")
            .expect("fx lisp should create the *fx* buffer")
            .id;
        editor.set_active_buffer(fx_id);
        let synth_layout = editor.widget_layout().expect("custom synth layout");
        assert_finite_layout_tree(&synth_layout);
        let synth_knob = find_layout_node_by_widget_type(&synth_layout, "knob-number")
            .expect("custom synth panel should render a knob-number with mods closed");
        assert_knob_measured(synth_knob, "mods-closed");
        #[cfg(target_os = "macos")]
        assert_knob_base_metal_instance(synth_knob, "mods-closed");
        #[cfg(target_os = "macos")]
        assert!(
            knob_proportional_texts(synth_knob)
                .iter()
                .any(|text| text == "cut"),
            "mods-closed knob should emit its label/value text primitives: {:?}",
            knob_proportional_texts(synth_knob)
        );

        editor
            .runtime_mut()
            .eval_str("(defstate lower-panel-buffer \"*fx*\")")
            .expect("install lower panel state for mods toggle action");
        let grid_src = std::fs::read_to_string("metal-seq-grid.lisp").expect("read grid lisp");
        let effect_panels_src = std::fs::read_to_string("metal-seq-fx/effect-panels.lisp")
            .expect("read effect panels lisp");
        let toggle_action_src = lisp_def_slice(
            &effect_panels_src,
            "(def instrument-toggle-mods-view",
            "(def instrument-mods-toggle-button",
        );
        let grid_action_src = lisp_def_slice(
            &grid_src,
            "(def seq-show-fx-lower-panel",
            "(bind-key \"Tab\"",
        );
        editor
            .runtime_mut()
            .eval_str(toggle_action_src)
            .expect("load real instrument mods toggle action");
        editor
            .runtime_mut()
            .eval_str(grid_action_src)
            .expect("load real mods toggle action");
        let state = Arc::new(SequencerState::new(1, vec![]));
        let current_track = Arc::new(AtomicUsize::new(0));
        let selected_steps = Arc::new(Mutex::new(HashSet::new()));
        let step_clipboard = Arc::new(Mutex::new(None));
        assert!(
            handle_metal_command_shortcut(
                &mut editor,
                &crossterm::event::KeyEvent::new(
                    crossterm::event::KeyCode::Char('m'),
                    crossterm::event::KeyModifiers::SUPER,
                ),
                &state,
                &current_track,
                &selected_steps,
                &step_clipboard,
            ),
            "Cmd+M should invoke the mods toggle action and refresh the visible FX layout"
        );
        assert_eq!(
            editor
                .runtime_mut()
                .eval_str("instrument-mods-open")
                .unwrap(),
            Some(Value::Bool(true))
        );
        if let Some(status) = editor.runtime_mut().take_status_message() {
            panic!("inline mods fx lisp status after refresh: {status}");
        }
        editor.set_active_buffer(fx_id);
        let layout = editor.widget_layout().expect("inline mods layout");
        assert_finite_layout_tree(&layout);
        let selector = find_layout_node_by_debug_name(&layout, "instrument-mod-selector")
            .expect("inline mods selector should render");
        assert!(
            selector.rect.width > 0.0 && selector.rect.height > 0.0,
            "selector must have a nonzero rect: {:?}",
            selector.rect
        );
        assert!(find_layout_node_by_text(&layout, "Mod 1").is_some());
        let lfo_editor = find_layout_node_by_debug_name(&layout, "instrument-lfo-source-editor")
            .expect("selected LFO source editor should render");
        assert!(
            lfo_editor.rect.width > 0.0 && lfo_editor.rect.height > 0.0,
            "LFO source editor should have a visible measured rect: {:?}",
            lfo_editor.rect
        );
        assert!(
            find_layout_node_by_text(&layout, "ON").is_some(),
            "reactive LFO sync source button should resolve its value and render as ON"
        );
        let custom_knob_wrapper = find_layout_node_by_stable_key(
            &layout,
            "custom-ui-lego-knob-mod-test-instrument-cutoff",
        )
        .expect("custom lego controls should render their modulation wrapper");
        assert!(
            layout_has_double_click(&layout),
            "modulatable parameter wrapper should expose on-double-click"
        );

        let knob = find_layout_node_by_widget_type(custom_knob_wrapper, "knob-number")
            .expect("custom mod test should render a knob-number");
        assert_knob_measured(knob, "mods-open");
        for prop in ["mod-range-0-slot", "mod-range-0-depth"] {
            assert!(
                knob.props.contains_key(prop),
                "modulatable knob should expose modulation range metadata prop {prop}"
            );
        }
        assert!(
            knob.props.contains_key("base-value"),
            "modulatable knob should expose base value for range overlays"
        );
        #[cfg(target_os = "macos")]
        {
            assert_knob_base_metal_instance(knob, "mods-open");
            assert!(
                knob_mod_range_instance_count(knob) > 0,
                "knob-number should emit at least one dedicated modulation range Metal instance"
            );
            assert!(
                knob_proportional_texts(knob)
                    .iter()
                    .any(|text| text == "cut"),
                "mods-open knob should emit its label/value text primitives: {:?}",
                knob_proportional_texts(knob)
            );
        }
        let callback = knob
            .props
            .get("on-change")
            .cloned()
            .expect("modulatable knob should expose on-change");
        editor
            .runtime_mut()
            .invoke(callback.clone(), vec![Value::Number(0.5)])
            .expect("selected modulation lane depth edit");
        let commands = editor.drain_host_commands();
        assert_eq!(commands.len(), 1, "commands={commands:?}");
        assert_set_instrument_param(&commands[0], 11.0, 0.5);

        editor
            .runtime_mut()
            .eval_str("(set! instrument-selected-mod-slot 2)")
            .expect("select second modulator");
        editor
            .runtime_mut()
            .invoke(callback.clone(), vec![Value::Number(0.75)])
            .expect("empty modulation lane assignment and depth edit");
        let commands = editor.drain_host_commands();
        assert_eq!(commands.len(), 2, "commands={commands:?}");
        assert_set_instrument_param(&commands[0], 12.0, 2.0);
        assert_set_instrument_param(&commands[1], 13.0, 0.75);

        editor.set_active_buffer(fx_id);
        let env_layout = editor.widget_layout().expect("Mod 2 source editor layout");
        let env_editor = find_layout_node_by_widget_type(&env_layout, "adsr-editor")
            .expect("Mod 2 source editor should use an adsr-editor widget");
        assert!(
            env_editor.rect.width > 8.0 && env_editor.rect.height > 1.5,
            "Mod 2 adsr-editor should have a visible measured rect, got {:?}",
            env_editor.rect
        );
        let env_callback = env_editor
            .props
            .get("on-change")
            .cloned()
            .expect("Mod 2 adsr-editor should expose on-change");
        editor
            .runtime_mut()
            .invoke(
                env_callback,
                vec![Value::Map(HashMap::from([
                    (
                        "attack".to_string(),
                        Rc::new(RefCell::new(Value::Number(11.0))),
                    ),
                    (
                        "decay".to_string(),
                        Rc::new(RefCell::new(Value::Number(220.0))),
                    ),
                    (
                        "sustain".to_string(),
                        Rc::new(RefCell::new(Value::Number(0.42))),
                    ),
                    (
                        "release".to_string(),
                        Rc::new(RefCell::new(Value::Number(330.0))),
                    ),
                ]))],
            )
            .expect("edit Mod 2 source ADSR");
        let commands = editor.drain_host_commands();
        assert_eq!(commands.len(), 4, "commands={commands:?}");
        assert_set_instrument_param(&commands[0], 20.0, 11.0);
        assert_set_instrument_param(&commands[1], 21.0, 220.0);
        assert_set_instrument_param(&commands[2], 22.0, 0.42);
        assert_set_instrument_param(&commands[3], 23.0, 330.0);

        editor
            .runtime_mut()
            .set_reactive("SEQ", "test-mod-source-12", Value::Number(2.0));
        editor
            .runtime_mut()
            .set_reactive("SEQ", "test-mod-depth-13", Value::Number(0.75));
        editor
            .runtime_mut()
            .invoke(callback, vec![Value::Number(0.9)])
            .expect("claimed modulation lane stays live after reactive sync");
        let commands = editor.drain_host_commands();
        assert_eq!(commands.len(), 1, "commands={commands:?}");
        assert_set_instrument_param(&commands[0], 13.0, 0.9);
    }

    #[test]
    fn metal_seq_fx_lisp_lays_out_inline_sampler_mod_selector() {
        fn layout_has_double_click(node: &eseqlisp::layout::LayoutNode) -> bool {
            node.props.contains_key("on-double-click")
                || node.children.iter().any(layout_has_double_click)
        }

        fn assert_set_instrument_param(
            command: &eseqlisp::host::HostCommand,
            expected_idx: f64,
            expected_value: f64,
        ) {
            match command {
                eseqlisp::host::HostCommand::Custom { name, payload } => {
                    assert_eq!(name, "set-instrument-param");
                    let Value::Map(payload) = payload else {
                        panic!("set-instrument-param payload should be a dict: {payload:?}");
                    };
                    assert_eq!(
                        payload.get("param-idx").map(|value| value.borrow().clone()),
                        Some(Value::Number(expected_idx))
                    );
                    assert_eq!(
                        payload.get("value").map(|value| value.borrow().clone()),
                        Some(Value::Number(expected_value))
                    );
                }
                other => panic!("expected set-instrument-param host command, got {other:?}"),
            }
        }

        fn test_mod_target(source_idx: f64, depth_idx: f64, source_slot: f64, depth: f64) -> Value {
            Value::Map(HashMap::from([
                (
                    "source-idx".to_string(),
                    Rc::new(RefCell::new(Value::Number(source_idx))),
                ),
                (
                    "depth-idx".to_string(),
                    Rc::new(RefCell::new(Value::Number(depth_idx))),
                ),
                (
                    "source-slot".to_string(),
                    Rc::new(RefCell::new(Value::Number(source_slot))),
                ),
                (
                    "depth".to_string(),
                    Rc::new(RefCell::new(Value::Number(depth))),
                ),
                (
                    "depth-min".to_string(),
                    Rc::new(RefCell::new(Value::Number(-1.0))),
                ),
                (
                    "depth-max".to_string(),
                    Rc::new(RefCell::new(Value::Number(1.0))),
                ),
            ]))
        }

        let mut enabled = test_param_map("enabled", 4, 1.0, 0.0, 1.0);
        enabled.insert(
            "boolean".to_string(),
            Rc::new(RefCell::new(Value::Bool(true))),
        );
        let mut warp = test_param_map("warp", 9, 0.0, 0.0, 1.0);
        warp.insert(
            "boolean".to_string(),
            Rc::new(RefCell::new(Value::Bool(true))),
        );
        let mut speed = test_param_map("speed", 11, 1.0, -4.0, 4.0);
        speed.insert(
            "modulatable".to_string(),
            Rc::new(RefCell::new(Value::Bool(true))),
        );
        speed.insert(
            "mod-targets".to_string(),
            Rc::new(RefCell::new(test_list(vec![
                test_mod_target(20.0, 21.0, 2.0, 0.25),
                test_mod_target(22.0, 23.0, 0.0, 0.0),
            ]))),
        );

        let mut inst = test_instrument_map();
        inst.insert(
            "name".to_string(),
            Rc::new(RefCell::new(Value::String("sampler".to_string()))),
        );
        inst.insert(
            "display-name".to_string(),
            Rc::new(RefCell::new(Value::String("Sampler".to_string()))),
        );
        inst.insert(
            "type".to_string(),
            Rc::new(RefCell::new(Value::String("sampler".to_string()))),
        );
        inst.insert(
            "duration".to_string(),
            Rc::new(RefCell::new(Value::Number(1.0))),
        );
        inst.insert(
            "synth".to_string(),
            Rc::new(RefCell::new(test_list(vec![
                Value::Map(test_param_map("attack", 0, 0.0, 0.0, 500.0)),
                Value::Map(test_param_map("release", 1, 0.0, 0.0, 2000.0)),
                Value::Map(test_param_map("start", 2, 0.0, 0.0, 1.0)),
                Value::Map(test_param_map("end", 3, 1.0, 0.0, 1.0)),
                Value::Map(enabled),
                Value::Map(test_param_map("reverse", 5, 0.0, 0.0, 1.0)),
                Value::Map(test_param_map("loop", 6, 1.0, 0.0, 3.0)),
                Value::Map(test_param_map("xfade", 7, 0.0, 0.0, 250.0)),
                Value::Map(test_param_map("sr", 8, 44100.0, 2000.0, 44100.0)),
                Value::Map(warp),
                Value::Map(test_param_map("mode", 10, 0.0, 0.0, 0.0)),
                Value::Map(speed),
                Value::Map(test_param_map("scrub", 12, 0.0, -1.0, 1.0)),
                Value::Map(test_param_map("bpm", 13, 120.0, 20.0, 400.0)),
            ]))),
        );
        inst.insert("mod".to_string(), Rc::new(RefCell::new(test_list(vec![]))));
        inst.insert(
            "modulators".to_string(),
            Rc::new(RefCell::new(test_list(vec![
                Value::Map(HashMap::from([
                    (
                        "slot".to_string(),
                        Rc::new(RefCell::new(Value::Number(1.0))),
                    ),
                    (
                        "label".to_string(),
                        Rc::new(RefCell::new(Value::String("Mod 1".to_string()))),
                    ),
                ])),
                Value::Map(HashMap::from([
                    (
                        "slot".to_string(),
                        Rc::new(RefCell::new(Value::Number(2.0))),
                    ),
                    (
                        "label".to_string(),
                        Rc::new(RefCell::new(Value::String("Mod 2".to_string()))),
                    ),
                ])),
            ]))),
        );
        inst.insert(
            "sources".to_string(),
            Rc::new(RefCell::new(test_list(vec![
                Value::Map(HashMap::from([
                    (
                        "name".to_string(),
                        Rc::new(RefCell::new(Value::String("Mod 1".to_string()))),
                    ),
                    (
                        "slot".to_string(),
                        Rc::new(RefCell::new(Value::Number(1.0))),
                    ),
                    (
                        "source-param".to_string(),
                        Rc::new(RefCell::new(Value::Map(test_enum_param_map(
                            "type",
                            39,
                            1.0,
                            vec![
                                "off", "lfo", "env", "rand", "drift", "ext1", "ext2", "ext3",
                                "ext4",
                            ],
                        )))),
                    ),
                    (
                        "params".to_string(),
                        Rc::new(RefCell::new(test_list(vec![
                            Value::Map(test_param_map("rate", 40, 5.0, 0.05, 40.0)),
                            Value::Map({
                                let mut p = test_param_map("sync", 41, 0.0, 0.0, 1.0);
                                p.insert(
                                    "boolean".to_string(),
                                    Rc::new(RefCell::new(Value::Bool(true))),
                                );
                                p
                            }),
                            Value::Map({
                                let mut p = test_param_map("division", 42, 6.0, 0.0, 10.0);
                                p.insert(
                                    "text-value".to_string(),
                                    Rc::new(RefCell::new(Value::String("1/4".to_string()))),
                                );
                                p.insert(
                                    "options".to_string(),
                                    Rc::new(RefCell::new(test_list(vec![
                                        Value::String("1/8".to_string()),
                                        Value::String("1/4".to_string()),
                                    ]))),
                                );
                                p
                            }),
                            Value::Map({
                                let mut p = test_param_map("shape", 43, 2.0, 0.0, 3.0);
                                p.insert(
                                    "text-value".to_string(),
                                    Rc::new(RefCell::new(Value::String("triangle".to_string()))),
                                );
                                p.insert(
                                    "options".to_string(),
                                    Rc::new(RefCell::new(test_list(vec![
                                        Value::String("sine".to_string()),
                                        Value::String("triangle".to_string()),
                                    ]))),
                                );
                                p
                            }),
                            Value::Map(test_param_map("pulse width", 44, 0.5, 0.05, 0.95)),
                            Value::Map({
                                let mut p = test_param_map("retrigger", 45, 0.0, 0.0, 1.0);
                                p.insert(
                                    "boolean".to_string(),
                                    Rc::new(RefCell::new(Value::Bool(true))),
                                );
                                p
                            }),
                        ]))),
                    ),
                ])),
                Value::Map(HashMap::from([
                    (
                        "name".to_string(),
                        Rc::new(RefCell::new(Value::String("Mod 2".to_string()))),
                    ),
                    (
                        "slot".to_string(),
                        Rc::new(RefCell::new(Value::Number(2.0))),
                    ),
                    (
                        "source-param".to_string(),
                        Rc::new(RefCell::new(Value::Map(test_enum_param_map(
                            "type",
                            29,
                            2.0,
                            vec![
                                "off", "lfo", "env", "rand", "drift", "ext1", "ext2", "ext3",
                                "ext4",
                            ],
                        )))),
                    ),
                    (
                        "params".to_string(),
                        Rc::new(RefCell::new(test_list(vec![
                            Value::Map(test_param_map("attack", 30, 5.0, 1.0, 5000.0)),
                            Value::Map(test_param_map("decay", 31, 120.0, 1.0, 5000.0)),
                            Value::Map(test_param_map("sustain", 32, 0.7, 0.0, 1.0)),
                            Value::Map(test_param_map("release", 33, 240.0, 1.0, 5000.0)),
                        ]))),
                    ),
                ])),
            ]))),
        );

        let src = std::fs::read_to_string("metal-seq-fx.lisp").expect("read fx lisp");
        let mut editor = eseqlisp::Editor::new(Runtime::new(), eseqlisp::EditorConfig::default());
        editor.set_layout_viewport(160, 18);
        editor.runtime_mut().register_reactive(
            "SEQ",
            vec![
                ("num-tracks", Value::Number(1.0)),
                ("compiling", Value::Bool(false)),
                ("tp-gate", Value::Bool(false)),
                ("available-effects", test_list(vec![])),
                ("available-builtin-effects", test_list(vec![])),
                ("available-midi-effects", test_list(vec![])),
                ("bus-names", test_list(vec![])),
                ("effects", test_list(vec![])),
                ("midi-effects", test_list(vec![])),
                ("instrument-panel", test_list(vec![Value::Map(inst)])),
                ("sampler-playhead", Value::Number(0.0)),
                ("bus-effects", test_list(vec![])),
            ],
            true,
        );
        editor
            .runtime_mut()
            .eval_str(
                r#"
                (def selected-bus-name () "Mix")
                (def seq-has-selection? () false)
                (def sbrowser-editor-name "")
                (defmacro aqua-slider-material () `(material :color (rgba 0.15 0.15 0.88 1.0)))
                (def custom-instrument-synth-ui (inst) false)
                (def custom-midi-fx-ui (fx) false)
                (def custom-audio-fx-ui (fx) false)
                (defstate selected-bus -1)
                "#,
            )
            .expect("install fx test helpers");
        register_test_delete_target_natives(&mut editor, 1);
        editor.runtime_mut().eval_str(&src).expect("load fx lisp");
        editor
            .runtime_mut()
            .eval_str("(do (set! instrument-panel-tab 0) (set! instrument-mods-open true) (set! instrument-selected-mod-slot 2))")
            .expect("open sampler inline mods");
        editor.refresh_runtime_side_effects();
        if let Some(status) = editor.runtime_mut().take_status_message() {
            panic!("sampler fx lisp status after refresh: {status}");
        }

        let fx_id = editor
            .buffers
            .iter()
            .find(|buffer| buffer.name == "*fx*")
            .expect("fx lisp should create the *fx* buffer")
            .id;
        editor.set_active_buffer(fx_id);
        let layout = editor.widget_layout().expect("sampler inline mods layout");
        assert_finite_layout_tree(&layout);
        let mut layout_summaries = Vec::new();
        collect_layout_node_summaries(&layout, &mut layout_summaries);
        let selector = find_layout_node_by_debug_name(&layout, "instrument-mod-selector")
            .unwrap_or_else(|| {
                panic!("sampler inline mods selector should render; layout={layout_summaries:#?}")
            });
        assert!(
            selector.rect.width > 0.0 && selector.rect.height > 0.0,
            "selector must have a nonzero rect: {:?}",
            selector.rect
        );
        assert!(
            find_layout_node_by_debug_name(&layout, "sampler-mods-inline-body").is_some(),
            "sampler should use inline mods body"
        );
        assert!(
            find_layout_node_by_stable_key(&layout, "sampler-param-11-mod-wrapper").is_some(),
            "sampler speed control should render its modulation wrapper"
        );
        assert!(
            layout_has_double_click(&layout),
            "sampler modulatable parameter wrapper should expose on-double-click"
        );

        let speed_knob = find_layout_node_by_stable_key(&layout, "sampler-param-11-mod-depth")
            .and_then(|node| find_layout_node_by_widget_type(node, "knob-number"))
            .expect("sampler speed should render as a mod-depth knob");
        assert!(
            speed_knob.props.contains_key("base-value")
                && speed_knob.props.contains_key("mod-range-0-slot")
                && speed_knob.props.contains_key("mod-range-0-depth"),
            "sampler speed knob should expose mod metadata props"
        );
        assert!(
            speed_knob.props.contains_key("mod-range-1-slot")
                && speed_knob.props.contains_key("mod-range-1-depth"),
            "sampler speed knob should expose multiple modulation lanes"
        );
        let callback = speed_knob
            .props
            .get("on-change")
            .cloned()
            .expect("sampler speed knob should expose on-change");
        editor
            .runtime_mut()
            .invoke(callback.clone(), vec![Value::Number(0.75)])
            .expect("sampler speed depth edit");
        let commands = editor.drain_host_commands();
        assert_eq!(commands.len(), 1, "commands={commands:?}");
        assert_set_instrument_param(&commands[0], 21.0, 0.75);

        let env_editor = find_layout_node_by_widget_type(&layout, "adsr-editor")
            .expect("sampler Mod 2 source editor should use an adsr-editor widget");
        assert!(
            env_editor.rect.width > 8.0 && env_editor.rect.height > 1.5,
            "sampler Mod 2 adsr-editor should be visible, got {:?}",
            env_editor.rect
        );

        editor
            .runtime_mut()
            .eval_str("(set! instrument-selected-mod-slot 1)")
            .expect("select sampler LFO source editor");
        editor
            .runtime_mut()
            .invoke(callback, vec![Value::Number(0.5)])
            .expect("assign sampler speed second modulation lane");
        let commands = editor.drain_host_commands();
        assert_eq!(commands.len(), 2, "commands={commands:?}");
        assert_set_instrument_param(&commands[0], 22.0, 1.0);
        assert_set_instrument_param(&commands[1], 23.0, 0.5);
        editor.set_active_buffer(fx_id);
        let lfo_layout = editor.widget_layout().expect("sampler LFO source layout");
        assert_finite_layout_tree(&lfo_layout);
        let lfo_editor =
            find_layout_node_by_debug_name(&lfo_layout, "instrument-lfo-source-editor")
                .expect("sampler LFO source should use compact LFO editor");
        assert!(
            lfo_editor.rect.width > 0.0 && lfo_editor.rect.height > 0.0,
            "LFO source editor should be visible, got {:?}",
            lfo_editor.rect
        );
        let pulse_width =
            find_layout_node_by_debug_name(&lfo_layout, "instrument-source-compact-knob-pw")
                .and_then(|node| find_layout_node_by_widget_type(node, "knob-number"))
                .expect("LFO pulse width should render as a compact knob");
        assert!(
            pulse_width.rect.width <= 4.5 && pulse_width.rect.height > 0.0,
            "pulse width knob should be compact, got {:?}",
            pulse_width.rect
        );
        let retrigger =
            find_layout_node_by_debug_name(&lfo_layout, "instrument-lfo-retrigger-button")
                .and_then(|node| find_layout_node_by_widget_type(node, "button"))
                .expect("LFO retrigger should render as a button widget");
        assert!(
            retrigger.rect.width <= 4.5 && retrigger.rect.height > 0.0,
            "retrigger button should be compact, got {:?}",
            retrigger.rect
        );
    }

    #[test]
    fn metal_seq_fx_lisp_lays_out_agent_instrument_stub_skeleton() {
        let src = std::fs::read_to_string("metal-seq-fx.lisp").expect("read fx lisp");
        let custom_ui_source = build_custom_instrument_ui_source_with_overlay(Some((
            "agent-draft-1/".to_string(),
            "instruments/agent-draft-1/ui.lisp".to_string(),
            super::AGENT_INSTRUMENT_STUB_UI.to_string(),
        )));

        let mut inst = test_instrument_map();
        inst.insert(
            "name".to_string(),
            Rc::new(RefCell::new(Value::String("agent-draft-1/".to_string()))),
        );
        inst.insert(
            "display-name".to_string(),
            Rc::new(RefCell::new(Value::String("agent-draft-1".to_string()))),
        );

        let mut editor = eseqlisp::Editor::new(Runtime::new(), eseqlisp::EditorConfig::default());
        editor.set_layout_viewport(120, 18);
        editor.runtime_mut().register_reactive(
            "SEQ",
            vec![
                ("num-tracks", Value::Number(1.0)),
                ("compiling", Value::Bool(false)),
                ("available-effects", test_list(vec![])),
                ("available-builtin-effects", test_list(vec![])),
                ("available-midi-effects", test_list(vec![])),
                ("bus-names", test_list(vec![])),
                ("effects", test_list(vec![])),
                ("midi-effects", test_list(vec![])),
                ("instrument-panel", test_list(vec![Value::Map(inst)])),
                ("bus-effects", test_list(vec![])),
            ],
            true,
        );
        editor
            .runtime_mut()
            .eval_str(
                r#"
                (def selected-bus-name () "Mix")
                (def seq-has-selection? () false)
                (def sbrowser-editor-name "")
                (defmacro aqua-slider-material () `(material :color (rgba 0.15 0.15 0.88 1.0)))
                (def custom-midi-fx-ui (fx) false)
                (def custom-audio-fx-ui (fx) false)
                (defstate selected-bus -1)
                "#,
            )
            .expect("install fx test helpers");
        register_test_delete_target_natives(&mut editor, 1);
        editor
            .runtime_mut()
            .eval_str(&custom_ui_source)
            .expect("load agent stub custom instrument ui");
        editor.runtime_mut().eval_str(&src).expect("load fx lisp");
        editor.refresh_runtime_side_effects();
        if let Some(status) = editor.runtime_mut().take_status_message() {
            panic!("agent stub fx lisp status after refresh: {status}");
        }

        let fx_id = editor
            .buffers
            .iter()
            .find(|buffer| buffer.name == "*fx*")
            .expect("fx lisp should create the *fx* buffer")
            .id;
        editor.set_active_buffer(fx_id);
        let layout = editor.widget_layout().expect("agent stub fx layout");
        let body = find_layout_node_by_debug_name(&layout, "instrument-content-box")
            .expect("instrument panel content body should be present");
        let skeleton = find_layout_node_by_debug_name(&layout, "agent-instrument-stub-skeleton")
            .expect("agent stub skeleton should be present in the measured layout");
        assert!(
            skeleton.rect.width.is_finite()
                && skeleton.rect.height.is_finite()
                && skeleton.rect.width > 1.0
                && skeleton.rect.height > 1.0,
            "agent stub skeleton should have a finite visible rect, got {:?}",
            skeleton.rect
        );
        assert!(
            skeleton.rect.height >= body.rect.height - 1.0,
            "agent stub skeleton should fill the instrument panel body height, body={:?} skeleton={:?}",
            body.rect,
            skeleton.rect
        );
        assert!(
            find_layout_node_by_text(&layout, "base_note").is_none(),
            "agent stub should not fall back to the generic base_note instrument panel"
        );
    }

    #[test]
    fn metal_seq_fx_lisp_lays_out_real_korg1_custom_instrument_ui() {
        fn assert_finite_layout(node: &eseqlisp::layout::LayoutNode) {
            assert!(
                node.rect.width.is_finite()
                    && node.rect.height.is_finite()
                    && node.rect.col.is_finite()
                    && node.rect.row.is_finite()
                    && node.rect.width < 10_000.0
                    && node.rect.col.abs() < 10_000.0,
                "non-finite or runaway layout node: type={} rect=({:.2},{:.2},{:.2},{:.2})",
                node.widget_type,
                node.rect.row,
                node.rect.col,
                node.rect.width,
                node.rect.height
            );
            for child in &node.children {
                assert_finite_layout(child);
            }
        }

        let src = std::fs::read_to_string("metal-seq-fx.lisp").expect("read fx lisp");
        let korg1_ui =
            std::fs::read_to_string("instruments/bass/korg1/ui.lisp").expect("read korg1 ui");
        let initial_custom_ui_source = build_custom_instrument_ui_source_with_overlay(None);
        let korg1_custom_ui_source = build_custom_instrument_ui_source_with_overlay(Some((
            "korg1/".to_string(),
            "instruments/bass/korg1/ui.lisp".to_string(),
            korg1_ui,
        )));

        let mut editor = eseqlisp::Editor::new(Runtime::new(), eseqlisp::EditorConfig::default());
        editor.set_layout_viewport(160, 20);
        editor.runtime_mut().register_reactive(
            "SEQ",
            vec![
                ("num-tracks", Value::Number(1.0)),
                ("compiling", Value::Bool(false)),
                ("available-effects", test_list(vec![])),
                ("available-builtin-effects", test_list(vec![])),
                ("available-midi-effects", test_list(vec![])),
                ("bus-names", test_list(vec![])),
                ("effects", test_list(vec![])),
                ("midi-effects", test_list(vec![])),
                (
                    "instrument-panel",
                    test_list(vec![Value::Map(korg1_test_instrument_map())]),
                ),
                ("bus-effects", test_list(vec![])),
            ],
            true,
        );
        editor
            .runtime_mut()
            .eval_str(
                r#"
                (def selected-bus-name () "Mix")
                (def seq-has-selection? () false)
                (def sbrowser-editor-name "")
                (defmacro aqua-slider-material () `(material :color (rgba 0.15 0.15 0.88 1.0)))
                (def custom-midi-fx-ui (fx) false)
                (def custom-audio-fx-ui (fx) false)
                (defstate selected-bus -1)
                "#,
            )
            .expect("install fx test helpers");
        register_test_delete_target_natives(&mut editor, 1);
        editor
            .runtime_mut()
            .eval_str(&initial_custom_ui_source)
            .expect("load initial empty custom instrument ui");
        editor.runtime_mut().eval_str(&src).expect("load fx lisp");
        editor
            .runtime_mut()
            .eval_str(&korg1_custom_ui_source)
            .expect("load korg1 custom instrument ui");
        editor.refresh_runtime_side_effects();
        if let Some(status) = editor.runtime_mut().take_status_message() {
            panic!("real korg1 custom instrument fx lisp status after refresh: {status}");
        }

        let fx_id = editor
            .buffers
            .iter()
            .find(|buffer| buffer.name == "*fx*")
            .expect("fx lisp should create the *fx* buffer")
            .id;
        editor.set_active_buffer(fx_id);
        editor.set_layout_viewport(160, 20);
        let layout = editor.widget_layout().expect("korg1 fx layout");
        assert_finite_layout_tree(&layout);

        let tree = editor
            .active_buffer()
            .widget_tree
            .as_ref()
            .expect("fx tree");
        for title in ["OSC MIX", "OSC SHAPE", "MS FILTER"] {
            assert!(
                value_contains_string(tree, title),
                "real korg1 custom UI should contain panel {title}"
            );
        }

        let instrument_panel = find_layout_node_by_debug_name(&layout, "instrument-panel")
            .expect("instrument panel layout node");
        let mut layout_summaries = Vec::new();
        collect_layout_node_summaries(&layout, &mut layout_summaries);
        let osc_mix = find_layout_node_by_text(&layout, "OSC MIX")
            .unwrap_or_else(|| panic!("OSC MIX label; layout={layout_summaries:#?}"));
        let ms_filter = find_layout_node_by_text(&layout, "MS FILTER")
            .unwrap_or_else(|| panic!("MS FILTER label; layout={layout_summaries:#?}"));

        assert!(
            instrument_panel.rect.width > 70.0 && instrument_panel.rect.height > 8.0,
            "instrument panel should occupy visible measured space, got {:?}",
            instrument_panel.rect
        );
        assert!(
            osc_mix.rect.width > 1.0
                && osc_mix.rect.col >= instrument_panel.rect.col
                && osc_mix.rect.col < instrument_panel.rect.col + instrument_panel.rect.width,
            "OSC MIX label should be measured inside the instrument panel, got {:?}",
            osc_mix.rect
        );
        assert!(
            ms_filter.rect.width > 1.0
                && ms_filter.rect.col > osc_mix.rect.col + 40.0
                && ms_filter.rect.col < instrument_panel.rect.col + instrument_panel.rect.width,
            "MS FILTER should be measured as a later korg1 column, got {:?}; OSC MIX={:?}; panel={:?}",
            ms_filter.rect,
            osc_mix.rect,
            instrument_panel.rect
        );
    }

    #[test]
    fn metal_seq_fx_lisp_lays_out_analog_bread_and_butter_lfo_column() {
        fn find_stable_key_suffix<'a>(
            node: &'a eseqlisp::layout::LayoutNode,
            suffix: &str,
        ) -> Option<&'a eseqlisp::layout::LayoutNode> {
            if node
                .stable_key
                .as_deref()
                .is_some_and(|key| key.ends_with(suffix))
            {
                return Some(node);
            }
            node.children
                .iter()
                .find_map(|child| find_stable_key_suffix(child, suffix))
        }

        let src = std::fs::read_to_string("metal-seq-fx.lisp").expect("read fx lisp");
        let analog_ui = std::fs::read_to_string("instruments/core/analog-bread-and-butter/ui.lisp")
            .expect("read analog bread-and-butter ui");
        let custom_ui_source = build_custom_instrument_ui_source_with_overlay(Some((
            "test-instrument".to_string(),
            "instruments/core/analog-bread-and-butter/ui.lisp".to_string(),
            analog_ui,
        )));
        let mut analog_inst = test_instrument_map();
        analog_inst.insert(
            "synth".to_string(),
            Rc::new(RefCell::new(test_list(vec![
                Value::Map(test_param_map("lfo2_wave", 0, 1.0, 0.0, 3.0)),
                Value::Map(test_param_map("lfo2_rate_hz", 1, 0.21, 0.03, 18.0)),
                Value::Map(test_param_map("lfo2_to_f1", 2, 35.0, 0.0, 2200.0)),
                Value::Map(test_param_map("lfo2_to_f2", 3, 15.0, 0.0, 2200.0)),
                Value::Map(test_param_map("output_gain", 4, 0.28, 0.0, 1.0)),
                Value::Map(test_param_map("glide_ms", 5, 0.0, 0.0, 500.0)),
                Value::Map(test_param_map("vibrato", 6, 0.0, 0.0, 0.12)),
            ]))),
        );

        let mut editor = eseqlisp::Editor::new(Runtime::new(), eseqlisp::EditorConfig::default());
        editor.set_layout_viewport(180, 18);
        editor.runtime_mut().register_reactive(
            "SEQ",
            vec![
                ("num-tracks", Value::Number(1.0)),
                ("compiling", Value::Bool(false)),
                ("available-effects", test_list(vec![])),
                ("available-builtin-effects", test_list(vec![])),
                ("available-midi-effects", test_list(vec![])),
                ("bus-names", test_list(vec![])),
                ("effects", test_list(vec![])),
                ("midi-effects", test_list(vec![])),
                ("instrument-panel", test_list(vec![Value::Map(analog_inst)])),
                ("bus-effects", test_list(vec![])),
            ],
            true,
        );
        editor
            .runtime_mut()
            .eval_str(
                r#"
                (def selected-bus-name () "Mix")
                (def seq-has-selection? () false)
                (def sbrowser-editor-name "")
                (defmacro aqua-slider-material () `(material :color (rgba 0.15 0.15 0.88 1.0)))
                (def custom-midi-fx-ui (fx) false)
                (def custom-audio-fx-ui (fx) false)
                (defstate selected-bus -1)
                "#,
            )
            .expect("install fx test helpers");
        register_test_delete_target_natives(&mut editor, 1);
        editor
            .runtime_mut()
            .eval_str(&custom_ui_source)
            .expect("load analog custom instrument ui");
        editor.runtime_mut().eval_str(&src).expect("load fx lisp");
        editor.refresh_runtime_side_effects();
        if let Some(status) = editor.runtime_mut().take_status_message() {
            panic!("analog bread-and-butter fx lisp status after refresh: {status}");
        }

        let fx_id = editor
            .buffers
            .iter()
            .find(|buffer| buffer.name == "*fx*")
            .expect("fx lisp should create the *fx* buffer")
            .id;
        editor.set_active_buffer(fx_id);
        editor.set_layout_viewport(180, 18);
        let layout = editor
            .widget_layout()
            .expect("analog bread-and-butter layout should build");

        let instrument_panel = find_layout_node_by_debug_name(&layout, "instrument-panel")
            .expect("instrument panel layout node");
        assert!(
            instrument_panel.rect.width > 70.0 && instrument_panel.rect.height > 8.0,
            "instrument panel should occupy visible measured space, got {:?}",
            instrument_panel.rect
        );

        let adsr_editor =
            find_layout_node_by_widget_type(&layout, "adsr-editor").expect("adsr editor");
        assert!(
            adsr_editor.rect.width > 8.0
                && adsr_editor.rect.height > 2.0
                && adsr_editor.rect.height <= 4.0,
            "ADSR editor should stay constrained in the medium detail panel, got {:?}",
            adsr_editor.rect
        );
        assert!(
            adsr_editor.rect.row >= instrument_panel.rect.row
                && adsr_editor.rect.row + adsr_editor.rect.height
                    <= instrument_panel.rect.row + instrument_panel.rect.height,
            "ADSR editor should be vertically inside the visible instrument panel, got {:?}; panel={:?}",
            adsr_editor.rect,
            instrument_panel.rect
        );

        for suffix in ["lfo2_rate_hz", "lfo2_to_f2", "output_gain"] {
            let node = find_stable_key_suffix(&layout, suffix)
                .unwrap_or_else(|| panic!("{suffix} control should be present in layout"));
            assert!(
                node.rect.width > 1.0 && node.rect.height > 0.0,
                "{suffix} should have a finite nonzero rect, got {:?}",
                node.rect
            );
            assert!(
                node.rect.row >= instrument_panel.rect.row
                    && node.rect.row + node.rect.height
                        <= instrument_panel.rect.row + instrument_panel.rect.height,
                "{suffix} should be vertically inside the visible instrument panel, got {:?}; panel={:?}",
                node.rect,
                instrument_panel.rect
            );
        }
    }

    #[test]
    fn metal_seq_fx_lisp_lays_out_audio_effect_mod_selector() {
        fn layout_has_double_click(node: &eseqlisp::layout::LayoutNode) -> bool {
            node.props.contains_key("on-double-click")
                || node.children.iter().any(layout_has_double_click)
        }

        fn test_mod_target(depth_idx: f64, source_slot: f64, depth: f64) -> Value {
            Value::Map(HashMap::from([
                (
                    "depth-idx".to_string(),
                    Rc::new(RefCell::new(Value::Number(depth_idx))),
                ),
                (
                    "source-slot".to_string(),
                    Rc::new(RefCell::new(Value::Number(source_slot))),
                ),
                (
                    "depth".to_string(),
                    Rc::new(RefCell::new(Value::Number(depth))),
                ),
                (
                    "depth-min".to_string(),
                    Rc::new(RefCell::new(Value::Number(-4.0))),
                ),
                (
                    "depth-max".to_string(),
                    Rc::new(RefCell::new(Value::Number(4.0))),
                ),
            ]))
        }

        let mut enabled = test_param_map("enabled", 0, 1.0, 0.0, 1.0);
        enabled.insert(
            "boolean".to_string(),
            Rc::new(RefCell::new(Value::Bool(true))),
        );
        let mut cutoff = test_param_map("cutoff", 1, 1200.0, 20.0, 20_000.0);
        cutoff.insert(
            "modulatable".to_string(),
            Rc::new(RefCell::new(Value::Bool(true))),
        );
        cutoff.insert(
            "mod-targets".to_string(),
            Rc::new(RefCell::new(test_list(vec![test_mod_target(
                11.0, 1.0, 0.5,
            )]))),
        );
        let mut mod1_sync = test_param_map("sync", 31, 0.0, 0.0, 1.0);
        mod1_sync.insert(
            "boolean".to_string(),
            Rc::new(RefCell::new(Value::Bool(true))),
        );

        let effect = Value::Map(HashMap::from([
            (
                "name".to_string(),
                Rc::new(RefCell::new(Value::String("Filter".to_string()))),
            ),
            (
                "slot-idx".to_string(),
                Rc::new(RefCell::new(Value::Number(0.0))),
            ),
            (
                "builtin".to_string(),
                Rc::new(RefCell::new(Value::Bool(true))),
            ),
            (
                "params".to_string(),
                Rc::new(RefCell::new(test_list(vec![
                    Value::Map(enabled),
                    Value::Map(cutoff),
                    Value::Map(test_param_map("resonance", 2, 0.4, 0.0, 1.0)),
                ]))),
            ),
            (
                "modulators".to_string(),
                Rc::new(RefCell::new(test_list(vec![Value::Map(HashMap::from([
                    (
                        "slot".to_string(),
                        Rc::new(RefCell::new(Value::Number(1.0))),
                    ),
                    (
                        "label".to_string(),
                        Rc::new(RefCell::new(Value::String("Mod 1".to_string()))),
                    ),
                ]))]))),
            ),
            (
                "sources".to_string(),
                Rc::new(RefCell::new(test_list(vec![Value::Map(HashMap::from([
                    (
                        "name".to_string(),
                        Rc::new(RefCell::new(Value::String("Mod 1".to_string()))),
                    ),
                    (
                        "slot".to_string(),
                        Rc::new(RefCell::new(Value::Number(1.0))),
                    ),
                    (
                        "source-param".to_string(),
                        Rc::new(RefCell::new(Value::Map(test_enum_param_map(
                            "type",
                            30,
                            1.0,
                            vec![
                                "off", "lfo", "env", "rand", "drift", "ext1", "ext2", "ext3",
                                "ext4",
                            ],
                        )))),
                    ),
                    (
                        "params".to_string(),
                        Rc::new(RefCell::new(test_list(vec![
                            Value::Map(test_param_map("rate", 31, 2.0, 0.1, 20.0)),
                            Value::Map(mod1_sync),
                            Value::Map(test_enum_param_map(
                                "division",
                                32,
                                1.0,
                                vec!["1/8", "1/4", "1/2"],
                            )),
                            Value::Map(test_enum_param_map(
                                "shape",
                                33,
                                1.0,
                                vec!["triangle", "sine", "pulse"],
                            )),
                            Value::Map(test_param_map("pulse width", 34, 0.5, 0.0, 1.0)),
                            Value::Map(test_param_map("retrigger", 35, 0.0, 0.0, 1.0)),
                        ]))),
                    ),
                ]))]))),
            ),
        ]));

        let src = std::fs::read_to_string("metal-seq-fx.lisp").expect("read fx lisp");
        let mut editor = eseqlisp::Editor::new(Runtime::new(), eseqlisp::EditorConfig::default());
        editor.set_layout_viewport(160, 18);
        editor.runtime_mut().register_reactive(
            "SEQ",
            vec![
                ("num-tracks", Value::Number(1.0)),
                ("compiling", Value::Bool(false)),
                ("available-effects", test_list(vec![])),
                ("available-builtin-effects", test_list(vec![])),
                ("available-midi-effects", test_list(vec![])),
                ("bus-names", test_list(vec![])),
                ("effects", test_list(vec![effect])),
                ("midi-effects", test_list(vec![])),
                (
                    "instrument-panel",
                    test_list(vec![Value::Map(test_instrument_map())]),
                ),
                ("bus-effects", test_list(vec![])),
            ],
            true,
        );
        editor
            .runtime_mut()
            .eval_str(
                r#"
                (def selected-bus-name () "Mix")
                (def seq-has-selection? () false)
                (def sbrowser-editor-name "")
                (defmacro aqua-slider-material () `(material :color (rgba 0.15 0.15 0.88 1.0)))
                (def custom-instrument-synth-ui (inst) false)
                (def custom-midi-fx-ui (fx) false)
                (def custom-audio-fx-ui (fx) false)
                (defstate selected-bus -1)
                "#,
            )
            .expect("install fx test helpers");
        register_test_delete_target_natives(&mut editor, 1);
        editor.runtime_mut().eval_str(&src).expect("load fx lisp");
        editor
            .runtime_mut()
            .eval_str(
                r#"(do
                  (set! effect-mods-open true)
                  (set! effect-mods-chain "audio")
                  (set! effect-mods-slot 0)
                  (set! effect-mods-bus -1)
                  (set! effect-selected-mod-slot 1))"#,
            )
            .expect("open effect mods");
        editor.refresh_runtime_side_effects();
        if let Some(status) = editor.runtime_mut().take_status_message() {
            panic!("effect mods fx lisp status after refresh: {status}");
        }

        let fx_id = editor
            .buffers
            .iter()
            .find(|buffer| buffer.name == "*fx*")
            .expect("fx lisp should create the *fx* buffer")
            .id;
        editor.set_active_buffer(fx_id);
        let layout = editor.widget_layout().expect("effect mods layout");
        assert_finite_layout_tree(&layout);
        let selector = find_layout_node_by_debug_name(&layout, "effect-mod-selector")
            .expect("effect mods selector should render");
        let inline_body = find_layout_node_by_debug_name(&layout, "effect-mods-inline-body")
            .expect("effect mods inline body should render");
        let control_panel = find_layout_node_by_debug_name(&layout, "effect-mod-control-panel")
            .expect("effect mods control panel should render");
        assert!(
            selector.rect.width > 0.0 && selector.rect.height > 0.0,
            "effect selector must have a nonzero rect: {:?}",
            selector.rect
        );
        assert!(
            control_panel.rect.height >= inline_body.rect.height - 0.1,
            "effect mods black panel should fill the inline body height; panel={:?}, body={:?}",
            control_panel.rect,
            inline_body.rect
        );
        assert!(
            control_panel.rect.height > 6.5,
            "effect mods black panel should use the available fixed effect panel height, got {:?}",
            control_panel.rect
        );
        assert!(
            find_layout_node_by_debug_name(&layout, "effect-lfo-source-editor").is_some(),
            "selected LFO editor should render for effect-local Mod 1"
        );
        assert!(
            layout_has_double_click(&layout),
            "modulatable effect cutoff should render an interactive modulation wrapper"
        );
    }

    #[test]
    fn custom_audio_effect_mods_button_reopens_after_close() {
        fn test_mod_target(depth_idx: f64, source_slot: f64, depth: f64) -> Value {
            Value::Map(HashMap::from([
                (
                    "depth-idx".to_string(),
                    Rc::new(RefCell::new(Value::Number(depth_idx))),
                ),
                (
                    "source-slot".to_string(),
                    Rc::new(RefCell::new(Value::Number(source_slot))),
                ),
                (
                    "depth".to_string(),
                    Rc::new(RefCell::new(Value::Number(depth))),
                ),
                (
                    "depth-min".to_string(),
                    Rc::new(RefCell::new(Value::Number(-1.0))),
                ),
                (
                    "depth-max".to_string(),
                    Rc::new(RefCell::new(Value::Number(1.0))),
                ),
            ]))
        }

        fn layout_node_contains_string(node: &eseqlisp::layout::LayoutNode, needle: &str) -> bool {
            node.props
                .values()
                .any(|value| value_contains_string(value, needle))
                || node
                    .children
                    .iter()
                    .any(|child| layout_node_contains_string(child, needle))
        }

        fn find_clickable_node_containing<'a>(
            node: &'a eseqlisp::layout::LayoutNode,
            needle: &str,
        ) -> Option<&'a eseqlisp::layout::LayoutNode> {
            if let Some(child) = node
                .children
                .iter()
                .find_map(|child| find_clickable_node_containing(child, needle))
            {
                return Some(child);
            }
            if node.props.contains_key("on-click") && layout_node_contains_string(node, needle) {
                return Some(node);
            }
            None
        }

        fn find_effect_mods_button_rect(editor: &eseqlisp::Editor) -> eseqlisp::layout::Rect {
            let layout = editor.widget_layout().expect("custom effect layout");
            let panel =
                find_layout_node_by_debug_name(&layout, "audio-fx-panel-root-0-toggle-effect")
                    .expect("custom effect panel should render");
            let header = find_layout_node_by_debug_name(panel, "audio-fx-panel-header")
                .expect("custom effect header should render");
            let button = find_clickable_node_containing(header, "mods")
                .expect("custom effect mods button should be clickable");
            assert!(
                button.rect.row >= header.rect.row - 0.05
                    && button.rect.row + button.rect.height
                        <= header.rect.row + header.rect.height + 0.05,
                "mods button hit area should stay inside the FX header; button={:?} header={:?}",
                button.rect,
                header.rect
            );
            let hit = eseqlisp::layout::hit_test_layout(
                &layout,
                button.rect.row + button.rect.height * 0.85,
                button.rect.col + button.rect.width * 0.5,
            )
            .expect("mods button lower hit area should hit a widget");
            assert_eq!(
                hit.widget_id, button.widget_id,
                "mods button lower hit area should hit the button, got {} {:?}",
                hit.widget_type, hit.rect
            );
            button.rect
        }

        fn assert_effect_mods_open(editor: &mut eseqlisp::Editor, expected: bool, context: &str) {
            assert_eq!(
                editor
                    .runtime_mut()
                    .eval_str("effect-mods-open")
                    .expect("read effect mods open"),
                Some(Value::Bool(expected)),
                "{context}"
            );
            let layout = editor.widget_layout().expect("custom effect layout");
            assert_eq!(
                find_layout_node_by_debug_name(&layout, "effect-mods-inline-body").is_some(),
                expected,
                "{context}: visible layout should match effect-mods-open"
            );
        }

        fn click_effect_mods_button(editor: &mut eseqlisp::Editor) {
            let rect = find_effect_mods_button_rect(editor);
            let col = rect.col + rect.width * 0.5;
            let row = rect.row + rect.height * 0.85;
            editor.handle_mouse_precise(
                crossterm::event::MouseEvent {
                    kind: crossterm::event::MouseEventKind::Down(
                        crossterm::event::MouseButton::Left,
                    ),
                    column: col.floor() as u16,
                    row: row.floor() as u16,
                    modifiers: crossterm::event::KeyModifiers::NONE,
                },
                0,
                0,
                160,
                18,
                col,
                row,
            );
            editor.handle_mouse_precise(
                crossterm::event::MouseEvent {
                    kind: crossterm::event::MouseEventKind::Drag(
                        crossterm::event::MouseButton::Left,
                    ),
                    column: (col + 1.25).floor() as u16,
                    row: row.floor() as u16,
                    modifiers: crossterm::event::KeyModifiers::NONE,
                },
                0,
                0,
                160,
                18,
                col + 1.25,
                row,
            );
            editor.handle_mouse_precise(
                crossterm::event::MouseEvent {
                    kind: crossterm::event::MouseEventKind::Up(crossterm::event::MouseButton::Left),
                    column: (col + 1.25).floor() as u16,
                    row: row.floor() as u16,
                    modifiers: crossterm::event::KeyModifiers::NONE,
                },
                0,
                0,
                160,
                18,
                col + 1.25,
                row,
            );
            editor.refresh_runtime_side_effects();
        }

        let mut cutoff = test_param_map("cutoff", 1, 1200.0, 20.0, 20_000.0);
        cutoff.insert(
            "modulatable".to_string(),
            Rc::new(RefCell::new(Value::Bool(true))),
        );
        cutoff.insert(
            "mod-targets".to_string(),
            Rc::new(RefCell::new(test_list(vec![test_mod_target(
                11.0, 1.0, 0.5,
            )]))),
        );
        let effect = Value::Map(HashMap::from([
            (
                "name".to_string(),
                Rc::new(RefCell::new(Value::String("toggle-effect".to_string()))),
            ),
            (
                "slot-idx".to_string(),
                Rc::new(RefCell::new(Value::Number(0.0))),
            ),
            (
                "builtin".to_string(),
                Rc::new(RefCell::new(Value::Bool(false))),
            ),
            (
                "params".to_string(),
                Rc::new(RefCell::new(test_list(vec![
                    Value::Map(cutoff),
                    Value::Map(test_param_map("mix", 2, 0.5, 0.0, 1.0)),
                ]))),
            ),
            (
                "modulators".to_string(),
                Rc::new(RefCell::new(test_list(vec![Value::Map(HashMap::from([
                    (
                        "slot".to_string(),
                        Rc::new(RefCell::new(Value::Number(1.0))),
                    ),
                    (
                        "label".to_string(),
                        Rc::new(RefCell::new(Value::String("Mod 1".to_string()))),
                    ),
                ]))]))),
            ),
            (
                "sources".to_string(),
                Rc::new(RefCell::new(test_list(vec![Value::Map(HashMap::from([
                    (
                        "name".to_string(),
                        Rc::new(RefCell::new(Value::String("Mod 1".to_string()))),
                    ),
                    (
                        "slot".to_string(),
                        Rc::new(RefCell::new(Value::Number(1.0))),
                    ),
                    (
                        "source-param".to_string(),
                        Rc::new(RefCell::new(Value::Map(test_enum_param_map(
                            "type",
                            30,
                            1.0,
                            vec![
                                "off", "lfo", "env", "rand", "drift", "ext1", "ext2", "ext3",
                                "ext4",
                            ],
                        )))),
                    ),
                    (
                        "params".to_string(),
                        Rc::new(RefCell::new(test_list(vec![Value::Map(test_param_map(
                            "rate", 31, 2.0, 0.1, 20.0,
                        ))]))),
                    ),
                ]))]))),
            ),
        ]));

        let src = std::fs::read_to_string("metal-seq-fx.lisp").expect("read fx lisp");
        let mut editor = eseqlisp::Editor::new(Runtime::new(), eseqlisp::EditorConfig::default());
        editor.set_layout_viewport(160, 18);
        editor.runtime_mut().register_reactive(
            "SEQ",
            vec![
                ("num-tracks", Value::Number(1.0)),
                ("compiling", Value::Bool(false)),
                ("available-effects", test_list(vec![])),
                ("available-builtin-effects", test_list(vec![])),
                ("available-midi-effects", test_list(vec![])),
                ("bus-names", test_list(vec![])),
                ("effects", test_list(vec![effect])),
                ("midi-effects", test_list(vec![])),
                (
                    "instrument-panel",
                    test_list(vec![Value::Map(test_instrument_map())]),
                ),
                ("bus-effects", test_list(vec![])),
            ],
            true,
        );
        editor
            .runtime_mut()
            .eval_str(
                r#"
                (def selected-bus-name () "Mix")
                (def seq-has-selection? () false)
                (def sbrowser-editor-name "")
                (defmacro aqua-slider-material () `(material :color (rgba 0.15 0.15 0.88 1.0)))
                (def custom-instrument-synth-ui (inst) false)
                (def custom-midi-fx-ui (fx) false)
                (def custom-audio-fx-ui (fx) false)
                (defstate selected-bus -1)
                "#,
            )
            .expect("install fx test helpers");
        register_test_delete_target_natives(&mut editor, 1);
        editor.runtime_mut().eval_str(&src).expect("load fx lisp");
        editor.refresh_runtime_side_effects();
        if let Some(status) = editor.runtime_mut().take_status_message() {
            panic!("custom effect mods toggle status after refresh: {status}");
        }
        let fx_id = editor
            .buffers
            .iter()
            .find(|buffer| buffer.name == "*fx*")
            .expect("fx lisp should create the *fx* buffer")
            .id;
        editor.set_active_buffer(fx_id);

        assert_effect_mods_open(&mut editor, false, "mods should start closed");
        for toggle_idx in 0..12 {
            click_effect_mods_button(&mut editor);
            let commands = editor.drain_host_commands();
            assert!(
                commands.is_empty(),
                "mods button click-drag should not arm the FX header drag/drop path: {commands:?}"
            );
            assert_effect_mods_open(
                &mut editor,
                toggle_idx % 2 == 0,
                &format!("mods toggle {} should flip state", toggle_idx + 1),
            );
        }
    }

    #[test]
    fn effect_sidechain_host_control_survives_effects_value_param_filter() {
        fn map_get<'a>(value: &'a Value, key: &str) -> Option<std::cell::Ref<'a, Value>> {
            let Value::Map(map) = value else {
                return None;
            };
            map.get(key).map(|value| value.borrow())
        }

        let desc = sequencer::effects::EffectDescriptor {
            name: "sidechain".to_string(),
            input_channels: 7,
            output_channels: 2,
            instrument_modulators: Vec::new(),
            instrument_modulation_targets: Vec::new(),
            params: vec![
                sequencer::effects::ParamDescriptor {
                    name: "threshold".to_string(),
                    min: -80.0,
                    max: -2.0,
                    default: -20.0,
                    kind: sequencer::effects::ParamKind::Continuous { unit: None },
                    scaling: sequencer::effects::ParamScaling::Linear,
                    node_param_idx: 0,
                    node_param_span: 1,
                    host_control: None,
                    ui_metadata: None,
                },
                sequencer::effects::ParamDescriptor {
                    name: "sidechain".to_string(),
                    min: 0.0,
                    max: 1.0,
                    default: 0.0,
                    kind: sequencer::effects::ParamKind::Enum {
                        labels: vec!["off".to_string(), "Track 2".to_string()],
                    },
                    scaling: sequencer::effects::ParamScaling::Linear,
                    node_param_idx: u32::MAX,
                    node_param_span: 1,
                    host_control: Some(sequencer::effects::HostControl::FxSidechain {
                        input_channel: 6,
                    }),
                    ui_metadata: None,
                },
            ],
        };
        let state = Arc::new(SequencerState::new(
            1,
            vec![sequencer::sequencer::default_empty_effect_chain()],
        ));
        state.pattern.effect_chains[0][0].apply_descriptor(&desc, 42);
        let selected = Arc::new(Mutex::new(HashSet::new()));

        let effects = build_effects_value(&state, 0, &[vec![desc]], &selected);

        let Value::List(slots) = effects else {
            panic!("effects value should be a list");
        };
        let first_slot = slots
            .first()
            .expect("descriptor should produce an effect slot")
            .borrow();
        let params = map_get(&first_slot, "params").expect("effect should expose params");
        let Value::List(params) = &*params else {
            panic!("params should be a list: {params:?}");
        };
        let sidechain = params
            .iter()
            .find(|param| {
                map_get(&param.borrow(), "name")
                    .map(|name| matches!(&*name, Value::String(name) if name == "sidechain"))
                    .unwrap_or(false)
            })
            .unwrap_or_else(|| panic!("sidechain param should survive filter: {params:?}"))
            .borrow();
        let options = map_get(&sidechain, "options").expect("sidechain should expose options");
        let Value::List(options) = &*options else {
            panic!("sidechain options should be a list: {options:?}");
        };
        let labels = options
            .iter()
            .map(|value| match &*value.borrow() {
                Value::String(label) => label.clone(),
                other => panic!("sidechain option should be a string, got {other:?}"),
            })
            .collect::<Vec<_>>();
        assert_eq!(labels, vec!["off".to_string(), "Track 2".to_string()]);
    }

    #[test]
    fn effect_source_dropdown_keeps_options_without_slot_state() {
        fn map_get<'a>(value: &'a Value, key: &str) -> Option<std::cell::Ref<'a, Value>> {
            let Value::Map(map) = value else {
                return None;
            };
            map.get(key).map(|value| value.borrow())
        }

        let state = Arc::new(SequencerState::new(1, vec![vec![]]));
        let selected = Arc::new(Mutex::new(HashSet::new()));
        let desc = sequencer::effects::EffectDescriptor::builtin_filter();
        let effects = build_effects_value(&state, 0, &[vec![desc]], &selected);

        let Value::List(slots) = effects else {
            panic!("effects value should be a list");
        };
        let first_slot = slots
            .first()
            .expect("descriptor should produce an effect row")
            .borrow();
        let sources = map_get(&first_slot, "sources").expect("effect should expose sources");
        let Value::List(source_sections) = &*sources else {
            panic!("sources should be a list: {sources:?}");
        };
        let mod1 = source_sections
            .first()
            .expect("mod1 source section should exist")
            .borrow();
        let source_param = map_get(&mod1, "source-param")
            .expect("mod1 should keep its source-param even before slot state is synced");
        let options = map_get(&source_param, "options").expect("source-param should have options");
        let Value::List(options) = &*options else {
            panic!("source options should be a list: {options:?}");
        };
        let labels = options
            .iter()
            .map(|value| match &*value.borrow() {
                Value::String(label) => label.clone(),
                other => panic!("source option should be a string, got {other:?}"),
            })
            .collect::<Vec<_>>();
        assert_eq!(
            labels,
            vec!["off", "lfo", "env", "rand", "drift", "ext1", "ext2", "ext3", "ext4"]
        );
    }

    #[test]
    fn metal_seq_fx_lisp_lays_out_modulator_panel_controls() {
        let src = std::fs::read_to_string("metal-seq-fx.lisp").expect("read fx lisp");
        let mut editor = eseqlisp::Editor::new(Runtime::new(), eseqlisp::EditorConfig::default());
        editor.set_layout_viewport(180, 18);
        editor.runtime_mut().register_reactive(
            "SEQ",
            vec![
                ("num-tracks", Value::Number(1.0)),
                ("compiling", Value::Bool(false)),
                ("available-effects", test_list(vec![])),
                ("available-builtin-effects", test_list(vec![])),
                ("available-midi-effects", test_list(vec![])),
                ("bus-names", test_list(vec![])),
                ("effects", test_list(vec![])),
                ("midi-effects", test_list(vec![])),
                (
                    "instrument-panel",
                    test_list(vec![Value::Map(test_modulator_instrument_map())]),
                ),
                ("bus-effects", test_list(vec![])),
            ],
            true,
        );
        editor
            .runtime_mut()
            .eval_str(
                r#"
                (def selected-bus-name () "Mix")
                (def seq-has-selection? () false)
                (def sbrowser-editor-name "")
                (defmacro aqua-slider-material () `(material :color (rgba 0.15 0.15 0.88 1.0)))
                (def custom-instrument-synth-ui (inst) false)
                (def custom-midi-fx-ui (fx) false)
                (def custom-audio-fx-ui (fx) false)
                (defstate selected-bus -1)
                "#,
            )
            .expect("install fx test helpers");
        register_test_delete_target_natives(&mut editor, 1);
        editor.runtime_mut().eval_str(&src).expect("load fx lisp");
        editor.refresh_runtime_side_effects();
        if let Some(status) = editor.runtime_mut().take_status_message() {
            panic!("modulator panel fx lisp status after refresh: {status}");
        }

        let fx_id = editor
            .buffers
            .iter()
            .find(|buffer| buffer.name == "*fx*")
            .expect("fx lisp should create the *fx* buffer")
            .id;
        editor.set_active_buffer(fx_id);
        editor.set_layout_viewport(180, 18);
        let layout = editor.widget_layout().expect("modulator panel layout");
        assert_finite_layout_tree(&layout);

        let panel =
            find_layout_node_by_debug_name(&layout, "modulator-panel").expect("modulator panel");
        let body = find_layout_node_by_debug_name(&layout, "modulator-panel-body")
            .expect("modulator panel body");
        let curve = find_layout_node_by_widget_type(&layout, "modulator-curve")
            .expect("modulator curve widget");
        assert!(matches!(
            curve.props.get("phase"),
            Some(Value::ReactiveRef {
                namespace,
                field,
                ..
            }) if namespace == "SEQ" && field == "modulator-phase-0"
        ));
        assert!(matches!(
            curve.props.get("level"),
            Some(Value::ReactiveRef {
                namespace,
                field,
                ..
            }) if namespace == "SEQ" && field == "modulator-level-0"
        ));
        assert!(
            find_layout_node_by_widget_type(&layout, "fx-enabled-dot").is_some(),
            "modulator panel should expose the standard enabled toggle"
        );
        assert_eq!(
            count_widget_type(&layout, "knob-number"),
            2,
            "modulator panel should expose only rise and fall knobs"
        );
        assert!(
            panel.rect.width > 30.0 && panel.rect.height > 8.0,
            "modulator panel should occupy visible measured space, got {:?}",
            panel.rect
        );
        assert!(
            body.rect.width > 25.0 && body.rect.height > 5.0,
            "modulator panel body should have nonzero visible space, got {:?}",
            body.rect
        );
        assert!(
            curve.rect.width > 8.0
                && curve.rect.height > 4.0
                && curve.rect.col >= panel.rect.col
                && curve.rect.row >= panel.rect.row
                && curve.rect.col + curve.rect.width <= panel.rect.col + panel.rect.width
                && curve.rect.row + curve.rect.height <= panel.rect.row + panel.rect.height,
            "modulator curve should be visible inside panel, curve={:?}; panel={:?}",
            curve.rect,
            panel.rect
        );
    }

    #[test]
    fn metal_seq_fx_lisp_lays_out_mod_fm_messui_dense_controls() {
        fn find_stable_key_suffix<'a>(
            node: &'a eseqlisp::layout::LayoutNode,
            suffix: &str,
        ) -> Option<&'a eseqlisp::layout::LayoutNode> {
            if node
                .stable_key
                .as_deref()
                .is_some_and(|key| key.ends_with(suffix))
            {
                return Some(node);
            }
            node.children
                .iter()
                .find_map(|child| find_stable_key_suffix(child, suffix))
        }

        let src = std::fs::read_to_string("metal-seq-fx.lisp").expect("read fx lisp");
        let ui = std::fs::read_to_string("instruments/fm/mod-fm-messui/ui.lisp")
            .expect("read mod FM ui");
        let custom_ui_source = build_custom_instrument_ui_source_with_overlay(Some((
            "mod-fm-messui/".to_string(),
            "instruments/fm/mod-fm-messui/ui.lisp".to_string(),
            ui,
        )));

        let mut editor = eseqlisp::Editor::new(Runtime::new(), eseqlisp::EditorConfig::default());
        editor.set_layout_viewport(180, 18);
        editor.runtime_mut().register_reactive(
            "SEQ",
            vec![
                ("num-tracks", Value::Number(1.0)),
                ("compiling", Value::Bool(false)),
                ("available-effects", test_list(vec![])),
                ("available-builtin-effects", test_list(vec![])),
                ("available-midi-effects", test_list(vec![])),
                ("bus-names", test_list(vec![])),
                ("effects", test_list(vec![])),
                ("midi-effects", test_list(vec![])),
                (
                    "instrument-panel",
                    test_list(vec![Value::Map(mod_fm_messui_test_instrument_map())]),
                ),
                ("bus-effects", test_list(vec![])),
            ],
            true,
        );
        editor
            .runtime_mut()
            .eval_str(
                r#"
                (def selected-bus-name () "Mix")
                (def seq-has-selection? () false)
                (def sbrowser-editor-name "")
                (defmacro aqua-slider-material () `(material :color (rgba 0.15 0.15 0.88 1.0)))
                (def custom-midi-fx-ui (fx) false)
                (def custom-audio-fx-ui (fx) false)
                (defstate selected-bus -1)
                "#,
            )
            .expect("install fx test helpers");
        register_test_delete_target_natives(&mut editor, 1);
        editor
            .runtime_mut()
            .eval_str(&custom_ui_source)
            .expect("load mod FM custom instrument ui");
        editor.runtime_mut().eval_str(&src).expect("load fx lisp");
        editor.refresh_runtime_side_effects();
        if let Some(status) = editor.runtime_mut().take_status_message() {
            panic!("mod FM custom instrument fx lisp status after refresh: {status}");
        }

        let fx_id = editor
            .buffers
            .iter()
            .find(|buffer| buffer.name == "*fx*")
            .expect("fx lisp should create the *fx* buffer")
            .id;
        editor.set_active_buffer(fx_id);
        editor.set_layout_viewport(180, 18);
        let layout = editor.widget_layout().expect("mod FM layout should build");
        let rendered = render_layout_cells(&layout, 180, 18);
        assert!(
            !rendered.contains("missing:"),
            "mod FM dense UI should not render missing-param diagnostics:\n{rendered}"
        );

        let instrument_panel = find_layout_node_by_debug_name(&layout, "instrument-panel")
            .expect("instrument panel layout node");
        let adsr_editor =
            find_layout_node_by_widget_type(&layout, "adsr-editor").expect("adsr editor");
        assert!(
            adsr_editor.rect.width > 8.0
                && adsr_editor.rect.height > 2.0
                && adsr_editor.rect.height <= 4.0,
            "mod FM ADSR editor should stay constrained in the dense detail panel, got {:?}",
            adsr_editor.rect
        );

        let op1_ratio = find_stable_key_suffix(&layout, "op1_ratio").expect("op1 ratio control");
        let op1_level = find_stable_key_suffix(&layout, "op1_level").expect("op1 level knob");
        let op3_level = find_stable_key_suffix(&layout, "op3_level").expect("op3 level knob");
        let f1_mode = find_stable_key_suffix(&layout, "f1_mode").expect("filter mode dropdown");
        let f1_cutoff = find_stable_key_suffix(&layout, "f1_cutoff").expect("filter cutoff knob");

        assert!(
            op1_ratio.rect.width > 1.0
                && op1_level.rect.width > 1.0
                && op3_level.rect.width > 1.0
                && f1_mode.rect.width > 1.0
                && f1_cutoff.rect.width > 1.0,
            "dense controls should have visible measured rects: op1_ratio={:?} op1_level={:?} op3_level={:?} f1_mode={:?} f1_cutoff={:?}",
            op1_ratio.rect,
            op1_level.rect,
            op3_level.rect,
            f1_mode.rect,
            f1_cutoff.rect
        );
        assert!(
            op1_ratio.rect.col + op1_ratio.rect.width <= op1_level.rect.col
                && op1_level.rect.col + op1_level.rect.width <= op3_level.rect.col
                && f1_mode.rect.col + f1_mode.rect.width <= f1_cutoff.rect.col,
            "dense micro clusters should end before the knob lanes; op1_ratio={:?} op1_level={:?} op3_level={:?} f1_mode={:?} f1_cutoff={:?}",
            op1_ratio.rect,
            op1_level.rect,
            op3_level.rect,
            f1_mode.rect,
            f1_cutoff.rect
        );

        for suffix in [
            "op3_level",
            "f1_cutoff",
            "f2_resonance",
            "filter_route",
            "gain",
        ] {
            let node = find_stable_key_suffix(&layout, suffix)
                .unwrap_or_else(|| panic!("{suffix} control should be present"));
            assert!(
                node.rect.row >= instrument_panel.rect.row
                    && node.rect.row + node.rect.height
                        <= instrument_panel.rect.row + instrument_panel.rect.height,
                "{suffix} should be vertically inside the visible instrument panel, got {:?}; panel={:?}",
                node.rect,
                instrument_panel.rect
            );
        }
    }

    #[test]
    fn metal_seq_fx_lisp_collects_modded_909_mutant_knob_primitives() {
        use crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};

        fn find_descendant_widget<'a>(
            node: &'a eseqlisp::layout::LayoutNode,
            widget_type: &str,
        ) -> Option<&'a eseqlisp::layout::LayoutNode> {
            if node.widget_type == widget_type {
                return Some(node);
            }
            node.children
                .iter()
                .find_map(|child| find_descendant_widget(child, widget_type))
        }

        fn find_stable_key_suffix<'a>(
            node: &'a eseqlisp::layout::LayoutNode,
            suffix: &str,
        ) -> Option<&'a eseqlisp::layout::LayoutNode> {
            if node
                .stable_key
                .as_deref()
                .is_some_and(|key| key.ends_with(suffix))
            {
                return Some(node);
            }
            node.children
                .iter()
                .find_map(|child| find_stable_key_suffix(child, suffix))
        }

        fn mouse_event(kind: MouseEventKind, column: u16, row: u16) -> MouseEvent {
            MouseEvent {
                kind,
                column,
                row,
                modifiers: KeyModifiers::NONE,
            }
        }

        let src = std::fs::read_to_string("metal-seq-fx.lisp").expect("read fx lisp");
        let ui = std::fs::read_to_string("instruments/drums/909-mutant-fm/ui.lisp")
            .expect("read 909 ui");
        let custom_ui_source = build_custom_instrument_ui_source_with_overlay(Some((
            "909-mutant-fm/".to_string(),
            "instruments/drums/909-mutant-fm/ui.lisp".to_string(),
            ui,
        )));

        let mut editor = eseqlisp::Editor::new(Runtime::new(), eseqlisp::EditorConfig::default());
        editor.set_layout_viewport(220, 18);
        editor.runtime_mut().register_reactive(
            "SEQ",
            vec![
                ("num-tracks", Value::Number(1.0)),
                ("compiling", Value::Bool(false)),
                ("available-effects", test_list(vec![])),
                ("available-builtin-effects", test_list(vec![])),
                ("available-midi-effects", test_list(vec![])),
                ("bus-names", test_list(vec![])),
                ("effects", test_list(vec![])),
                ("midi-effects", test_list(vec![])),
                (
                    "instrument-panel",
                    test_list(vec![Value::Map(mutant_909_test_instrument_map())]),
                ),
                ("bus-effects", test_list(vec![])),
            ],
            true,
        );
        editor
            .runtime_mut()
            .eval_str(
                r#"
                (def selected-bus-name () "Mix")
                (def seq-has-selection? () false)
                (def sbrowser-editor-name "")
                (defmacro aqua-slider-material () `(material :color (rgba 0.15 0.15 0.88 1.0)))
                (def custom-midi-fx-ui (fx) false)
                (def custom-audio-fx-ui (fx) false)
                (defstate selected-bus -1)
                "#,
            )
            .expect("install fx test helpers");
        register_test_delete_target_natives(&mut editor, 1);
        editor
            .runtime_mut()
            .eval_str(&custom_ui_source)
            .expect("load 909 custom instrument ui");
        editor.runtime_mut().eval_str(&src).expect("load fx lisp");
        editor
            .runtime_mut()
            .eval_str("(do (set! instrument-panel-tab 0) (set! instrument-mods-open false) (set! instrument-selected-mod-slot 1))")
            .expect("show synth tab before opening inline mods");
        editor.refresh_runtime_side_effects();
        if let Some(status) = editor.runtime_mut().take_status_message() {
            panic!("909 custom instrument fx lisp status after synth refresh: {status}");
        }

        let fx_id = editor
            .buffers
            .iter()
            .find(|buffer| buffer.name == "*fx*")
            .expect("fx lisp should create the *fx* buffer")
            .id;
        editor.set_active_buffer(fx_id);
        let synth_layout = editor
            .widget_layout()
            .expect("909 synth layout should build");
        assert_finite_layout_tree(&synth_layout);
        let synth_sweep =
            find_stable_key_suffix(&synth_layout, "pitch_sweep").expect("synth sweep knob wrapper");
        let synth_sweep_knob = find_descendant_widget(synth_sweep, "knob-number")
            .expect("synth sweep wrapper should contain its knob-number");
        assert!(
            synth_sweep_knob.rect.width > 0.0 && synth_sweep_knob.rect.height > 0.0,
            "synth sweep knob-number should be measured before opening mods: {:?}",
            synth_sweep_knob.rect
        );
        let _synth_frame =
            eseqlisp::ui::frame::build_tiled_render_frame_borderless(&mut editor, 220, 18);

        let mods_label =
            find_layout_node_by_text(&synth_layout, "mods").expect("mods header label");
        let click_col = mods_label.rect.col + mods_label.rect.width * 0.5;
        let click_row = mods_label.rect.row + mods_label.rect.height * 0.5;
        editor.handle_mouse_precise(
            mouse_event(
                MouseEventKind::Down(MouseButton::Left),
                click_col as u16,
                click_row as u16,
            ),
            0,
            0,
            220,
            18,
            click_col,
            click_row,
        );
        editor.refresh_runtime_side_effects();
        if let Some(status) = editor.runtime_mut().take_status_message() {
            panic!("909 custom instrument fx lisp status after mods refresh: {status}");
        }
        editor.set_active_buffer(fx_id);
        let _mods_frame =
            eseqlisp::ui::frame::build_tiled_render_frame_borderless(&mut editor, 220, 18);
        let layout = editor.widget_layout().expect("909 mod layout should build");
        assert_finite_layout_tree(&layout);
        let selector = find_layout_node_by_debug_name(&layout, "instrument-mod-selector")
            .expect("909 mods-open layout should include the inline mod selector");
        assert!(
            selector.rect.width > 0.0 && selector.rect.height > 0.0,
            "909 mods selector should be measured: {:?}",
            selector.rect
        );
        let selector_button_count = count_widget_type(selector, "button");
        assert!(selector_button_count >= 4, "{}", {
            let mut summaries = Vec::new();
            collect_layout_node_summaries(selector, &mut summaries);
            format!(
                "909 mods-open layout should render one selector button per mod slot; got {selector_button_count}\n{}",
                summaries.join("\n")
            )
        });
        let sweep = find_stable_key_suffix(&layout, "pitch_sweep").expect("sweep knob wrapper");
        assert!(
            sweep.rect.width > 0.0 && sweep.rect.height > 0.0,
            "sweep knob wrapper should be measured: {:?}",
            sweep.rect
        );
        let sweep_knob = find_descendant_widget(sweep, "knob-number")
            .expect("sweep wrapper should contain its knob-number while mods are open");
        assert!(
            sweep_knob.rect.width > 0.0 && sweep_knob.rect.height > 0.0,
            "sweep knob-number should be measured while mods are open: {:?}",
            sweep_knob.rect
        );
        for prop in [
            "base-value",
            "base-min",
            "base-max",
            "selected-mod-slot",
            "mod-range-0-slot",
            "mod-range-0-depth",
        ] {
            assert!(
                !matches!(sweep_knob.props.get(prop), None | Some(Value::Bool(false))),
                "sweep knob-number should carry active mod prop {prop:?} in mods tab; props={:?}",
                sweep_knob.props.keys().collect::<Vec<_>>()
            );
        }

        #[cfg(target_os = "macos")]
        {
            use eseqlisp::widget_render::{MetalPrimitive, WidgetViewport};

            let viewport = WidgetViewport {
                cell_w: 10.0,
                cell_h: 10.0,
                vp_w: 2200.0,
                vp_h: 180.0,
                time_seconds: 0.0,
                focused_widget_id: None,
                focused_branch: false,
                tile_content_rows: 18.0,
                scroll_top: 0.0,
                scroll_left: 0.0,
                inherited_hover: false,
            };
            let (primitives, _) =
                eseqlisp::widget_render::collect_metal_primitives(&layout, viewport, 0.0, 18);
            let knob_instances = primitives
                .iter()
                .filter(|primitive| {
                    matches!(
                        primitive,
                        MetalPrimitive::WidgetInstance { widget_type, .. }
                            if widget_type == "knob-number"
                    )
                })
                .count();
            let knob_text = primitives
                .iter()
                .filter_map(|primitive| match primitive {
                    MetalPrimitive::ProportionalText(text) => Some(text.text.as_str()),
                    _ => None,
                })
                .filter(|text| matches!(*text, "sweep" | "FM" | "body" | "drv"))
                .collect::<Vec<_>>();
            assert!(
                knob_instances >= 12,
                "909 mods-open primitive stream should include knob-number instances; got {knob_instances}"
            );
            assert!(
                knob_text.len() >= 4,
                "909 mods-open primitive stream should include knob labels; got {knob_text:?}"
            );
        }
    }

    #[test]
    fn metal_seq_fx_lisp_lays_out_prophet_6_inspired_condensed_controls() {
        fn find_stable_key_suffix<'a>(
            node: &'a eseqlisp::layout::LayoutNode,
            suffix: &str,
        ) -> Option<&'a eseqlisp::layout::LayoutNode> {
            if node
                .stable_key
                .as_deref()
                .is_some_and(|key| key.ends_with(suffix))
            {
                return Some(node);
            }
            node.children
                .iter()
                .find_map(|child| find_stable_key_suffix(child, suffix))
        }

        let src = std::fs::read_to_string("metal-seq-fx.lisp").expect("read fx lisp");
        let ui = std::fs::read_to_string("instruments/emulations/prophet-6-inspired/ui.lisp")
            .expect("read prophet-6-inspired ui");
        let custom_ui_source = build_custom_instrument_ui_source_with_overlay(Some((
            "emulations/prophet-6-inspired/".to_string(),
            "instruments/emulations/prophet-6-inspired/ui.lisp".to_string(),
            ui,
        )));

        let mut editor = eseqlisp::Editor::new(Runtime::new(), eseqlisp::EditorConfig::default());
        editor.set_layout_viewport(180, 18);
        editor.runtime_mut().register_reactive(
            "SEQ",
            vec![
                ("num-tracks", Value::Number(1.0)),
                ("compiling", Value::Bool(false)),
                ("available-effects", test_list(vec![])),
                ("available-builtin-effects", test_list(vec![])),
                ("available-midi-effects", test_list(vec![])),
                ("bus-names", test_list(vec![])),
                ("effects", test_list(vec![])),
                ("midi-effects", test_list(vec![])),
                (
                    "instrument-panel",
                    test_list(vec![Value::Map(prophet_6_inspired_test_instrument_map())]),
                ),
                ("bus-effects", test_list(vec![])),
            ],
            true,
        );
        editor
            .runtime_mut()
            .eval_str(
                r#"
                (def selected-bus-name () "Mix")
                (def seq-has-selection? () false)
                (def sbrowser-editor-name "")
                (defmacro aqua-slider-material () `(material :color (rgba 0.15 0.15 0.88 1.0)))
                (def custom-midi-fx-ui (fx) false)
                (def custom-audio-fx-ui (fx) false)
                (defstate selected-bus -1)
                "#,
            )
            .expect("install fx test helpers");
        register_test_delete_target_natives(&mut editor, 1);
        editor
            .runtime_mut()
            .eval_str(&custom_ui_source)
            .expect("load prophet-6-inspired custom instrument ui");
        editor.runtime_mut().eval_str(&src).expect("load fx lisp");
        editor.refresh_runtime_side_effects();
        if let Some(status) = editor.runtime_mut().take_status_message() {
            panic!("prophet-6-inspired custom instrument fx lisp status after refresh: {status}");
        }

        let fx_id = editor
            .buffers
            .iter()
            .find(|buffer| buffer.name == "*fx*")
            .expect("fx lisp should create the *fx* buffer")
            .id;
        editor.set_active_buffer(fx_id);
        editor.set_layout_viewport(180, 18);
        let layout = editor
            .widget_layout()
            .expect("prophet-6-inspired layout should build");
        let rendered = render_layout_cells(&layout, 180, 18);
        assert!(
            !rendered.contains("missing:"),
            "prophet-6-inspired condensed UI should not render missing-param diagnostics:\n{rendered}"
        );

        let instrument_panel = find_layout_node_by_debug_name(&layout, "instrument-panel")
            .expect("instrument panel layout node");
        let adsr_editor =
            find_layout_node_by_widget_type(&layout, "adsr-editor").expect("adsr editor");
        assert!(
            adsr_editor.rect.width > 8.0 && adsr_editor.rect.height > 2.0,
            "prophet-6-inspired ADSR editor should have a visible measured rect, got {:?}",
            adsr_editor.rect
        );

        for suffix in [
            "osc1_shape",
            "osc2_shape",
            "cutoff",
            "filter_drive",
            "lfo_rate_hz",
            "gain",
        ] {
            let node = find_stable_key_suffix(&layout, suffix)
                .unwrap_or_else(|| panic!("{suffix} control should be present"));
            assert!(
                node.rect.width > 1.0
                    && node.rect.height > 0.4
                    && node.rect.row >= instrument_panel.rect.row
                    && node.rect.row + node.rect.height
                        <= instrument_panel.rect.row + instrument_panel.rect.height,
                "{suffix} should have a finite visible rect inside the instrument panel, got {:?}; panel={:?}",
                node.rect,
                instrument_panel.rect
            );
        }
    }

    #[test]
    fn metal_seq_fx_lisp_lays_out_converted_monomachine_and_prophet_uis() {
        fn find_stable_key_suffix<'a>(
            node: &'a eseqlisp::layout::LayoutNode,
            suffix: &str,
        ) -> Option<&'a eseqlisp::layout::LayoutNode> {
            if node
                .stable_key
                .as_deref()
                .is_some_and(|key| key.ends_with(suffix))
            {
                return Some(node);
            }
            node.children
                .iter()
                .find_map(|child| find_stable_key_suffix(child, suffix))
        }

        fn dsp_param_names(path: &str) -> Vec<String> {
            std::fs::read_to_string(path)
                .unwrap_or_else(|error| panic!("read {path}: {error}"))
                .lines()
                .filter_map(|line| {
                    let line = line.trim_start();
                    line.strip_prefix("(param ")
                        .and_then(|rest| rest.split_whitespace().next())
                        .map(str::to_string)
                })
                .collect()
        }

        fn test_instrument_map_from_dsp(
            instrument_name: &str,
            dsp_path: &str,
        ) -> HashMap<String, Rc<RefCell<Value>>> {
            let mut inst = test_instrument_map();
            inst.insert(
                "name".to_string(),
                Rc::new(RefCell::new(Value::String(instrument_name.to_string()))),
            );
            inst.insert(
                "display-name".to_string(),
                Rc::new(RefCell::new(Value::String(
                    instrument_name.trim_end_matches('/').to_string(),
                ))),
            );

            let synth_params: Vec<Value> = std::iter::once(Value::Map(test_base_note_param_map(0)))
                .chain(
                    dsp_param_names(dsp_path)
                        .into_iter()
                        .enumerate()
                        .map(|(idx, name)| {
                            let (min, max) = if name.ends_with("_mode") {
                                (0.0, 3.0)
                            } else {
                                (-10000.0, 10000.0)
                            };
                            Value::Map(test_param_map(&name, idx + 1, 0.0, min, max))
                        }),
                )
                .collect();
            inst.insert(
                "synth".to_string(),
                Rc::new(RefCell::new(test_list(synth_params))),
            );
            inst
        }

        let cases = [
            (
                "monomachine/dpro/monomachine-digipro/",
                "instruments/monomachine/dpro/monomachine-digipro/dsp.lisp",
                "instruments/monomachine/dpro/monomachine-digipro/ui.lisp",
                vec!["morph", "cutoff", "gain"],
            ),
            (
                "monomachine/dpro/monomachine-dpro-bbox-v1/",
                "instruments/monomachine/dpro/monomachine-dpro-bbox-v1/dsp.lisp",
                "instruments/monomachine/dpro/monomachine-dpro-bbox-v1/ui.lisp",
                vec!["ptch", "cutoff", "gain"],
            ),
            (
                "monomachine/dpro/monomachine-dpro-dens-v1/",
                "instruments/monomachine/dpro/monomachine-dpro-dens-v1/dsp.lisp",
                "instruments/monomachine/dpro/monomachine-dpro-dens-v1/ui.lisp",
                vec!["wave", "chrl", "cutoff"],
            ),
            (
                "monomachine/dpro/monomachine-dpro-ddrw-v1/",
                "instruments/monomachine/dpro/monomachine-dpro-ddrw-v1/dsp.lisp",
                "instruments/monomachine/dpro/monomachine-dpro-ddrw-v1/ui.lisp",
                vec!["wav1", "time", "cutoff"],
            ),
            (
                "monomachine/dpro/monomachine-dpro-wave-v2/",
                "instruments/monomachine/dpro/monomachine-dpro-wave-v2/dsp.lisp",
                "instruments/monomachine/dpro/monomachine-dpro-wave-v2/ui.lisp",
                vec!["wave", "cutoff", "gain"],
            ),
            (
                "monomachine/fmplus/monomachine-fmplus/",
                "instruments/monomachine/fmplus/monomachine-fmplus/dsp.lisp",
                "instruments/monomachine/fmplus/monomachine-fmplus/ui.lisp",
                vec!["ratio_a", "tone", "gain"],
            ),
            (
                "monomachine/fmplus/monomachine-fmplus-par-v1/",
                "instruments/monomachine/fmplus/monomachine-fmplus-par-v1/dsp.lisp",
                "instruments/monomachine/fmplus/monomachine-fmplus-par-v1/ui.lisp",
                vec!["op1_frq", "op3_frq", "cutoff"],
            ),
            (
                "monomachine/fmplus/monomachine-fmplus-stat-v1/",
                "instruments/monomachine/fmplus/monomachine-fmplus-stat-v1/dsp.lisp",
                "instruments/monomachine/fmplus/monomachine-fmplus-stat-v1/ui.lisp",
                vec!["op1_frq", "op2_vol", "cutoff"],
            ),
            (
                "emulations/monomachine-sid/",
                "instruments/emulations/monomachine-sid/dsp.lisp",
                "instruments/emulations/monomachine-sid/ui.lisp",
                vec!["osc2_semi", "pulse_width", "cutoff"],
            ),
            (
                "monomachine/superwave/monomachine-superwave/",
                "instruments/monomachine/superwave/monomachine-superwave/dsp.lisp",
                "instruments/monomachine/superwave/monomachine-superwave/ui.lisp",
                vec!["saw_mix", "motion_rate", "cutoff"],
            ),
            (
                "emulations/prophet-6/",
                "instruments/emulations/prophet-6/dsp.lisp",
                "instruments/emulations/prophet-6/ui.lisp",
                vec!["osc1_shape", "osc2_mix", "cutoff"],
            ),
            (
                "emulations/prophet-6-emu/",
                "instruments/emulations/prophet-6-emu/dsp.lisp",
                "instruments/emulations/prophet-6-emu/ui.lisp",
                vec!["osc1_shape", "filter_drive", "gain"],
            ),
        ];

        let src = std::fs::read_to_string("metal-seq-fx.lisp").expect("read fx lisp");
        for (instrument_name, dsp_path, ui_path, expected_suffixes) in cases {
            let ui = std::fs::read_to_string(ui_path).unwrap_or_else(|error| {
                panic!("read {ui_path}: {error}");
            });
            let custom_ui_source = build_custom_instrument_ui_source_with_overlay(Some((
                instrument_name.to_string(),
                ui_path.to_string(),
                ui,
            )));

            let mut editor =
                eseqlisp::Editor::new(Runtime::new(), eseqlisp::EditorConfig::default());
            editor.set_layout_viewport(180, 18);
            editor.runtime_mut().register_reactive(
                "SEQ",
                vec![
                    ("num-tracks", Value::Number(1.0)),
                    ("compiling", Value::Bool(false)),
                    ("available-effects", test_list(vec![])),
                    ("available-builtin-effects", test_list(vec![])),
                    ("available-midi-effects", test_list(vec![])),
                    ("bus-names", test_list(vec![])),
                    ("effects", test_list(vec![])),
                    ("midi-effects", test_list(vec![])),
                    (
                        "instrument-panel",
                        test_list(vec![Value::Map(test_instrument_map_from_dsp(
                            instrument_name,
                            dsp_path,
                        ))]),
                    ),
                    ("bus-effects", test_list(vec![])),
                ],
                true,
            );
            editor
                .runtime_mut()
                .eval_str(
                    r#"
                    (def selected-bus-name () "Mix")
                    (def seq-has-selection? () false)
                    (def sbrowser-editor-name "")
                    (defmacro aqua-slider-material () `(material :color (rgba 0.15 0.15 0.88 1.0)))
                    (def custom-midi-fx-ui (fx) false)
                    (def custom-audio-fx-ui (fx) false)
                    (defstate selected-bus -1)
                    "#,
                )
                .expect("install fx test helpers");
            register_test_delete_target_natives(&mut editor, 1);
            editor
                .runtime_mut()
                .eval_str(&custom_ui_source)
                .unwrap_or_else(|error| panic!("load {instrument_name} custom UI: {error:?}"));
            editor.runtime_mut().eval_str(&src).expect("load fx lisp");
            editor.refresh_runtime_side_effects();
            if let Some(status) = editor.runtime_mut().take_status_message() {
                panic!(
                    "{instrument_name} custom instrument fx lisp status after refresh: {status}"
                );
            }

            let fx_id = editor
                .buffers
                .iter()
                .find(|buffer| buffer.name == "*fx*")
                .expect("fx lisp should create the *fx* buffer")
                .id;
            editor.set_active_buffer(fx_id);
            editor.set_layout_viewport(180, 18);
            let layout = editor
                .widget_layout()
                .unwrap_or_else(|| panic!("{instrument_name} layout should build"));
            let rendered = render_layout_cells(&layout, 180, 18);
            assert!(
                !rendered.contains("missing:"),
                "{instrument_name} should not render missing-param diagnostics:\n{rendered}"
            );
            let instrument_panel = find_layout_node_by_debug_name(&layout, "instrument-panel")
                .unwrap_or_else(|| panic!("{instrument_name} instrument panel layout node"));

            for suffix in expected_suffixes {
                let node = find_stable_key_suffix(&layout, suffix).unwrap_or_else(|| {
                    panic!("{instrument_name} should render a control ending in {suffix}")
                });
                assert!(
                    node.rect.width > 1.0
                        && node.rect.height > 0.4
                        && node.rect.row >= instrument_panel.rect.row
                        && node.rect.row + node.rect.height
                            <= instrument_panel.rect.row + instrument_panel.rect.height,
                    "{instrument_name} {suffix} should have a finite visible rect inside the instrument panel, got {:?}; panel={:?}",
                    node.rect,
                    instrument_panel.rect
                );
            }
        }
    }

    #[test]
    fn minimoog_lad2_filter_controls_select_filter_envelope() {
        fn find_stable_key_suffix<'a>(
            node: &'a eseqlisp::layout::LayoutNode,
            suffix: &str,
        ) -> Option<&'a eseqlisp::layout::LayoutNode> {
            if node
                .stable_key
                .as_deref()
                .is_some_and(|key| key.ends_with(suffix))
            {
                return Some(node);
            }
            node.children
                .iter()
                .find_map(|child| find_stable_key_suffix(child, suffix))
        }

        fn collect_stable_keys(node: &eseqlisp::layout::LayoutNode, keys: &mut Vec<String>) {
            if let Some(key) = &node.stable_key {
                keys.push(key.clone());
            }
            for child in &node.children {
                collect_stable_keys(child, keys);
            }
        }

        fn layout_node_contains_string(node: &eseqlisp::layout::LayoutNode, needle: &str) -> bool {
            node.props
                .values()
                .any(|value| value_contains_string(value, needle))
                || node
                    .children
                    .iter()
                    .any(|child| layout_node_contains_string(child, needle))
        }

        fn find_clickable_node_containing<'a>(
            node: &'a eseqlisp::layout::LayoutNode,
            needle: &str,
        ) -> Option<&'a eseqlisp::layout::LayoutNode> {
            if node.props.contains_key("on-click") && layout_node_contains_string(node, needle) {
                return Some(node);
            }
            node.children
                .iter()
                .find_map(|child| find_clickable_node_containing(child, needle))
        }

        let src = std::fs::read_to_string("metal-seq-fx.lisp").expect("read fx lisp");
        let minimoog_ui = std::fs::read_to_string("instruments/emulations/minimoog-lad2/ui.lisp")
            .expect("read ui");
        let initial_custom_ui_source = build_custom_instrument_ui_source_with_overlay(None);
        let minimoog_custom_ui_source = build_custom_instrument_ui_source_with_overlay(Some((
            "emulations/minimoog-lad2".to_string(),
            "instruments/emulations/minimoog-lad2/ui.lisp".to_string(),
            minimoog_ui,
        )));

        let mut editor = eseqlisp::Editor::new(Runtime::new(), eseqlisp::EditorConfig::default());
        editor.set_layout_viewport(160, 20);
        editor.runtime_mut().register_reactive(
            "SEQ",
            vec![
                ("num-tracks", Value::Number(1.0)),
                ("compiling", Value::Bool(false)),
                ("available-effects", test_list(vec![])),
                ("available-builtin-effects", test_list(vec![])),
                ("available-midi-effects", test_list(vec![])),
                ("bus-names", test_list(vec![])),
                ("effects", test_list(vec![])),
                ("midi-effects", test_list(vec![])),
                (
                    "instrument-panel",
                    test_list(vec![Value::Map(minimoog_lad2_test_instrument_map())]),
                ),
                ("bus-effects", test_list(vec![])),
            ],
            true,
        );
        editor
            .runtime_mut()
            .eval_str(
                r#"
                (def selected-bus-name () "Mix")
                (def seq-has-selection? () false)
                (def sbrowser-editor-name "")
                (defmacro aqua-slider-material () `(material :color (rgba 0.15 0.15 0.88 1.0)))
                (def custom-midi-fx-ui (fx) false)
                (def custom-audio-fx-ui (fx) false)
                (defstate selected-bus -1)
                "#,
            )
            .expect("install fx test helpers");
        register_test_delete_target_natives(&mut editor, 1);
        editor
            .runtime_mut()
            .eval_str(&initial_custom_ui_source)
            .expect("load initial empty custom instrument UI");
        editor.runtime_mut().eval_str(&src).expect("load fx lisp");
        editor
            .runtime_mut()
            .eval_str(&minimoog_custom_ui_source)
            .expect("load minimoog-lad2 custom instrument UI");
        editor.refresh_runtime_side_effects();
        if let Some(status) = editor.runtime_mut().take_status_message() {
            panic!("minimoog-lad2 custom instrument fx lisp status after refresh: {status}");
        }

        let initial_tree = editor
            .runtime_mut()
            .eval_str("(custom-instrument-synth-ui (nth SEQ.instrument-panel 0))")
            .expect("render initial minimoog-lad2 UI")
            .expect("initial UI value");
        assert!(value_contains_string(&initial_tree, "AMP ENV"));
        assert!(
            !value_contains_string(&initial_tree, "FILTER ENV"),
            "filter envelope should not be selected before filter controls are used"
        );

        let fx_id = editor
            .buffers
            .iter()
            .find(|buffer| buffer.name == "*fx*")
            .expect("fx lisp should create the *fx* buffer")
            .id;
        editor.set_active_buffer(fx_id);
        let layout = editor.widget_layout().expect("minimoog-lad2 layout");
        let mut layout_summaries = Vec::new();
        collect_layout_node_summaries(&layout, &mut layout_summaries);
        let mut stable_keys = Vec::new();
        collect_stable_keys(&layout, &mut stable_keys);
        let env_amount =
            find_stable_key_suffix(&layout, "filter_env_amount").unwrap_or_else(|| {
                panic!(
                "filter_env_amount knob; stable_keys={stable_keys:#?}; layout={layout_summaries:#?}"
            )
            });
        let callback = env_amount
            .props
            .get("on-change")
            .cloned()
            .expect("filter_env_amount on-change");
        editor
            .runtime_mut()
            .invoke(callback, vec![Value::Number(750.0)])
            .expect("invoke filter_env_amount on-change");
        let sections = editor
            .runtime_mut()
            .eval_str("custom-ui-selected-sections")
            .expect("read custom UI selected sections")
            .expect("selected sections value");

        let selected_tree = editor
            .runtime_mut()
            .eval_str("(custom-instrument-synth-ui (nth SEQ.instrument-panel 0))")
            .expect("render selected minimoog-lad2 UI")
            .expect("selected UI value");
        assert!(
            value_contains_string(&selected_tree, "FILTER ENV"),
            "using filter controls should select the filter envelope; sections={sections:?}; tree={selected_tree:?}"
        );
    }

    #[test]
    fn metal_seq_fx_lisp_lego_text_readout_uses_optical_vertical_center() {
        let src = std::fs::read_to_string("metal-seq-fx.lisp").expect("read fx lisp");
        let custom_ui_source = build_custom_instrument_ui_source_with_overlay(Some((
            "test-instrument/".to_string(),
            "test/ui.lisp".to_string(),
            r#"
            (defsynth-ui
              (ui-readout-block-small "SOURCE" (ui-accent-cyan)
                (ui-lego-text-row-4
                  (label "saw" :font-size 9.0 :color (ui-accent-cyan) :bg :transparent)
                  (label "<->" :font-size 9.0 :color :dim :bg :transparent)
                  (label "pulse" :font-size 9.0 :color (ui-accent-cyan) :bg :transparent)
                  (label "pw mod" :font-size 9.0 :color (ui-accent-blue) :bg :transparent))))
            "#
            .to_string(),
        )));

        let mut editor = eseqlisp::Editor::new(Runtime::new(), eseqlisp::EditorConfig::default());
        editor.set_layout_viewport(120, 18);
        editor.runtime_mut().register_reactive(
            "SEQ",
            vec![
                ("num-tracks", Value::Number(1.0)),
                ("compiling", Value::Bool(false)),
                ("available-effects", test_list(vec![])),
                ("available-builtin-effects", test_list(vec![])),
                ("available-midi-effects", test_list(vec![])),
                ("bus-names", test_list(vec![])),
                ("effects", test_list(vec![])),
                ("midi-effects", test_list(vec![])),
                (
                    "instrument-panel",
                    test_list(vec![Value::Map(test_instrument_map())]),
                ),
                ("bus-effects", test_list(vec![])),
            ],
            true,
        );
        editor
            .runtime_mut()
            .eval_str(
                r#"
                (def selected-bus-name () "Mix")
                (def seq-has-selection? () false)
                (def sbrowser-editor-name "")
                (defmacro aqua-slider-material () `(material :color (rgba 0.15 0.15 0.88 1.0)))
                (def custom-midi-fx-ui (fx) false)
                (def custom-audio-fx-ui (fx) false)
                (defstate selected-bus -1)
                "#,
            )
            .expect("install fx test helpers");
        register_test_delete_target_natives(&mut editor, 1);
        editor
            .runtime_mut()
            .eval_str(&custom_ui_source)
            .expect("load custom instrument ui");
        editor.runtime_mut().eval_str(&src).expect("load fx lisp");
        editor.refresh_runtime_side_effects();
        if let Some(status) = editor.runtime_mut().take_status_message() {
            panic!("lego text readout fx lisp status after refresh: {status}");
        }
        let fx_id = editor
            .buffers
            .iter()
            .find(|buffer| buffer.name == "*fx*")
            .expect("fx lisp should create the *fx* buffer")
            .id;
        editor.set_active_buffer(fx_id);
        editor.set_layout_viewport(120, 18);
        let layout = editor
            .widget_layout()
            .expect("lego text readout layout should build");

        let surface = find_layout_node_by_debug_name(&layout, "ui-lego-plain-surface")
            .expect("plain readout surface");
        let text_row =
            find_layout_node_by_debug_name(&layout, "ui-lego-text-row").expect("text row");
        let saw = find_layout_node_by_text(&layout, "saw").expect("saw label");

        let surface_center = surface.rect.row + surface.rect.height * 0.5;
        let text_visual_center = saw.rect.row + 0.5;
        let optical_lift = surface_center - text_visual_center;
        let left_inset = saw.rect.col - surface.rect.col;

        assert!(
            optical_lift > 0.13 && optical_lift < 0.22,
            "text row should be optically lifted within the readout surface; \
             surface={:?} text_row={:?} saw={:?} optical_lift={optical_lift:.3}",
            surface.rect,
            text_row.rect,
            saw.rect
        );
        assert!(
            left_inset > 0.75 && left_inset < 1.10,
            "text row should have a stable left inset within the readout surface; \
             surface={:?} saw={:?} left_inset={left_inset:.3}",
            surface.rect,
            saw.rect
        );
    }

    /// Regression: `ui-rack` with a real `ui-adsr-switch` plus base-note/many
    /// knobs (korg1's actual shape) should still produce multiple columns.
    #[test]
    fn metal_seq_fx_lisp_ui_rack_korg1_shape_renders_all_panels() {
        let src = std::fs::read_to_string("metal-seq-fx.lisp").expect("read fx lisp");
        let custom_ui_source = build_custom_instrument_ui_source_with_overlay(Some((
            "test-instrument/".to_string(),
            "test/ui.lisp".to_string(),
            r#"
            (defsynth-ui
              (ui-rack :breathe
                (list
                  (ui-panel "GLOBAL" 0
                    (h-stack :gap 0.2 (ui-param-knob "cutoff" "cut")))
                  (ui-panel "VCO 1" 0
                    (h-stack :gap 0.2 (ui-param-knob "cutoff" "saw")))
                  (ui-panel "VCO 2 / MIX" 0
                    (h-stack :gap 0.2 (ui-param-knob "cutoff" "vco2")))
                  (ui-panel "DIRT" 0
                    (h-stack :gap 0.2 (ui-param-knob "cutoff" "input"))))
                (ui-adsr-switch
                  0 "AMP ENV" "amp_attack" "amp_decay" "amp_sustain" "amp_release"
                  1 "AMP ENV" "amp_attack" "amp_decay" "amp_sustain" "amp_release")
                (list
                  (ui-panel "MS FILTER" 1
                    (h-stack :gap 0.2 (ui-param-knob "cutoff" "cut")))
                  (ui-panel "HP / SCREAM" 1
                    (h-stack :gap 0.2 (ui-param-knob "cutoff" "hp")))
                  (ui-panel "MOD" 0
                    (h-stack :gap 0.2 (ui-param-knob "cutoff" "rate")))
                  (ui-panel "NOISE / RING" 0
                    (h-stack :gap 0.2 (ui-param-knob "cutoff" "noise"))))))
            "#
            .to_string(),
        )));

        let mut editor = eseqlisp::Editor::new(Runtime::new(), eseqlisp::EditorConfig::default());
        editor.set_layout_viewport(140, 20);
        editor.runtime_mut().register_reactive(
            "SEQ",
            vec![
                ("num-tracks", Value::Number(1.0)),
                ("compiling", Value::Bool(false)),
                ("available-effects", test_list(vec![])),
                ("available-builtin-effects", test_list(vec![])),
                ("available-midi-effects", test_list(vec![])),
                ("bus-names", test_list(vec![])),
                ("effects", test_list(vec![])),
                ("midi-effects", test_list(vec![])),
                (
                    "instrument-panel",
                    test_list(vec![Value::Map(test_instrument_map())]),
                ),
                ("bus-effects", test_list(vec![])),
            ],
            true,
        );
        editor
            .runtime_mut()
            .eval_str(
                r#"
                (def selected-bus-name () "Mix")
                (def seq-has-selection? () false)
                (def sbrowser-editor-name "")
                (defmacro aqua-slider-material () `(material :color (rgba 0.15 0.15 0.88 1.0)))
                (def custom-midi-fx-ui (fx) false)
                (def custom-audio-fx-ui (fx) false)
                (defstate selected-bus -1)
                "#,
            )
            .expect("install fx test helpers");
        register_test_delete_target_natives(&mut editor, 1);
        editor
            .runtime_mut()
            .eval_str(&custom_ui_source)
            .expect("load custom instrument ui");
        editor.runtime_mut().eval_str(&src).expect("load fx lisp");
        editor.refresh_runtime_side_effects();
        if let Some(status) = editor.runtime_mut().take_status_message() {
            panic!("ui-rack fx lisp status after refresh: {status}");
        }
        let fx_id = editor
            .buffers
            .iter()
            .find(|buffer| buffer.name == "*fx*")
            .expect("fx lisp should create the *fx* buffer")
            .id;
        editor.set_active_buffer(fx_id);
        editor.set_layout_viewport(140, 20);
        editor.widget_layout().expect("ui-rack fx layout");
        let tree = editor
            .active_buffer()
            .widget_tree
            .as_ref()
            .expect("fx tree");

        for title in [
            "GLOBAL",
            "VCO 1",
            "VCO 2 / MIX",
            "DIRT",
            "MS FILTER",
            "HP / SCREAM",
            "MOD",
            "NOISE / RING",
        ] {
            assert!(
                value_contains_string(tree, title),
                "panel {title} missing — ui-rack-switch combo is broken"
            );
        }

        // Measure-confirm: walk the LAYOUT tree and verify all column v-stacks
        // are 31 cells wide (no v-stack should inflate to the rack width).
        let layout = editor.widget_layout().expect("re-layout");
        fn collect_panels(
            node: &eseqlisp::layout::LayoutNode,
            out: &mut Vec<eseqlisp::layout::Rect>,
        ) {
            // Custom panels are boxes wrapping a v-stack — record their rects.
            if node.widget_type == "v-stack" {
                if let Some(Value::Number(w)) = node.props.get("width") {
                    if (*w - 31.0).abs() < 0.01 {
                        out.push(node.rect);
                    }
                }
            }
            for child in &node.children {
                collect_panels(child, out);
            }
        }
        let mut column_rects = Vec::new();
        collect_panels(&layout, &mut column_rects);
        // Expect 4 columns (2 left + 2 right at :breathe with 4 panels each).
        assert!(
            column_rects.len() >= 4,
            "expected at least 4 v-stack columns at width 22, found {}",
            column_rects.len()
        );
        // Every column must be ≤ 32 cells wide (its declared 31 + a touch).
        for rect in &column_rects {
            assert!(
                rect.width < 32.5,
                "column v-stack inflated past its :width 31 — got {}",
                rect.width
            );
        }
        // Columns must be at DIFFERENT col positions (laid out side by side).
        let mut cols: Vec<f32> = column_rects.iter().map(|r| r.col).collect();
        cols.sort_by(|a, b| a.partial_cmp(b).unwrap());
        cols.dedup_by(|a, b| (*a - *b).abs() < 0.1);
        assert!(
            cols.len() >= 4,
            "columns are stacked at same x position — h-stack didn't lay them out side by side: {:?}",
            cols
        );
    }

    /// Regression: `ui-rack` should produce multiple side-by-side columns
    /// when an instrument provides a flat panel list. Caught a bug where
    /// only the first column was rendering (panels stretched to rack width).
    #[test]
    fn metal_seq_fx_lisp_ui_rack_renders_multiple_columns() {
        let src = std::fs::read_to_string("metal-seq-fx.lisp").expect("read fx lisp");
        let custom_ui_source = build_custom_instrument_ui_source_with_overlay(Some((
            "test-instrument/".to_string(),
            "test/ui.lisp".to_string(),
            r#"
            (defsynth-ui
              (ui-rack :breathe
                (list
                  (ui-panel "P1" 0 (h-stack :gap 0.2 (ui-param-knob "cutoff" "c")))
                  (ui-panel "P2" 0 (h-stack :gap 0.2 (ui-param-knob "cutoff" "c")))
                  (ui-panel "P3" 0 (h-stack :gap 0.2 (ui-param-knob "cutoff" "c")))
                  (ui-panel "P4" 0 (h-stack :gap 0.2 (ui-param-knob "cutoff" "c"))))
                (ui-adsr "AMP" "amp_attack" "amp_decay" "amp_sustain" "amp_release")
                (list
                  (ui-panel "P5" 0 (h-stack :gap 0.2 (ui-param-knob "cutoff" "c")))
                  (ui-panel "P6" 0 (h-stack :gap 0.2 (ui-param-knob "cutoff" "c"))))))
            "#
            .to_string(),
        )));

        let mut editor = eseqlisp::Editor::new(Runtime::new(), eseqlisp::EditorConfig::default());
        editor.set_layout_viewport(140, 20);
        editor.runtime_mut().register_reactive(
            "SEQ",
            vec![
                ("num-tracks", Value::Number(1.0)),
                ("compiling", Value::Bool(false)),
                ("available-effects", test_list(vec![])),
                ("available-builtin-effects", test_list(vec![])),
                ("available-midi-effects", test_list(vec![])),
                ("bus-names", test_list(vec![])),
                ("effects", test_list(vec![])),
                ("midi-effects", test_list(vec![])),
                (
                    "instrument-panel",
                    test_list(vec![Value::Map(test_instrument_map())]),
                ),
                ("bus-effects", test_list(vec![])),
            ],
            true,
        );
        editor
            .runtime_mut()
            .eval_str(
                r#"
                (def selected-bus-name () "Mix")
                (def seq-has-selection? () false)
                (def sbrowser-editor-name "")
                (defmacro aqua-slider-material () `(material :color (rgba 0.15 0.15 0.88 1.0)))
                (def custom-midi-fx-ui (fx) false)
                (def custom-audio-fx-ui (fx) false)
                (defstate selected-bus -1)
                "#,
            )
            .expect("install fx test helpers");
        register_test_delete_target_natives(&mut editor, 1);
        editor
            .runtime_mut()
            .eval_str(&custom_ui_source)
            .expect("load custom instrument ui");
        editor.runtime_mut().eval_str(&src).expect("load fx lisp");
        editor.refresh_runtime_side_effects();
        if let Some(status) = editor.runtime_mut().take_status_message() {
            panic!("ui-rack fx lisp status after refresh: {status}");
        }
        let fx_id = editor
            .buffers
            .iter()
            .find(|buffer| buffer.name == "*fx*")
            .expect("fx lisp should create the *fx* buffer")
            .id;
        editor.set_active_buffer(fx_id);
        editor.set_layout_viewport(140, 20);
        let _layout = editor.widget_layout().expect("ui-rack fx layout");
        let tree = editor
            .active_buffer()
            .widget_tree
            .as_ref()
            .expect("fx tree");

        // All six panel titles must appear somewhere in the tree.
        for title in ["P1", "P2", "P3", "P4", "P5", "P6"] {
            assert!(
                value_contains_string(tree, title),
                "panel {title} missing from widget tree — ui-rack did not splice columns"
            );
        }
    }

    #[test]
    fn metal_seq_mixer_clicks_dispatch_to_matching_track_and_bus_controls() {
        use eseqlisp::layout::LayoutNode;
        use std::sync::{Arc, Mutex};

        fn node_text(node: &LayoutNode) -> Option<String> {
            match node.props.get("text") {
                Some(Value::String(text)) => Some(text.clone()),
                _ => None,
            }
        }

        fn find_button_by_text<'a>(node: &'a LayoutNode, text: &str) -> Option<&'a LayoutNode> {
            if node.widget_type == "button" && node_text(node).as_deref() == Some(text) {
                return Some(node);
            }
            node.children
                .iter()
                .find_map(|child| find_button_by_text(child, text))
        }

        fn find_node_by_stable_key<'a>(node: &'a LayoutNode, key: &str) -> Option<&'a LayoutNode> {
            if node.stable_key.as_deref() == Some(key) {
                return Some(node);
            }
            node.children
                .iter()
                .find_map(|child| find_node_by_stable_key(child, key))
        }

        fn find_descendant_button_by_text<'a>(
            node: &'a LayoutNode,
            text: &str,
        ) -> Option<&'a LayoutNode> {
            if node.widget_type == "button" && node_text(node).as_deref() == Some(text) {
                return Some(node);
            }
            node.children
                .iter()
                .find_map(|child| find_descendant_button_by_text(child, text))
        }

        fn assert_reveal_command(commands: &[eseqlisp::host::HostCommand], expected_track: f64) {
            assert_eq!(commands.len(), 1, "expected one reveal command");
            match &commands[0] {
                eseqlisp::host::HostCommand::Custom { name, payload } => {
                    assert_eq!(name, "reveal-sequencer-track");
                    let Value::Map(payload) = payload else {
                        panic!("reveal-sequencer-track payload should be a dict: {payload:?}");
                    };
                    assert_eq!(
                        payload.get("track").map(|value| value.borrow().clone()),
                        Some(Value::Number(expected_track))
                    );
                }
                other => panic!("expected reveal-sequencer-track command, got {other:?}"),
            }
        }

        let src = std::fs::read_to_string("metal-seq-mixer-v2.lisp").expect("read mixer lisp");
        let calls = Arc::new(Mutex::new(Vec::<String>::new()));
        let mut editor = eseqlisp::Editor::new(Runtime::new(), eseqlisp::EditorConfig::default());
        editor.set_layout_viewport(120, 30);
        editor.runtime_mut().register_reactive(
            "SEQ",
            vec![
                (
                    "track-names",
                    test_list(vec![
                        Value::String("kick".to_string()),
                        Value::String("snare".to_string()),
                    ]),
                ),
                ("track-colors", test_track_colors()),
                (
                    "track-collapsed",
                    test_list(vec![Value::Bool(false), Value::Bool(false)]),
                ),
                (
                    "track-pattern-cells",
                    test_list(vec![
                        test_list(vec![
                            test_track_pattern_cell(1.0, true, true, false),
                            test_track_pattern_cell(2.0, false, false, false),
                        ]),
                        test_list(vec![
                            test_track_pattern_cell(3.0, true, false, true),
                            test_track_pattern_cell(4.0, false, true, true),
                        ]),
                    ]),
                ),
                ("num-tracks", Value::Number(2.0)),
                ("current-track", Value::Number(0.0)),
                ("track-selected-0", Value::Bool(true)),
                ("track-selected-1", Value::Bool(false)),
                ("delete-target-version", Value::Number(0.0)),
                (
                    "record-armed",
                    test_list(vec![Value::Bool(false), Value::Bool(false)]),
                ),
                (
                    "track-mutes",
                    test_list(vec![Value::Bool(false), Value::Bool(false)]),
                ),
                (
                    "track-solos",
                    test_list(vec![Value::Bool(false), Value::Bool(false)]),
                ),
                (
                    "track-muted-by-solo",
                    test_list(vec![Value::Bool(false), Value::Bool(false)]),
                ),
                (
                    "track-instrument-types",
                    test_list(vec![
                        Value::String("modulator".to_string()),
                        Value::String("instrument".to_string()),
                    ]),
                ),
                (
                    "track-mod-output-available",
                    test_list(vec![Value::Bool(true), Value::Bool(true)]),
                ),
                (
                    "mod-routes",
                    test_list(vec![Value::Map({
                        let mut map = std::collections::HashMap::new();
                        map.insert(
                            "source".to_string(),
                            Rc::new(RefCell::new(Value::Number(0.0))),
                        );
                        map.insert(
                            "dest".to_string(),
                            Rc::new(RefCell::new(Value::Number(1.0))),
                        );
                        map.insert(
                            "input".to_string(),
                            Rc::new(RefCell::new(Value::Number(2.0))),
                        );
                        map
                    })]),
                ),
                ("selected-mod-routes", test_list(vec![])),
                (
                    "track-volumes",
                    test_list(vec![Value::Number(1.0), Value::Number(1.0)]),
                ),
                (
                    "track-mixer-pans",
                    test_list(vec![Value::Number(0.0), Value::Number(0.0)]),
                ),
                (
                    "track-outputs",
                    test_list(vec![
                        Value::String("main".to_string()),
                        Value::String("main".to_string()),
                    ]),
                ),
                (
                    "track-output-options",
                    test_list(vec![
                        Value::String("main".to_string()),
                        Value::String("sends only".to_string()),
                        Value::String("Bus A".to_string()),
                        Value::String("Bus B".to_string()),
                    ]),
                ),
                (
                    "track-bus-sends",
                    test_list(vec![
                        test_list(vec![
                            test_track_bus_send(1, "Bus A", 0.0),
                            test_track_bus_send(2, "Bus B", 0.0),
                        ]),
                        test_list(vec![
                            test_track_bus_send(1, "Bus A", 0.0),
                            test_track_bus_send(2, "Bus B", 0.0),
                        ]),
                    ]),
                ),
                ("track-0-bus-1-send", Value::Number(0.0)),
                ("track-0-bus-2-send", Value::Number(0.0)),
                ("track-1-bus-1-send", Value::Number(0.0)),
                ("track-1-bus-2-send", Value::Number(0.0)),
                ("mixer-track-delete-target-0", Value::Bool(false)),
                ("mixer-track-delete-target-1", Value::Bool(false)),
                ("track-peak-0", Value::Number(0.0)),
                ("track-peak-1", Value::Number(0.0)),
                ("master-peak-l", Value::Number(0.0)),
                ("master-peak-r", Value::Number(0.0)),
                ("bus-peak-0", Value::Number(0.0)),
                ("bus-peak-1", Value::Number(0.0)),
                ("bus-peak-2", Value::Number(0.0)),
                (
                    "bus-names",
                    test_list(vec![
                        Value::String("Mix".to_string()),
                        Value::String("Bus A".to_string()),
                        Value::String("Bus B".to_string()),
                    ]),
                ),
                (
                    "bus-volumes",
                    test_list(vec![
                        Value::Number(1.0),
                        Value::Number(1.0),
                        Value::Number(1.0),
                    ]),
                ),
                (
                    "bus-mutes",
                    test_list(vec![
                        Value::Bool(false),
                        Value::Bool(false),
                        Value::Bool(false),
                    ]),
                ),
                (
                    "bus-solos",
                    test_list(vec![
                        Value::Bool(false),
                        Value::Bool(false),
                        Value::Bool(false),
                    ]),
                ),
            ],
            true,
        );
        editor
            .runtime_mut()
            .eval_str(
                "(defmacro aqua-slider-material () `(material :color (rgba 0.15 0.15 0.88 1.0)))",
            )
            .expect("install test slider material macro");
        editor
            .runtime_mut()
            .eval_str("(defstate selected-bus -1)")
            .expect("install shared mixer selection state");
        register_test_delete_target_natives(&mut editor, 2);
        set_test_track_pattern_cell_bindings(&mut editor, 0, 1, true, true, false, false);
        set_test_track_pattern_cell_bindings(&mut editor, 0, 2, false, false, false, false);
        set_test_track_pattern_cell_bindings(&mut editor, 1, 3, true, false, true, false);
        set_test_track_pattern_cell_bindings(&mut editor, 1, 4, false, true, true, false);

        for name in [
            "seq-toggle-record-arm",
            "seq-toggle-track-mute",
            "seq-toggle-track-solo",
            "seq-set-track",
            "seq-set-track-volume",
            "seq-set-track-pan",
            "seq-toggle-bus-mute",
            "seq-toggle-bus-solo",
            "seq-set-bus-volume",
            "seq-clear-selection",
        ] {
            let calls = Arc::clone(&calls);
            editor
                .runtime_mut()
                .register_native(name, move |args, _ctx| {
                    calls.lock().unwrap().push(format!("{name}:{args:?}"));
                    Ok(Value::Bool(true))
                });
        }

        editor
            .runtime_mut()
            .eval_str(&src)
            .expect("load mixer lisp");
        editor.refresh_runtime_side_effects();
        let mixer_id = editor
            .buffers
            .iter()
            .find(|buffer| buffer.name == "*mixer*")
            .expect("mixer lisp should create the *mixer* buffer")
            .id;
        editor.set_active_buffer(mixer_id);
        editor.active_buffer_mut().view_mode = eseqlisp::editor::ViewMode::UiOnly;
        editor.set_layout_viewport(120, 30);
        editor.refresh_runtime_side_effects();

        editor
            .runtime_mut()
            .eval_str("(mixer-v2-select-next-channel)")
            .expect("right arrow should select next track channel");
        assert_eq!(
            calls.lock().unwrap().last().map(String::as_str),
            Some("seq-set-track:[1]"),
            "next channel from track 1 should select track 2"
        );
        assert_eq!(
            editor
                .runtime_mut()
                .eval_str("selected-bus")
                .expect("read selected bus"),
            Some(Value::Number(-1.0)),
            "next track selection should keep selected-bus cleared"
        );
        editor
            .runtime_mut()
            .set_reactive("SEQ", "current-track", Value::Number(1.0));
        editor
            .runtime_mut()
            .set_reactive("SEQ", &track_selected_field(0), Value::Bool(false));
        editor
            .runtime_mut()
            .set_reactive("SEQ", &track_selected_field(1), Value::Bool(true));
        editor
            .runtime_mut()
            .eval_str("(mixer-v2-select-next-channel)")
            .expect("right arrow should select next mixer channel");
        assert_eq!(
            editor
                .runtime_mut()
                .eval_str("selected-bus")
                .expect("read selected bus"),
            Some(Value::Number(1.0)),
            "next channel from the last track should select Bus A in display order"
        );
        assert!(
            editor.drain_host_commands().is_empty(),
            "selecting a bus should not queue a host command"
        );
        editor
            .runtime_mut()
            .eval_str("(mixer-v2-delete-selected-track)")
            .expect("delete on bus selection should be handled safely");
        assert!(
            editor.drain_host_commands().is_empty(),
            "delete with a bus selected should not delete a track"
        );
        editor
            .runtime_mut()
            .eval_str("(mixer-v2-select-prev-channel)")
            .expect("left arrow should select previous mixer channel");
        assert_eq!(
            calls.lock().unwrap().last().map(String::as_str),
            Some("seq-set-track:[1]"),
            "previous channel from Bus A should return to track 2"
        );
        assert_eq!(
            editor
                .runtime_mut()
                .eval_str("selected-bus")
                .expect("read selected bus"),
            Some(Value::Number(-1.0)),
            "track selection should clear selected-bus"
        );
        editor
            .runtime_mut()
            .eval_str("(mixer-v2-select-track 0)")
            .expect("explicit mixer track selection should not claim delete target");
        assert_reveal_command(&editor.drain_host_commands(), 0.0);
        assert_eq!(
            editor
                .runtime_mut()
                .eval_str("(seq-active-delete-target-kind)")
                .expect("read delete target kind after background track select"),
            Some(Value::Bool(false)),
            "mixer track background selection should not claim mixer-track deletion"
        );
        editor
            .runtime_mut()
            .eval_str("(mixer-v2-select-track-delete-target 0)")
            .expect("explicit mixer track badge selection should claim delete target");
        assert_reveal_command(&editor.drain_host_commands(), 0.0);
        assert_eq!(
            editor
                .runtime_mut()
                .eval_str("(seq-active-delete-target-kind)")
                .expect("read delete target kind"),
            Some(Value::String("mixer-track".to_string()))
        );
        editor
            .runtime_mut()
            .eval_str("(mixer-v2-delete-selected-track)")
            .expect("delete on mixer delete target should queue host command");
        let commands = editor.drain_host_commands();
        assert_eq!(commands.len(), 1);
        match &commands[0] {
            eseqlisp::host::HostCommand::Custom { name, payload } => {
                assert_eq!(name, "delete-track");
                let Value::Map(payload) = payload else {
                    panic!("delete-track payload should be a dict: {payload:?}");
                };
                assert_eq!(
                    payload.get("track").map(|value| value.borrow().clone()),
                    Some(Value::Number(0.0))
                );
            }
            other => panic!("expected delete-track host command, got {other:?}"),
        }

        editor
            .runtime_mut()
            .eval_str(
                r#"(mixer-v2-drop-sample-on-track
                    (dict
                      :payload (dict :path "samples/kick.wav")
                      :target (dict :kind "track" :track 0)))"#,
            )
            .expect("drop sample on track");
        let commands = editor.drain_host_commands();
        assert_eq!(commands.len(), 1);
        match &commands[0] {
            eseqlisp::host::HostCommand::Custom { name, payload } => {
                assert_eq!(name, "load-sample-into-track");
                let Value::Map(payload) = payload else {
                    panic!("load-sample-into-track payload should be a dict: {payload:?}");
                };
                assert_eq!(
                    payload.get("track").map(|value| value.borrow().clone()),
                    Some(Value::Number(0.0))
                );
                assert_eq!(
                    payload.get("path").map(|value| value.borrow().clone()),
                    Some(Value::String("samples/kick.wav".to_string()))
                );
                assert_eq!(
                    payload
                        .get("preserve-browser-context")
                        .map(|value| value.borrow().clone()),
                    Some(Value::Bool(true))
                );
            }
            other => panic!("expected load-sample-into-track host command, got {other:?}"),
        }

        editor
            .runtime_mut()
            .eval_str(
                r#"(mixer-v2-drop-on-track
                    (dict
                      :drag-type "audio-effect"
                      :payload (dict :kind "builtin-audio-effect" :name "Filter")
                      :target (dict :kind "track" :track 1)))"#,
            )
            .expect("drop built-in audio effect on track");
        let commands = editor.drain_host_commands();
        assert_eq!(commands.len(), 1);
        match &commands[0] {
            eseqlisp::host::HostCommand::Custom { name, payload } => {
                assert_eq!(name, "add-builtin-effect-to-track");
                let Value::Map(payload) = payload else {
                    panic!("add-builtin-effect-to-track payload should be a dict: {payload:?}");
                };
                assert_eq!(
                    payload.get("track").map(|value| value.borrow().clone()),
                    Some(Value::Number(1.0))
                );
                assert_eq!(
                    payload.get("name").map(|value| value.borrow().clone()),
                    Some(Value::String("Filter".to_string()))
                );
            }
            other => panic!("expected add-builtin-effect-to-track host command, got {other:?}"),
        }

        editor
            .runtime_mut()
            .eval_str(
                r#"(mixer-v2-drop-on-track
                    (dict
                      :drag-type "audio-effect"
                      :payload (dict :kind "custom-audio-effect" :name "delayz")
                      :target (dict :kind "track" :track 1)))"#,
            )
            .expect("drop custom audio effect on track");
        let commands = editor.drain_host_commands();
        assert_eq!(commands.len(), 1);
        match &commands[0] {
            eseqlisp::host::HostCommand::Custom { name, payload } => {
                assert_eq!(name, "add-effect-to-track");
                let Value::Map(payload) = payload else {
                    panic!("add-effect-to-track payload should be a dict: {payload:?}");
                };
                assert_eq!(
                    payload.get("track").map(|value| value.borrow().clone()),
                    Some(Value::Number(1.0))
                );
                assert_eq!(
                    payload.get("name").map(|value| value.borrow().clone()),
                    Some(Value::String("delayz".to_string()))
                );
            }
            other => panic!("expected add-effect-to-track host command, got {other:?}"),
        }

        editor
            .runtime_mut()
            .eval_str(
                r#"(mixer-v2-drop-on-track
                    (dict
                      :drag-type "midi-effect"
                      :payload (dict :kind "midi-effect" :name "Arp")
                      :target (dict :kind "track" :track 1)))"#,
            )
            .expect("drop MIDI effect on track");
        let commands = editor.drain_host_commands();
        assert_eq!(commands.len(), 1);
        match &commands[0] {
            eseqlisp::host::HostCommand::Custom { name, payload } => {
                assert_eq!(name, "add-midi-fx-to-track");
                let Value::Map(payload) = payload else {
                    panic!("add-midi-fx-to-track payload should be a dict: {payload:?}");
                };
                assert_eq!(
                    payload.get("track").map(|value| value.borrow().clone()),
                    Some(Value::Number(1.0))
                );
                assert_eq!(
                    payload.get("name").map(|value| value.borrow().clone()),
                    Some(Value::String("Arp".to_string()))
                );
            }
            other => panic!("expected add-midi-fx-to-track host command, got {other:?}"),
        }

        editor
            .runtime_mut()
            .eval_str(
                r#"(mixer-v2-drop-sample-new-track
                    (dict :payload (dict :path "samples/kick.wav")))"#,
            )
            .expect("drop sample on new-track zone");
        let commands = editor.drain_host_commands();
        assert_eq!(commands.len(), 1);
        match &commands[0] {
            eseqlisp::host::HostCommand::Custom { name, payload } => {
                assert_eq!(name, "add-track-sample");
                let Value::Map(payload) = payload else {
                    panic!("add-track-sample payload should be a dict: {payload:?}");
                };
                assert_eq!(
                    payload.get("path").map(|value| value.borrow().clone()),
                    Some(Value::String("samples/kick.wav".to_string()))
                );
                assert_eq!(
                    payload
                        .get("preserve-browser-context")
                        .map(|value| value.borrow().clone()),
                    Some(Value::Bool(true))
                );
            }
            other => panic!("expected add-track-sample host command, got {other:?}"),
        }

        let layout = editor
            .runtime_mut()
            .current_layout
            .clone()
            .expect("mixer layout should be available");
        let send_knob = find_node_by_stable_key(&layout, "mixer-v2-track-0-send-1")
            .expect("track 1 Bus A send knob");
        assert!(matches!(
            send_knob.props.get("value"),
            Some(Value::ReactiveRef {
                namespace,
                field,
                ..
            }) if namespace == "SEQ" && field == "track-0-bus-1-send"
        ));
        assert!(
            send_knob.rect.width > 0.0 && send_knob.rect.height > 0.0,
            "track send knob should have a finite visible rect: {:?}",
            send_knob.rect
        );
        let send_knob_widget_id = send_knob.widget_id;
        let _ = editor.take_dirty_widget_ids();
        editor
            .runtime_mut()
            .set_reactive("SEQ", "track-0-bus-1-send", Value::Number(0.42));
        assert_eq!(
            editor.take_dirty_widget_ids(),
            vec![send_knob_widget_id],
            "send amount binding should dirty only the send knob widget"
        );
        let drop_zone = find_node_by_stable_key(&layout, "mixer-v2-sample-drop-zone")
            .expect("sample drop zone");
        assert!(
            drop_zone.rect.width > 0.0 && drop_zone.rect.height > 0.0,
            "sample drop zone should have a finite visible rect: {:?}",
            drop_zone.rect
        );
        let mod_out =
            find_node_by_stable_key(&layout, "mixer-v2-mod-out-0").expect("track mod out port");
        let custom_mod_out = find_node_by_stable_key(&layout, "mixer-v2-mod-out-1")
            .expect("custom track mod out port");
        let mod_in =
            find_node_by_stable_key(&layout, "mixer-v2-mod-in-1-0").expect("track mod in port");
        assert_eq!(
            custom_mod_out.props.get("active"),
            Some(&Value::Bool(true)),
            "custom track with a declared mod output should expose the mixer source port"
        );
        for input in 1..4 {
            let key = format!("mixer-v2-mod-in-1-{input}");
            let node = find_node_by_stable_key(&layout, &key)
                .unwrap_or_else(|| panic!("track mod in port {input}"));
            assert!(
                node.rect.width > 0.0 && node.rect.height > 0.0,
                "mod-port Ext{} should have finite visible rect: {:?}",
                input + 1,
                node.rect
            );
        }
        let ext3_in =
            find_node_by_stable_key(&layout, "mixer-v2-mod-in-1-2").expect("Ext3 mod in port");
        let Value::List(sources) = ext3_in
            .props
            .get("connected-sources")
            .expect("Ext3 input should expose connected sources")
        else {
            panic!(
                "Ext3 connected-sources prop should be a list: {:?}",
                ext3_in.props.get("connected-sources")
            );
        };
        assert_eq!(sources.len(), 1);
        assert_eq!(*sources[0].borrow(), Value::Number(0.0));
        assert!(
            mod_out.rect.width > 0.0
                && mod_out.rect.height > 0.0
                && mod_in.rect.width > 0.0
                && mod_in.rect.height > 0.0,
            "mod-port widgets should have finite visible rects: out={:?}, in={:?}",
            mod_out.rect,
            mod_in.rect
        );
        let pattern_cell = find_node_by_stable_key(&layout, "mixer-v2-track-pattern-cell-1-4")
            .expect("track 2 active override pattern cell");
        assert!(
            pattern_cell.rect.width > 0.0 && pattern_cell.rect.height > 0.0,
            "track pattern cell should have a finite visible rect: {:?}",
            pattern_cell.rect
        );
        assert_eq!(
            layout_prop_bool(pattern_cell, "active"),
            Some(true),
            "track pattern cell should use the bound active state"
        );
        assert_eq!(
            layout_prop_bool(pattern_cell, "override"),
            Some(true),
            "track pattern cell should use the bound override state"
        );
        editor
            .runtime_mut()
            .eval_str("(mixer-v2-launch-track-pattern 1 (nth (mixer-v2-track-pattern-cells 1) 1))")
            .expect("launch track pattern from mixer grid");
        let commands = editor.drain_host_commands();
        assert_eq!(commands.len(), 1);
        match &commands[0] {
            eseqlisp::host::HostCommand::Custom { name, payload } => {
                assert_eq!(name, "set-scene-cell");
                let Value::Map(payload) = payload else {
                    panic!("set-scene-cell payload should be a dict: {payload:?}");
                };
                assert_eq!(
                    payload.get("scene").map(|value| value.borrow().clone()),
                    Some(Value::Number(0.0))
                );
                assert_eq!(
                    payload.get("track").map(|value| value.borrow().clone()),
                    Some(Value::Number(1.0))
                );
                assert_eq!(
                    payload
                        .get("pattern-id")
                        .map(|value| value.borrow().clone()),
                    Some(Value::Number(4.0))
                );
            }
            other => panic!("expected set-scene-cell host command, got {other:?}"),
        }
        editor.runtime_mut().set_reactive(
            "SEQ",
            &track_pattern_cell_selected_field(1, 4),
            Value::Bool(true),
        );
        editor.refresh_runtime_side_effects();
        let layout_with_focused_pattern = editor
            .runtime_mut()
            .current_layout
            .clone()
            .expect("mixer layout should refresh after selecting a track pattern");
        let focused_pattern_cell = find_node_by_stable_key(
            &layout_with_focused_pattern,
            "mixer-v2-track-pattern-cell-1-4",
        )
        .expect("focused track pattern cell");
        assert_eq!(
            layout_prop_bool(focused_pattern_cell, "selected"),
            Some(true),
            "clicked track pattern should show keyboard focus immediately"
        );
        assert!(
            find_node_by_stable_key(&layout, "mixer-v2-track-pattern-clone-1").is_none(),
            "the mixer grid should not render a dedicated track-pattern clone cell"
        );
        editor
            .runtime_mut()
            .eval_str("(seq-clone-active-track-pattern)")
            .expect("clone selected track pattern from mixer grid");
        let commands = editor.drain_host_commands();
        assert_eq!(commands.len(), 1);
        match &commands[0] {
            eseqlisp::host::HostCommand::Custom { name, payload } => {
                assert_eq!(name, "clone-track-pattern");
                let Value::Map(payload) = payload else {
                    panic!("clone-track-pattern payload should be a dict: {payload:?}");
                };
                assert_eq!(
                    payload.get("track").map(|value| value.borrow().clone()),
                    Some(Value::Number(1.0))
                );
                assert_eq!(
                    payload
                        .get("pattern-id")
                        .map(|value| value.borrow().clone()),
                    Some(Value::Number(4.0))
                );
            }
            other => panic!("expected clone-track-pattern host command, got {other:?}"),
        }
        editor
            .runtime_mut()
            .eval_str("(mixer-v2-handle-key \"BS\" nil)")
            .expect("delete selected track pattern from mixer grid");
        let commands = editor.drain_host_commands();
        assert_eq!(commands.len(), 1);
        match &commands[0] {
            eseqlisp::host::HostCommand::Custom { name, payload } => {
                assert_eq!(name, "delete-track-pattern");
                let Value::Map(payload) = payload else {
                    panic!("delete-track-pattern payload should be a dict: {payload:?}");
                };
                assert_eq!(
                    payload.get("track").map(|value| value.borrow().clone()),
                    Some(Value::Number(1.0))
                );
                assert_eq!(
                    payload
                        .get("pattern-id")
                        .map(|value| value.borrow().clone()),
                    Some(Value::Number(4.0))
                );
            }
            other => panic!("expected delete-track-pattern host command, got {other:?}"),
        }
        editor
            .runtime_mut()
            .eval_str("(mixer-v2-select-track 0)")
            .expect("select track before clicking track control");
        assert_reveal_command(&editor.drain_host_commands(), 0.0);
        editor.runtime_mut().set_reactive(
            "SEQ",
            &track_pattern_cell_selected_field(1, 4),
            Value::Bool(false),
        );
        editor.refresh_runtime_side_effects();
        let layout_after_track_select = editor
            .runtime_mut()
            .current_layout
            .clone()
            .expect("mixer layout should refresh after selecting another track");
        let unfocused_pattern_cell = find_node_by_stable_key(
            &layout_after_track_select,
            "mixer-v2-track-pattern-cell-1-4",
        )
        .expect("previously focused track pattern cell");
        assert_eq!(
            layout_prop_bool(unfocused_pattern_cell, "selected"),
            Some(false),
            "selecting a different track target should clear the track-pattern focus border"
        );
        editor
            .runtime_mut()
            .set_reactive("SEQ", "delete-target-version", Value::Number(2.0));
        editor.runtime_mut().set_reactive(
            "SEQ",
            &mixer_track_delete_target_field(0),
            Value::Bool(true),
        );
        editor.runtime_mut().run_reactive_cycle();
        editor.refresh_runtime_side_effects();
        let layout_for_control_click = editor
            .runtime_mut()
            .current_layout
            .clone()
            .expect("mixer layout should be available before control click");
        let track_strip = find_node_by_stable_key(&layout_for_control_click, "mixer-v2-track-0")
            .expect("track mixer strip");
        let track_select =
            find_descendant_button_by_text(track_strip, "1").expect("track mute button");
        calls.lock().unwrap().clear();
        let track_select_callback = track_select
            .props
            .get("on-click")
            .cloned()
            .expect("track mute button on-click");
        editor
            .runtime_mut()
            .invoke(track_select_callback, vec![Value::Bool(true)])
            .expect("invoke track mute click");
        assert!(
            editor.drain_host_commands().is_empty(),
            "mixer control clicks should not request sequencer reveal"
        );
        assert_eq!(
            calls.lock().unwrap().last().map(String::as_str),
            Some("seq-toggle-track-mute:[0]")
        );
        assert_eq!(
            editor
                .runtime_mut()
                .eval_str("(seq-active-delete-target-kind)")
                .expect("read delete target after control click"),
            Some(Value::Bool(false)),
            "track controls should clear active delete target instead of claiming track deletion"
        );
        let track_label =
            find_node_by_stable_key(&layout, "mixer-v2-track-label-0").expect("track label box");
        let track_label_callback = track_label
            .props
            .get("on-click")
            .cloned()
            .expect("track label on-click");
        editor
            .runtime_mut()
            .invoke(track_label_callback, vec![Value::Bool(true)])
            .expect("invoke track label click");
        assert_reveal_command(&editor.drain_host_commands(), 0.0);
        assert_eq!(
            calls.lock().unwrap().last().map(String::as_str),
            Some("seq-set-track:[0]")
        );
        assert_eq!(
            editor
                .runtime_mut()
                .eval_str("(seq-active-delete-target-kind)")
                .expect("read delete target after track label click"),
            Some(Value::String("mixer-track".to_string())),
            "track-name badge should claim mixer track deletion"
        );
        editor
            .runtime_mut()
            .set_reactive("SEQ", "delete-target-version", Value::Number(2.0));
        editor.runtime_mut().set_reactive(
            "SEQ",
            &mixer_track_delete_target_field(0),
            Value::Bool(true),
        );
        editor.refresh_runtime_side_effects();
        let selected_layout = editor
            .widget_layout()
            .expect("selected mixer layout should be available");
        let selected_track_label =
            find_node_by_stable_key(&selected_layout, "mixer-v2-track-label-0")
                .expect("selected track label box");
        assert!(
            selected_track_label.rect.width > 0.0 && selected_track_label.rect.height > 0.0,
            "active mixer delete-target badge should have a finite visible rect: {:?}",
            selected_track_label.rect
        );
        assert_eq!(
            layout_prop_bool(selected_track_label, "selected"),
            Some(true),
            "active mixer delete-target badge should use the bound selected state"
        );
        assert_eq!(
            selected_track_label.props.get("selected-background-color"),
            Some(&Value::Keyword("fx-panel-header-selected-bg".to_string())),
            "active mixer delete-target badge should use the selected FX header color path"
        );

        let bus_a_strip =
            find_node_by_stable_key(&layout, "mixer-v2-bus-1").expect("Bus A mixer strip");
        find_descendant_button_by_text(bus_a_strip, "A").expect("Bus A mute button");
        let bus_a_solo =
            find_descendant_button_by_text(bus_a_strip, "S").expect("Bus A solo button");
        let bus_a_label =
            find_descendant_button_by_text(bus_a_strip, "Bus A").expect("Bus A label button");

        calls.lock().unwrap().clear();
        let bus_label_callback = bus_a_label
            .props
            .get("on-click")
            .cloned()
            .expect("Bus A label on-click");
        editor
            .runtime_mut()
            .invoke(bus_label_callback, vec![Value::Bool(true)])
            .expect("invoke Bus A label click");
        editor.runtime_mut().run_reactive_cycle();
        editor.refresh_runtime_side_effects();
        assert_eq!(
            editor
                .runtime_mut()
                .eval_str("selected-bus")
                .expect("read selected bus after Bus A label click"),
            Some(Value::Number(1.0)),
            "Bus A label click should select Bus A"
        );
        assert!(
            editor.drain_host_commands().is_empty(),
            "bus selection should not queue a host command"
        );
        assert_eq!(
            calls.lock().unwrap().last().map(String::as_str),
            Some("seq-clear-selection:[]"),
            "bus selection should clear step selection without selecting a track"
        );
        let bus_selected_layout = editor
            .widget_layout()
            .expect("mixer layout should be available after bus selection");
        let track_strip_after_bus_select =
            find_node_by_stable_key(&bus_selected_layout, "mixer-v2-track-0")
                .expect("track strip after bus selection");
        assert!(
            matches!(
                track_strip_after_bus_select.props.get("selected"),
                Some(Value::ReactiveRef { namespace, field, .. })
                    if namespace == "SEQ" && field == &track_selected_field(0)
            ),
            "selecting a bus should not replace track selected bindings with literals"
        );

        calls.lock().unwrap().clear();
        let bus_solo_callback = bus_a_solo
            .props
            .get("on-click")
            .cloned()
            .expect("Bus A solo on-click");
        editor
            .runtime_mut()
            .invoke(bus_solo_callback, vec![Value::Bool(true)])
            .expect("invoke Bus A solo click");
        editor.runtime_mut().run_reactive_cycle();
        editor.refresh_runtime_side_effects();
        assert_eq!(
            editor
                .runtime_mut()
                .eval_str("selected-bus")
                .expect("read selected bus after Bus A solo click"),
            Some(Value::Number(1.0)),
            "Bus A solo click should keep Bus A selected"
        );
        assert_eq!(
            calls.lock().unwrap().last().map(String::as_str),
            Some("seq-toggle-bus-solo:[1]"),
            "Bus A solo button should only toggle Bus A solo after selecting the bus"
        );

        #[cfg(target_os = "macos")]
        {
            use eseqlisp::widget_render::{MetalPrimitive, WidgetViewport};

            let viewport = WidgetViewport {
                cell_w: 10.0,
                cell_h: 10.0,
                vp_w: 1200.0,
                vp_h: 300.0,
                time_seconds: 0.0,
                focused_widget_id: None,
                focused_branch: false,
                tile_content_rows: 30.0,
                scroll_top: 0.0,
                scroll_left: 0.0,
                inherited_hover: false,
            };

            fn instance_center_cells(
                instance: &eseqlisp::widget_render::WidgetInstance,
                viewport: WidgetViewport,
            ) -> (f32, f32) {
                let center_ndc_x = (instance.ndc_min[0] + instance.ndc_max[0]) * 0.5;
                let center_ndc_y = (instance.ndc_min[1] + instance.ndc_max[1]) * 0.5;
                let px_x = ((center_ndc_x + 1.0) * 0.5) * viewport.vp_w;
                let px_y = ((1.0 - center_ndc_y) * 0.5) * viewport.vp_h;
                (px_x / viewport.cell_w, px_y / viewport.cell_h)
            }

            fn rect_contains_node(rect: eseqlisp::layout::Rect, col: f32, row: f32) -> bool {
                col >= rect.col
                    && col <= rect.col + rect.width
                    && row >= rect.row
                    && row <= rect.row + rect.height
            }

            let (full_primitives, _) =
                eseqlisp::widget_render::collect_metal_primitives(&layout, viewport, 0.0, 30);
            let full_bus_button_backgrounds = full_primitives
                .iter()
                .filter(|primitive| {
                    let MetalPrimitive::WidgetInstance {
                        widget_type,
                        instance,
                        is_background: true,
                    } = primitive
                    else {
                        return false;
                    };
                    if widget_type != "button" {
                        return false;
                    }
                    let (col, row) = instance_center_cells(instance, viewport);
                    rect_contains_node(bus_a_strip.rect, col, row)
                })
                .count();
            assert_eq!(
                full_bus_button_backgrounds, 3,
                "full mixer Metal primitive stream should dispatch styled button backgrounds for Bus A mute, solo, and label"
            );

            let bus_primitives =
                eseqlisp::widget_render::collect_metal_primitives(bus_a_strip, viewport, 0.0, 30).0;
            let subtree_bus_button_backgrounds = bus_primitives
                .iter()
                .filter(|primitive| {
                    matches!(
                        primitive,
                        MetalPrimitive::WidgetInstance {
                            widget_type,
                            is_background: true,
                            ..
                        } if widget_type == "button"
                    )
                })
                .count();
            assert_eq!(
                subtree_bus_button_backgrounds, 3,
                "Bus A strip should render styled button backgrounds for mute, solo, and label"
            );
        }
    }

    #[test]
    fn metal_seq_piano_roll_lisp_loads() {
        let src =
            std::fs::read_to_string("metal-seq-piano-roll.lisp").expect("read piano roll lisp");
        let tokens = Parser::new(src.clone())
            .parse()
            .expect("tokenize metal-seq-piano-roll.lisp");
        let mut pos = 0;
        while pos < tokens.len() {
            if let Err(err) = parse_expression_at(&tokens, &mut pos) {
                let start = pos.saturating_sub(8);
                let end = (pos + 8).min(tokens.len());
                panic!(
                    "parse metal-seq-piano-roll.lisp at token {pos}: {err:?}\ncontext: {:?}",
                    &tokens[start..end]
                );
            }
        }
        ASTParser::new(tokens)
            .parse()
            .expect("parse metal-seq-piano-roll.lisp");

        let mut editor = eseqlisp::Editor::new(Runtime::new(), eseqlisp::EditorConfig::default());
        editor.runtime_mut().register_reactive(
            "SEQ",
            vec![
                ("playhead", Value::Number(0.0)),
                ("current-track", Value::Number(0.0)),
                ("track-colors", test_track_colors()),
                ("tp-num-steps", Value::Number(16.0)),
                ("piano-roll-lanes", build_piano_roll_lanes_value()),
                ("piano-roll-items", Value::List(vec![])),
                ("piano-roll-selection", Value::List(vec![])),
            ],
            true,
        );
        editor
            .runtime_mut()
            .register_native("seq-piano-roll-action", |_args, _ctx| Ok(Value::Bool(true)));
        editor
            .runtime_mut()
            .eval_str(&src)
            .expect("load piano roll lisp");
        editor.refresh_runtime_side_effects();
        assert!(
            editor
                .buffers
                .iter()
                .any(|buffer| buffer.name == "*piano-roll*"),
            "piano roll lisp should create the *piano-roll* buffer"
        );
        let piano_buffer_id = editor
            .buffers
            .iter()
            .find(|buffer| buffer.name == "*piano-roll*")
            .expect("piano roll buffer")
            .id;
        editor.set_active_buffer(piano_buffer_id);
        let layout = editor
            .widget_layout()
            .expect("piano roll should have a widget layout");
        assert_eq!(layout.widget_type, "timeline");
        let expected_track_color = test_number_list(&[0.96, 0.28, 0.52]);
        assert_eq!(
            layout.props.get("item-color"),
            Some(&expected_track_color),
            "piano roll notes should use the current track color"
        );
        assert_eq!(
            layout.props.get("loop-color"),
            Some(&expected_track_color),
            "piano roll loop selector should use the current track color"
        );
        assert_eq!(
            layout.props.get("snap"),
            Some(&Value::Number(1.0)),
            "piano roll move snapping should use step grid lines"
        );
        assert_eq!(
            layout.props.get("move-snap-mode"),
            Some(&Value::Keyword("alignment-helper".to_string())),
            "piano roll should preserve sub-step drag offsets until the next grid line"
        );
        assert_eq!(
            layout.props.get("resize-snap-mode"),
            Some(&Value::Keyword("alignment-helper".to_string())),
            "piano roll duration resize should use the same alignment helper as note moves"
        );
        assert_eq!(
            layout.props.get("min-duration"),
            Some(&Value::Number(0.03125)),
            "piano roll duration resize should not be clamped to the visible grid"
        );
        assert_eq!(
            layout.props.get("create-duration"),
            Some(&Value::Number(1.0)),
            "piano roll should default new notes to one step"
        );
        editor
            .runtime_mut()
            .eval_str("(piano-roll-action (dict :type :finish-create-item :start 2 :end 4.5))")
            .expect("record created duration");
        editor.refresh_runtime_side_effects();
        let layout = editor
            .widget_layout()
            .expect("piano roll should still have a widget layout");
        assert_eq!(
            layout.props.get("create-duration"),
            Some(&Value::Number(2.5)),
            "piano roll should reuse the last created note duration"
        );
        editor
            .runtime_mut()
            .eval_str("(piano-roll-action (dict :type :resize-item-absolute :duration 3.25))")
            .expect("record resized duration");
        editor.refresh_runtime_side_effects();
        let layout = editor
            .widget_layout()
            .expect("piano roll should still have a widget layout");
        assert_eq!(
            layout.props.get("create-duration"),
            Some(&Value::Number(3.25)),
            "piano roll should reuse the last edited note duration"
        );
        editor
            .runtime_mut()
            .eval_str("(piano-roll-action (dict :type :clear-selection :time 4.5))")
            .expect("record cursor time");
        editor.refresh_runtime_side_effects();
        let layout = editor
            .widget_layout()
            .expect("piano roll should still have a widget layout");
        assert_eq!(
            layout.props.get("cursor-time"),
            Some(&Value::Number(4.5)),
            "piano roll should expose the last clicked cursor time to the timeline"
        );
        let _ = editor.runtime_mut().take_pending_buffer_widget_trees();
        editor
            .runtime_mut()
            .set_reactive("SEQ", "playhead", Value::Number(4.0));
        editor.runtime_mut().run_reactive_cycle();
        assert!(
            editor
                .runtime_mut()
                .take_pending_buffer_widget_trees()
                .is_empty(),
            "bound piano-roll playhead updates must not enqueue timeline widget tree rebuilds"
        );
        editor
            .runtime_mut()
            .eval_str("(set! piano-roll-view-duration 8)")
            .expect("set piano roll duration");
        editor
            .runtime_mut()
            .eval_str("(set! piano-roll-lane-height 1)")
            .expect("set piano roll lane height");
        editor
            .runtime_mut()
            .eval_str("(piano-roll-action (dict :type :zoom-view :anchor-time 4 :factor 2))")
            .expect("zoom piano roll");
        assert_eq!(
            editor
                .runtime_mut()
                .eval_str("piano-roll-lane-height")
                .expect("read piano roll lane height after x zoom"),
            Some(Value::Number(1.0))
        );
        editor
            .runtime_mut()
            .eval_str("(set! piano-roll-view-duration 8)")
            .expect("reset piano roll duration");
        editor
            .runtime_mut()
            .eval_str("(piano-roll-action (dict :type :scroll-view :delta-time 100))")
            .expect("scroll piano roll");
        assert_eq!(
            editor
                .runtime_mut()
                .eval_str("piano-roll-view-start")
                .expect("read piano roll view start"),
            Some(Value::Number(12.0))
        );
    }

    #[test]
    fn piano_roll_resize_updates_only_target_chord_note_duration() {
        let state = Arc::new(SequencerState::new(1, vec![]));
        let selection = Arc::new(Mutex::new(HashSet::new()));
        let move_state = Arc::new(Mutex::new(None));
        let track = 0;
        let step = 2;
        state.pattern.chord_data[track].add_note_with_duration(step, 0.0, 1.0);
        state.pattern.chord_data[track].add_note_with_duration(step, 7.0, 4.0);
        state.pattern.patterns[track].set_step_active(step, true);
        state.pattern.step_data[track].set(step, StepParam::Duration, 4.0);

        let action = map_value([
            ("type", Value::Keyword("resize-item-absolute".to_string())),
            ("id", Value::Number(piano_roll_item_id(step, 0) as f64)),
            ("time", Value::Number(4.0)),
        ]);

        apply_piano_roll_action(&state, track, &selection, &move_state, &action)
            .expect("resize action");

        assert_eq!(state.pattern.chord_data[track].get_duration(step, 0), 2.0);
        assert_eq!(state.pattern.chord_data[track].get_duration(step, 1), 4.0);
    }

    #[test]
    fn piano_roll_resize_selected_notes_by_same_delta() {
        let state = Arc::new(SequencerState::new(1, vec![]));
        let selection = Arc::new(Mutex::new(HashSet::new()));
        let move_state = Arc::new(Mutex::new(None));
        let track = 0;
        state.pattern.patterns[track].set_step_active(2, true);
        state.pattern.step_data[track].set(2, StepParam::Duration, 1.0);
        state.pattern.patterns[track].set_step_active(5, true);
        state.pattern.step_data[track].set(5, StepParam::Duration, 2.0);

        let action = map_value([
            ("type", Value::Keyword("resize-item-absolute".to_string())),
            ("id", Value::Number(piano_roll_item_id(2, 0) as f64)),
            (
                "ids",
                list_value(vec![
                    Value::Number(piano_roll_item_id(2, 0) as f64),
                    Value::Number(piano_roll_item_id(5, 0) as f64),
                ]),
            ),
            ("time", Value::Number(4.0)),
            ("duration-delta", Value::Number(1.0)),
        ]);

        apply_piano_roll_action(&state, track, &selection, &move_state, &action)
            .expect("resize action");

        assert_eq!(
            state.pattern.step_data[track].get(2, StepParam::Duration),
            2.0
        );
        assert_eq!(
            state.pattern.step_data[track].get(5, StepParam::Duration),
            3.0
        );
    }

    #[test]
    fn piano_roll_create_preserves_fractional_start_as_delay() {
        let state = Arc::new(SequencerState::new(1, vec![]));
        let selection = Arc::new(Mutex::new(HashSet::new()));
        let move_state = Arc::new(Mutex::new(None));
        let track = 0;

        let action = map_value([
            ("type", Value::Keyword("finish-create-item".to_string())),
            ("lane", Value::Number(48.0)),
            ("start", Value::Number(2.5)),
            ("end", Value::Number(3.5)),
        ]);

        apply_piano_roll_action(&state, track, &selection, &move_state, &action)
            .expect("create action");

        assert!(state.pattern.patterns[track].is_active(2));
        assert!(!state.pattern.patterns[track].is_active(3));
        assert_eq!(
            state.pattern.step_data[track].get(2, StepParam::Duration),
            1.0
        );
        assert_eq!(state.pattern.chord_data[track].count(2), 1);
        assert_eq!(state.pattern.chord_data[track].get_delay(2, 0), 0.5);
        assert_eq!(state.pattern.step_data[track].get(2, StepParam::Delay), 0.0);

        let items = build_piano_roll_items_value(&state, track, &selection);
        let Value::List(items) = items else {
            panic!("expected item list");
        };
        let Value::Map(item) = items[0].borrow().clone() else {
            panic!("expected item map");
        };
        assert_eq!(
            item.get("label").map(|value| value.borrow().clone()),
            Some(Value::String("C4 +0.50".to_string()))
        );
    }

    #[test]
    fn piano_roll_move_absolute_preserves_fractional_start_as_delay() {
        let state = Arc::new(SequencerState::new(1, vec![]));
        let selection = Arc::new(Mutex::new(HashSet::new()));
        let move_state = Arc::new(Mutex::new(None));
        let track = 0;

        state.pattern.patterns[track].set_step_active(2, true);
        state.pattern.step_data[track].set(2, StepParam::Duration, 1.0);
        state.pattern.step_data[track].set(2, StepParam::Transpose, 0.0);

        let action = map_value([
            ("type", Value::Keyword("move-items-absolute".to_string())),
            (
                "ids",
                list_value(vec![Value::Number(piano_roll_item_id(2, 0) as f64)]),
            ),
            ("anchor-id", Value::Number(piano_roll_item_id(2, 0) as f64)),
            ("start", Value::Number(4.375)),
            ("lane", Value::Number(48.0)),
        ]);

        apply_piano_roll_action(&state, track, &selection, &move_state, &action)
            .expect("move action");

        assert!(!state.pattern.patterns[track].is_active(2));
        assert!(state.pattern.patterns[track].is_active(4));
        assert_eq!(
            state.pattern.step_data[track].get(4, StepParam::Duration),
            1.0
        );
        assert_eq!(state.pattern.chord_data[track].count(4), 1);
        assert_eq!(state.pattern.chord_data[track].get_delay(4, 0), 0.375);
        assert_eq!(state.pattern.step_data[track].get(4, StepParam::Delay), 0.0);
    }

    #[test]
    fn piano_roll_allows_strummed_notes_on_one_step() {
        let state = Arc::new(SequencerState::new(1, vec![]));
        let selection = Arc::new(Mutex::new(HashSet::new()));
        let move_state = Arc::new(Mutex::new(None));
        let track = 0;

        for (lane, start) in [(48.0, 3.0), (44.0, 3.125), (41.0, 3.25)] {
            let action = map_value([
                ("type", Value::Keyword("finish-create-item".to_string())),
                ("lane", Value::Number(lane)),
                ("start", Value::Number(start)),
                ("end", Value::Number(start + 1.0)),
            ]);
            apply_piano_roll_action(&state, track, &selection, &move_state, &action)
                .expect("create action");
        }

        assert_eq!(state.pattern.chord_data[track].count(3), 3);
        assert_eq!(state.pattern.chord_data[track].get_delay(3, 0), 0.0);
        assert_eq!(state.pattern.chord_data[track].get_delay(3, 1), 0.125);
        assert_eq!(state.pattern.chord_data[track].get_delay(3, 2), 0.25);
    }

    #[test]
    fn piano_roll_copy_paste_preserves_fractional_offsets_at_cursor() {
        let state = Arc::new(SequencerState::new(1, vec![]));
        let selection = Arc::new(Mutex::new(HashSet::new()));
        let move_state = Arc::new(Mutex::new(None));
        let clipboard = new_piano_roll_clipboard();
        let track = 0;
        let step = 2;

        state.pattern.chord_data[track].add_note_with_timing(step, 0.0, 1.0, 0.0);
        state.pattern.chord_data[track].add_note_with_timing(step, 4.0, 1.5, 0.25);
        state.pattern.patterns[track].set_step_active(step, true);
        state.pattern.step_data[track].set(step, StepParam::Duration, 1.5);

        let copy = map_value([
            ("type", Value::Keyword("copy-items".to_string())),
            (
                "ids",
                list_value(vec![
                    Value::Number(piano_roll_item_id(step, 0) as f64),
                    Value::Number(piano_roll_item_id(step, 1) as f64),
                ]),
            ),
        ]);
        apply_piano_roll_action_with_clipboard(
            &state,
            track,
            &selection,
            &move_state,
            &clipboard,
            &copy,
        )
        .expect("copy action");

        let paste = map_value([
            ("type", Value::Keyword("paste-items".to_string())),
            ("time", Value::Number(4.5)),
        ]);
        apply_piano_roll_action_with_clipboard(
            &state,
            track,
            &selection,
            &move_state,
            &clipboard,
            &paste,
        )
        .expect("paste action");

        assert!(state.pattern.patterns[track].is_active(step));
        assert!(state.pattern.patterns[track].is_active(4));
        assert_eq!(state.pattern.chord_data[track].count(4), 2);
        assert_eq!(state.pattern.chord_data[track].get_delay(4, 0), 0.5);
        assert_eq!(state.pattern.chord_data[track].get_delay(4, 1), 0.75);
        assert_eq!(state.pattern.chord_data[track].get_duration(4, 0), 1.0);
        assert_eq!(state.pattern.chord_data[track].get_duration(4, 1), 1.5);

        let selected = selection.lock().unwrap();
        assert_eq!(selected.len(), 2);
        assert!(selected.contains(&piano_roll_item_id(4, 0)));
        assert!(selected.contains(&piano_roll_item_id(4, 1)));
    }

    #[test]
    fn piano_roll_nudge_selected_chord_moves_all_selected_notes() {
        let state = Arc::new(SequencerState::new(1, vec![]));
        let selection = Arc::new(Mutex::new(HashSet::new()));
        let move_state = Arc::new(Mutex::new(None));
        let track = 0;
        let step = 2;

        state.pattern.chord_data[track].add_note_with_duration(step, 0.0, 1.0);
        state.pattern.chord_data[track].add_note_with_duration(step, 4.0, 1.0);
        state.pattern.chord_data[track].add_note_with_duration(step, 7.0, 1.0);
        state.pattern.patterns[track].set_step_active(step, true);

        let action = map_value([
            ("type", Value::Keyword("nudge-selection".to_string())),
            (
                "ids",
                list_value(vec![
                    Value::Number(piano_roll_item_id(step, 0) as f64),
                    Value::Number(piano_roll_item_id(step, 1) as f64),
                    Value::Number(piano_roll_item_id(step, 2) as f64),
                ]),
            ),
            ("delta-time", Value::Number(1.0)),
            ("delta-lane", Value::Number(0.0)),
        ]);

        apply_piano_roll_action(&state, track, &selection, &move_state, &action)
            .expect("nudge action");

        assert!(!state.pattern.patterns[track].is_active(step));
        assert_eq!(state.pattern.chord_data[track].count(step), 0);
        assert!(state.pattern.patterns[track].is_active(step + 1));
        let mut transposes = (0..state.pattern.chord_data[track].count(step + 1))
            .map(|idx| state.pattern.chord_data[track].get(step + 1, idx))
            .collect::<Vec<_>>();
        transposes.sort_by(|a, b| a.partial_cmp(b).unwrap());
        assert_eq!(transposes, vec![0.0, 4.0, 7.0]);
    }

    #[test]
    fn piano_roll_nudge_selected_chord_subset_leaves_unselected_notes() {
        let state = Arc::new(SequencerState::new(1, vec![]));
        let selection = Arc::new(Mutex::new(HashSet::new()));
        let move_state = Arc::new(Mutex::new(None));
        let track = 0;
        let step = 2;

        state.pattern.chord_data[track].add_note_with_duration(step, 0.0, 1.0);
        state.pattern.chord_data[track].add_note_with_duration(step, 4.0, 1.0);
        state.pattern.chord_data[track].add_note_with_duration(step, 7.0, 1.0);
        state.pattern.patterns[track].set_step_active(step, true);

        let action = map_value([
            ("type", Value::Keyword("nudge-selection".to_string())),
            (
                "ids",
                list_value(vec![
                    Value::Number(piano_roll_item_id(step, 0) as f64),
                    Value::Number(piano_roll_item_id(step, 2) as f64),
                ]),
            ),
            ("delta-time", Value::Number(1.0)),
            ("delta-lane", Value::Number(0.0)),
        ]);

        apply_piano_roll_action(&state, track, &selection, &move_state, &action)
            .expect("nudge action");

        assert_eq!(state.pattern.chord_data[track].count(step), 1);
        assert_eq!(state.pattern.chord_data[track].get(step, 0), 4.0);
        let mut transposes = (0..state.pattern.chord_data[track].count(step + 1))
            .map(|idx| state.pattern.chord_data[track].get(step + 1, idx))
            .collect::<Vec<_>>();
        transposes.sort_by(|a, b| a.partial_cmp(b).unwrap());
        assert_eq!(transposes, vec![0.0, 7.0]);
    }

    #[test]
    fn piano_roll_bulk_resize_chord_uses_original_voice_indices() {
        let state = Arc::new(SequencerState::new(1, vec![]));
        let selection = Arc::new(Mutex::new(HashSet::new()));
        let move_state = Arc::new(Mutex::new(None));
        let track = 0;
        let step = 2;
        state.pattern.chord_data[track].add_note_with_duration(step, 7.0, 1.0);
        state.pattern.chord_data[track].add_note_with_duration(step, 0.0, 8.0);
        state.pattern.chord_data[track].add_note_with_duration(step, 4.0, 2.0);
        state.pattern.patterns[track].set_step_active(step, true);
        state.pattern.step_data[track].set(step, StepParam::Duration, 8.0);

        let action = map_value([
            ("type", Value::Keyword("resize-item-absolute".to_string())),
            ("id", Value::Number(piano_roll_item_id(step, 0) as f64)),
            (
                "ids",
                list_value(vec![
                    Value::Number(piano_roll_item_id(step, 0) as f64),
                    Value::Number(piano_roll_item_id(step, 1) as f64),
                    Value::Number(piano_roll_item_id(step, 2) as f64),
                ]),
            ),
            ("time", Value::Number(4.0)),
            ("duration-delta", Value::Number(1.0)),
        ]);

        apply_piano_roll_action(&state, track, &selection, &move_state, &action)
            .expect("resize action");

        let mut durations_by_transpose = (0..state.pattern.chord_data[track].count(step))
            .map(|idx| {
                (
                    state.pattern.chord_data[track].get(step, idx),
                    state.pattern.chord_data[track].get_duration(step, idx),
                )
            })
            .collect::<Vec<_>>();
        durations_by_transpose.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());

        assert_eq!(
            durations_by_transpose,
            vec![(0.0, 9.0), (4.0, 3.0), (7.0, 2.0)]
        );
    }

    #[test]
    fn piano_roll_repeated_bulk_resize_chord_keeps_anchor_identity() {
        let state = Arc::new(SequencerState::new(1, vec![]));
        let selection = Arc::new(Mutex::new(HashSet::new()));
        let move_state = Arc::new(Mutex::new(None));
        let track = 0;
        let step = 2;
        state.pattern.chord_data[track].add_note_with_duration(step, 7.0, 3.0);
        state.pattern.chord_data[track].add_note_with_duration(step, 0.0, 4.0);
        state.pattern.chord_data[track].add_note_with_duration(step, 4.0, 3.0);
        state.pattern.patterns[track].set_step_active(step, true);
        state.pattern.step_data[track].set(step, StepParam::Duration, 4.0);

        let ids = || {
            list_value(vec![
                Value::Number(piano_roll_item_id(step, 0) as f64),
                Value::Number(piano_roll_item_id(step, 1) as f64),
                Value::Number(piano_roll_item_id(step, 2) as f64),
            ])
        };
        let first = map_value([
            ("type", Value::Keyword("resize-item-absolute".to_string())),
            ("id", Value::Number(piano_roll_item_id(step, 0) as f64)),
            ("ids", ids()),
            ("time", Value::Number(6.0)),
            ("duration-delta", Value::Number(1.0)),
        ]);
        let second = map_value([
            ("type", Value::Keyword("resize-item-absolute".to_string())),
            ("id", Value::Number(piano_roll_item_id(step, 0) as f64)),
            ("ids", ids()),
            ("time", Value::Number(4.0)),
            ("duration-delta", Value::Number(-1.0)),
        ]);

        apply_piano_roll_action(&state, track, &selection, &move_state, &first)
            .expect("first resize action");
        apply_piano_roll_action(&state, track, &selection, &move_state, &second)
            .expect("second resize action");

        let mut durations_by_transpose = (0..state.pattern.chord_data[track].count(step))
            .map(|idx| {
                (
                    state.pattern.chord_data[track].get(step, idx),
                    state.pattern.chord_data[track].get_duration(step, idx),
                )
            })
            .collect::<Vec<_>>();
        durations_by_transpose.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());

        assert_eq!(
            durations_by_transpose,
            vec![(0.0, 3.0), (4.0, 2.0), (7.0, 2.0)]
        );
    }

    #[test]
    fn piano_roll_delete_multiple_chord_notes_uses_original_voice_indices() {
        let state = Arc::new(SequencerState::new(1, vec![]));
        let selection = Arc::new(Mutex::new(HashSet::new()));
        let move_state = Arc::new(Mutex::new(None));
        let track = 0;
        let step = 2;
        state.pattern.chord_data[track].add_note_with_duration(step, 0.0, 1.0);
        state.pattern.chord_data[track].add_note_with_duration(step, 4.0, 1.0);
        state.pattern.chord_data[track].add_note_with_duration(step, 7.0, 1.0);
        state.pattern.patterns[track].set_step_active(step, true);

        let action = map_value([
            ("type", Value::Keyword("delete-items".to_string())),
            (
                "ids",
                list_value(vec![
                    Value::Number(piano_roll_item_id(step, 0) as f64),
                    Value::Number(piano_roll_item_id(step, 1) as f64),
                ]),
            ),
        ]);

        apply_piano_roll_action(&state, track, &selection, &move_state, &action)
            .expect("delete action");

        assert_eq!(state.pattern.chord_data[track].count(step), 1);
        assert!(state.pattern.patterns[track].is_active(step));
        assert_eq!(state.pattern.chord_data[track].get(step, 0), 7.0);
    }

    #[test]
    fn piano_roll_delete_one_chord_note_leaves_other_notes() {
        let state = Arc::new(SequencerState::new(1, vec![]));
        let selection = Arc::new(Mutex::new(HashSet::new()));
        let move_state = Arc::new(Mutex::new(None));
        let track = 0;
        let step = 2;
        state.pattern.chord_data[track].add_note_with_duration(step, 0.0, 1.0);
        state.pattern.chord_data[track].add_note_with_duration(step, 4.0, 1.0);
        state.pattern.chord_data[track].add_note_with_duration(step, 7.0, 1.0);
        state.pattern.patterns[track].set_step_active(step, true);

        let action = map_value([
            ("type", Value::Keyword("delete-items".to_string())),
            (
                "ids",
                list_value(vec![Value::Number(piano_roll_item_id(step, 1) as f64)]),
            ),
        ]);

        apply_piano_roll_action(&state, track, &selection, &move_state, &action)
            .expect("delete action");

        assert_eq!(state.pattern.chord_data[track].count(step), 2);
        assert_eq!(state.pattern.chord_data[track].get(step, 0), 0.0);
        assert_eq!(state.pattern.chord_data[track].get(step, 1), 7.0);
    }

    #[test]
    fn piano_roll_preserves_half_step_duration_resolution() {
        let state = Arc::new(SequencerState::new(1, vec![]));
        let selection = Arc::new(Mutex::new(HashSet::new()));
        let move_state = Arc::new(Mutex::new(None));
        let track = 0;
        let step = 2;
        state.pattern.patterns[track].set_step_active(step, true);
        state.pattern.step_data[track].set(step, StepParam::Duration, 0.5);

        let items = build_piano_roll_items_value(&state, track, &selection);
        let Value::List(items) = items else {
            panic!("expected item list");
        };
        let Value::Map(item) = items[0].borrow().clone() else {
            panic!("expected item map");
        };
        assert_eq!(
            item.get("end").map(|value| value.borrow().clone()),
            Some(Value::Number(2.5))
        );

        let action = map_value([
            ("type", Value::Keyword("resize-item-absolute".to_string())),
            ("id", Value::Number(piano_roll_item_id(step, 0) as f64)),
            ("time", Value::Number(2.125)),
        ]);

        apply_piano_roll_action(&state, track, &selection, &move_state, &action)
            .expect("resize action");

        assert_eq!(
            state.pattern.step_data[track].get(step, StepParam::Duration),
            0.125
        );
    }

    #[test]
    fn piano_roll_empty_marquee_clears_selection() {
        let state = Arc::new(SequencerState::new(1, vec![]));
        let selection = Arc::new(Mutex::new(HashSet::new()));
        let move_state = Arc::new(Mutex::new(None));
        let track = 0;
        selection.lock().unwrap().insert(piano_roll_item_id(2, 0));

        let action = map_value([
            ("type", Value::Keyword("finish-marquee-select".to_string())),
            ("time-a", Value::Number(8.0)),
            ("time-b", Value::Number(9.0)),
            ("lane-a", Value::Number(0.0)),
            ("lane-b", Value::Number(1.0)),
        ]);

        apply_piano_roll_action(&state, track, &selection, &move_state, &action)
            .expect("marquee action");

        assert!(selection.lock().unwrap().is_empty());
    }

    #[test]
    fn piano_roll_ignores_left_edge_resize_actions() {
        let state = Arc::new(SequencerState::new(1, vec![]));
        let selection = Arc::new(Mutex::new(HashSet::new()));
        let move_state = Arc::new(Mutex::new(None));
        let track = 0;
        let step = 2;
        state.pattern.patterns[track].set_step_active(step, true);
        state.pattern.step_data[track].set(step, StepParam::Duration, 4.0);

        let action = map_value([
            ("type", Value::Keyword("resize-item-absolute".to_string())),
            ("edge", Value::Keyword("start".to_string())),
            ("id", Value::Number(piano_roll_item_id(step, 0) as f64)),
            ("time", Value::Number(2.125)),
        ]);

        apply_piano_roll_action(&state, track, &selection, &move_state, &action)
            .expect("resize action");

        assert_eq!(
            state.pattern.step_data[track].get(step, StepParam::Duration),
            4.0
        );
    }

    #[test]
    fn generated_custom_instrument_uis_eval_and_dispatch() {
        let mut runtime = Runtime::new();
        runtime
            .eval_str(
                r#"
                (def synth-ui-current-inst false)
                (def synth-ui-current-name "")
                (def inst-param (inst name)
                  (nth (filter |p| (= (get p :name) name) (get inst :synth)) 0))
                (def inst-base-note-param (inst)
                  (nth (filter |p| (= (get p :control) "base-note") (get inst :synth)) 0))
                (def fx-param-value (p)
                  (if (get p :value-field)
                    (bind-seq (get p :value-field))
                    (get p :value)))
                (def base-note ()
                  (label "base" :font-size 10 :color :gray :bg :transparent))
                (def custom-ui-current-kind "instrument")
                (def custom-ui-selected-section 0)
                (def custom-ui-selected-section-for-current-scope () custom-ui-selected-section)
                (def ui-select-section (section) (set! custom-ui-selected-section section))
                (def ui-accent-blue () :blue)
                (def ui-accent-cyan () :cyan)
                (def ui-accent-orange () :orange)
                (def ui-accent-green () :green)
                (def ui-accent-violet () :magenta)
                (def ui-lego-gap () 0.25)
                (def ui-lego-small-h () 1.95)
                (def ui-lego-medium-h () 4.08)
                (def ui-lego-dense-h () 3.08)
                (def ui-lego-full-h () 8.48)
                (def ui-lego-col-w () 24.0)
                (def ui-lego-strip-w () 7.2)
                (def ui-control-block-small (title accent body) body)
                (def ui-control-block-medium (title accent body) body)
                (def ui-control-block-full (title accent body) body)
                (def ui-control-block-small-s (title accent section body) body)
                (def ui-control-block-medium-s (title accent section body) body)
                (def ui-control-block-dense-s (title accent section body) body)
                (def ui-control-panel-dense-s (section body) body)
                (def ui-control-panel-small-s (section body) body)
                (def ui-control-panel-medium-s (section body) body)
                (def ui-control-block-full-s (title accent section body) body)
                (def ui-readout-block-small (title accent body) body)
                (def ui-readout-block-small-s (title accent section body) body)
                (def ui-readout-block-dense-s (title accent section body) body)
                (def ui-readout-panel-small-s (section body) body)
                (def ui-readout-panel-dense-s (section body) body)
                (def ui-readout-panel-medium-s (section body) body)
                (def ui-readout-block-medium (title accent body) body)
                (def ui-readout-block-full (title accent body) body)
                (def ui-lego-column (a b c) (v-stack a b c))
                (def ui-lego-column-2 (a b) (v-stack a b))
                (def ui-lego-column-full (a) (v-stack a))
                (def ui-lego-strip-s (title accent section body) body)
                (def ui-lego-strip-half-s (title accent section body) body)
                (def ui-lego-strip-panel-s (section body) body)
                (def ui-lego-badge (title width accent) (label title :font-size 10 :color :gray :bg :transparent))
                (def ui-lego-badge-s (section title width accent) (label title :font-size 10 :color :gray :bg :transparent))
                (def ui-lego-knob (name title width accent decimals) (label title :font-size 10 :color :gray :bg :transparent))
                (def ui-lego-knob-s (section name title width accent decimals) (label title :font-size 10 :color :gray :bg :transparent))
                (def ui-lego-num (name title width decimals unit accent) (label title :font-size 10 :color :gray :bg :transparent))
                (def ui-lego-num-s (section name title width decimals unit accent) (label title :font-size 10 :color :gray :bg :transparent))
                (def ui-lego-micro-num-s (section name title width decimals unit accent) (label title :font-size 10 :color :gray :bg :transparent))
                (def ui-lego-option (name title width options accent) (label title :font-size 10 :color :gray :bg :transparent))
                (def ui-lego-option-s (section name title width options accent) (label title :font-size 10 :color :gray :bg :transparent))
                (def ui-lego-micro-option-s (section name title width options accent) (label title :font-size 10 :color :gray :bg :transparent))
                (def ui-lego-row (name title decimals unit accent) (label title :font-size 10 :color :gray :bg :transparent))
                (def ui-lego-base-note (width accent) (label "base" :font-size 10 :color :gray :bg :transparent))
                (def ui-lego-micro-base-note-s (section width accent) (label "base" :font-size 10 :color :gray :bg :transparent))
                (def ui-lego-text-row-3 (a b c) (h-stack a b c))
                (def ui-lego-text-row-4 (a b c d) (h-stack a b c d))
                (def ui-lego-adsr-s (section title attack decay sustain release) (label title :font-size 10 :color :gray :bg :transparent))
                (def ui-detail-adsr-s (section title attack decay sustain release) (label title :font-size 10 :color :gray :bg :transparent))
                (def ui-detail-adsr-switch-s (section-a title-a attack-a decay-a sustain-a release-a section-b title-b attack-b decay-b sustain-b release-b) (label title-a :font-size 10 :color :gray :bg :transparent))
                (def ui-adsr-compact-s (section title attack decay sustain release) (label title :font-size 10 :color :gray :bg :transparent))
                (def ui-adsr-compact-switch-s (section-a title-a attack-a decay-a sustain-a release-a section-b title-b attack-b decay-b sustain-b release-b) (label title-a :font-size 10 :color :gray :bg :transparent))
                (def ui-adsr-number-s (section name title decimals unit) (label title :font-size 10 :color :gray :bg :transparent))
                "#,
            )
            .expect("load custom UI test helpers");

        let custom_ui_source = build_custom_instrument_ui_source_with_overlay(None);
        runtime
            .eval_str(&custom_ui_source)
            .expect("load custom instrument UIs");

        for instrument_name in [
            "emulations/dx7-4op/",
            "emulations/digitone/",
            "emulations/hammond-organ/",
            "emulations/minimoog/",
            "emulations/monomachine-digipro/",
            "emulations/monomachine-dpro-bbox-v1/",
            "emulations/monomachine-dpro-dens-v1/",
            "emulations/monomachine-dpro-ddrw-v1/",
            "emulations/monomachine-dpro-wave-v2/",
            "emulations/monomachine-fmplus/",
            "emulations/monomachine-fmplus-par-v1/",
            "emulations/monomachine-fmplus-stat-v1/",
            "emulations/monomachine-sid/",
            "emulations/monomachine-superwave/",
            "emulations/oberheim-sem/",
            "emulations/prophet-5/",
            "emulations/prophet-6/",
            "emulations/prophet-6-emu/",
            "emulations/prophet-6-inspired/",
            "emulations/rhodes-additive-v2/",
        ] {
            let expr = format!(
                "(custom-instrument-synth-ui (dict :name {:?} :synth (list (dict :name \"base_note\" :control \"base-note\" :value 0 :min -48 :max 48))))",
                instrument_name
            );
            let rendered = runtime.eval_str(&expr).expect(instrument_name);
            assert!(
                !matches!(rendered, Some(Value::Bool(false)) | None),
                "{instrument_name} did not dispatch to a custom UI"
            );
        }
    }

    #[test]
    fn generated_custom_midi_fx_uis_eval_and_dispatch() {
        let mut runtime = Runtime::new();
        runtime
            .eval_str(
                r#"
                (def midi-fx-ui-current-fx false)
                (def midi-fx-ui-current-name "")
                (def midi-fx-ui-param (fx name)
                  (nth (filter |p| (= (get p :name) name) (get fx :params)) 0))
                (def fx-param-row (p fx key)
                  (dict :param (get p :name) :key key))
                (def midi-fx-ui-param-control (name)
                  (let ((p (midi-fx-ui-param midi-fx-ui-current-fx name)))
                    (if p
                      (fx-param-row p midi-fx-ui-current-fx
                        (str "custom-midi-fx-ui-" midi-fx-ui-current-name
                             "-slot-" (get midi-fx-ui-current-fx :slot-idx) "-" name))
                      false)))
                "#,
            )
            .expect("load custom MIDI FX UI test helpers");

        let custom_ui_source = build_custom_midi_fx_ui_source_with_overlay(None);
        runtime
            .eval_str(&custom_ui_source)
            .expect("load custom MIDI FX UIs");

        let rendered = runtime
            .eval_str(
                r#"
                (custom-midi-fx-ui
                  (dict :name "arp"
                        :params (list
                          (dict :name "rate" :value 4 :min 0 :max 12)
                          (dict :name "direction" :value 0 :min 0 :max 3)
                          (dict :name "gate" :value 0.9 :min 0.05 :max 1.0)
                          (dict :name "velocity" :value 0.8 :min 0 :max 1))))
                "#,
            )
            .expect("render custom MIDI FX UI");
        assert!(!matches!(rendered, Some(Value::Bool(false)) | None));
    }

    #[test]
    fn generated_custom_audio_fx_ui_eval_and_dispatch() {
        let mut runtime = Runtime::new();
        runtime
            .eval_str(
                r#"
                (def audio-fx-ui-current-fx false)
                (def audio-fx-ui-current-name "")
                (def audio-fx-ui-param (fx name)
                  (nth (filter |p| (= (get p :name) name) (get fx :params)) 0))
                (def fx-param-row (p fx key)
                  (dict :param (get p :name) :key key))
                (def custom-ui-scope-name ()
                  (if (get audio-fx-ui-current-fx :bus-fx)
                    (str audio-fx-ui-current-name "-bus-" (get audio-fx-ui-current-fx :bus-idx)
                         "-slot-" (get audio-fx-ui-current-fx :slot-idx))
                    (str audio-fx-ui-current-name "-slot-" (get audio-fx-ui-current-fx :slot-idx))))
                (def audio-fx-ui-param-control (name)
                  (let ((p (audio-fx-ui-param audio-fx-ui-current-fx name)))
                    (if p
                      (fx-param-row p audio-fx-ui-current-fx
                        (str "custom-audio-fx-ui-" (custom-ui-scope-name) "-" name))
                      false)))
                (def custom-ui-current-kind "audio-fx")
                (def custom-ui-selected-section 0)
                (def custom-ui-selected-section-for-current-scope () custom-ui-selected-section)
                (def ui-select-section (section) section)
                (def ui-accent-blue () :blue)
                (def ui-accent-cyan () :cyan)
                (def ui-accent-orange () :orange)
                (def ui-accent-green () :green)
                (def ui-accent-violet () :magenta)
                (def ui-control-block-small-s (title accent section body) body)
                (def ui-control-block-medium-s (title accent section body) body)
                (def ui-control-block-full-s (title accent section body) body)
                (def ui-readout-block-small-s (title accent section body) body)
                (def ui-readout-block-medium (title accent body) body)
                (def ui-readout-block-full (title accent body) body)
                (def ui-lego-column (a b c) (v-stack a b c))
                (def ui-lego-column-2 (a b) (v-stack a b))
                (def ui-lego-column-full (a) (v-stack a))
                (def ui-lego-knob-s (section name title width accent decimals)
                  (audio-fx-ui-param-control name))
                (def ui-lego-num-s (section name title width decimals unit accent)
                  (audio-fx-ui-param-control name))
                (def ui-lego-text-row-3 (a b c) (h-stack a b c))
                (def ui-lego-text-row-4 (a b c d) (h-stack a b c d))
                "#,
            )
            .expect("load custom audio FX UI test helpers");

        let custom_ui_source = build_custom_audio_fx_ui_source_with_overlay(Some((
            "folder-delay".to_string(),
            "effects/folder-delay/ui.lisp".to_string(),
            r#"
            (def helper-label (text) (label text :font-size 10 :color :gray :bg :transparent))
            (defeffect-ui
              (v-stack :width :fill
                (helper-label "Folder Delay")
                (params "time" "feedback")))
            "#
            .to_string(),
        )));
        runtime
            .eval_str(&custom_ui_source)
            .expect("load custom audio FX UI");

        let rendered = runtime
            .eval_str(
                r#"
                (custom-audio-fx-ui
                  (dict :name "folder-delay"
                        :slot-idx 2
                        :params (list
                          (dict :name "time" :value 250 :min 1 :max 2000)
                          (dict :name "feedback" :value 0.35 :min 0 :max 0.95))))
                "#,
            )
            .expect("render custom audio FX UI");
        assert!(!matches!(rendered, Some(Value::Bool(false)) | None));
        let rendered_control = runtime
            .eval_str(r#"(audio-fx-ui-param-control "time")"#)
            .expect("render scoped custom audio FX control");
        let rendered_text = format!("{rendered_control:?}");
        assert!(
            rendered_text.contains("custom-audio-fx-ui-folder-delay-slot-2-time"),
            "custom audio FX param controls must include slot scope in stable keys: {rendered_text}"
        );

        let rendered_slot_5 = runtime
            .eval_str(
                r#"
                (custom-audio-fx-ui
                  (dict :name "folder-delay"
                        :slot-idx 5
                        :params (list
                          (dict :name "time" :value 250 :min 1 :max 2000)
                          (dict :name "feedback" :value 0.35 :min 0 :max 0.95))))
                "#,
            )
            .expect("render second custom audio FX UI slot");
        assert!(!matches!(rendered_slot_5, Some(Value::Bool(false)) | None));
        let rendered_slot_5_control = runtime
            .eval_str(r#"(audio-fx-ui-param-control "time")"#)
            .expect("render second scoped custom audio FX control");
        let rendered_slot_5_text = format!("{rendered_slot_5_control:?}");
        assert!(
            rendered_slot_5_text.contains("custom-audio-fx-ui-folder-delay-slot-5-time"),
            "custom audio FX param controls must not collide across slots: {rendered_slot_5_text}"
        );
        assert_ne!(rendered_text, rendered_slot_5_text);

        let all_params = [
            "base",
            "color",
            "cutoff",
            "damping",
            "decay",
            "delay_max",
            "density",
            "depth",
            "diffusion",
            "drive",
            "fbk",
            "feedback",
            "freeze",
            "freq",
            "gain",
            "knee",
            "lforate",
            "m1",
            "m2",
            "manual",
            "max1",
            "max2",
            "mix",
            "mod_amt",
            "mod_freq",
            "motion",
            "phase_amt",
            "pre",
            "pre_dly",
            "ratio",
            "rate",
            "res",
            "shimmer",
            "size",
            "smear",
            "spread",
            "threshold",
            "tone",
            "wet",
            "width",
        ];
        let params = all_params
            .iter()
            .map(|name| format!(r#"(dict :name "{name}" :value 0.5 :min 0 :max 1)"#))
            .collect::<Vec<_>>()
            .join(" ");
        for fx_name in [
            "MODUM_DELAY",
            "dimension-d-chorus",
            "dualdelaymod",
            "jet-flanger-stereo",
            "lexilush",
            "lowdcmp",
            "lushlexiconreverb",
            "mod_delay",
            "roland-flanger-stereo",
            "sidechain",
            "spectral-bin-freeze",
            "spectral-cloud-gate",
            "spectral-short-ir",
            "spectral-vocoder-shift",
            "stereo-tremolo",
        ] {
            let expr =
                format!(r#"(custom-audio-fx-ui (dict :name "{fx_name}" :params (list {params})))"#);
            let rendered = runtime.eval_str(&expr).expect(fx_name);
            assert!(
                !matches!(rendered, Some(Value::Bool(false)) | None),
                "{fx_name} did not dispatch to a custom audio FX UI"
            );
        }
    }
}

pub(crate) fn auto_follow_enabled(override_until: &Arc<Mutex<Option<Instant>>>) -> bool {
    let guard = override_until.lock().unwrap();
    match *guard {
        Some(until) => Instant::now() >= until,
        None => true,
    }
}

pub(crate) fn poll_pending_compile_status(
    app: &mut ui::App,
    editor: &mut Editor,
    state: &Arc<SequencerState>,
    current_track: &Arc<AtomicUsize>,
    selected_steps: &Arc<Mutex<HashSet<usize>>>,
    fx_epoch: &Arc<AtomicUsize>,
    ui_epoch: &Arc<AtomicUsize>,
) {
    if let Some(status) = app.poll_pending_compile() {
        let ct = current_track.load(Ordering::Relaxed);
        let rt = editor.runtime_mut();
        rt.set_reactive("SEQ", "compiling", Value::Bool(false));
        rt.set_reactive(
            "SEQ",
            "effects",
            if app.tracks.is_empty() {
                Value::List(vec![])
            } else {
                build_effects_value(&state, ct, &app.graph.effect_descriptors, &selected_steps)
            },
        );
        rt.set_reactive(
            "SEQ",
            "midi-effects",
            if app.tracks.is_empty() {
                Value::List(vec![])
            } else {
                build_midi_effects_value(&state, ct, &selected_steps)
            },
        );
        rt.set_reactive(
            "SEQ",
            "instrument-panel",
            if app.tracks.is_empty() {
                Value::List(vec![])
            } else {
                build_instrument_panel_value(&app, ct, &selected_steps)
            },
        );
        sync_fx_param_binding_fields(rt, app, state, ct, selected_steps);
        rt.set_reactive(
            "SEQ",
            "step-has-plocks",
            if app.tracks.is_empty() {
                Value::List(vec![])
            } else {
                build_step_has_plocks(state, ct, &app.graph.effect_descriptors)
            },
        );
        rt.run_reactive_cycle();
        editor.refresh_runtime_side_effects();
        editor.refresh_visible_layouts_for_buffer_named("*fx*");
        fx_epoch.fetch_add(1, Ordering::Relaxed);
        ui_epoch.fetch_add(1, Ordering::Relaxed);
        editor.handle_host_event(HostEvent::Status(status));
    }
}
