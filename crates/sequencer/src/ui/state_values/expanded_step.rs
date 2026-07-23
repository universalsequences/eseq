use super::*;

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

pub(super) fn expanded_step_param_for_mode(mode: usize) -> Option<StepParam> {
    match mode {
        0 => Some(StepParam::Velocity),
        1 => Some(StepParam::Duration),
        2 => Some(StepParam::AuxA),
        3 => Some(StepParam::Transpose),
        4 => Some(StepParam::Pan),
        5 => Some(StepParam::Sync),
        6 => Some(StepParam::Delay),
        _ => None,
    }
}

pub(super) fn expanded_step_param_slider_value(param: StepParam, value: f32) -> f32 {
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
        param
            .map(|param| state.pattern.step_data[viewport.track].get(step, param))
            .or_else(|| process_lane_value_for_mode(state, viewport.track, mode, step))
            .unwrap_or(0.0)
    } else {
        0.0
    };
    let slider_value = param
        .map(|param| expanded_step_param_slider_value(param, value))
        .unwrap_or(value);
    let mut dirty = false;
    dirty |= rt
        .set_reactive(
            "SEQ",
            &expanded_step_slot_param_slider_field(viewport.track_id, mode, slot),
            Value::Number(slider_value as f64),
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
    app: &app::App,
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
    app: &app::App,
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
    app: &app::App,
    current_track_idx: usize,
    selected_steps: &Arc<Mutex<HashSet<usize>>>,
    plock_masks: &[[u64; MAX_STEPS / 64]],
) {
    sync_all_track_step_binding_fields_inner(
        rt,
        state,
        app,
        current_track_idx,
        selected_steps,
        plock_masks,
        None,
    );
}

pub(crate) fn sync_all_track_step_binding_fields_profiled(
    rt: &mut Runtime,
    state: &Arc<SequencerState>,
    app: &app::App,
    current_track_idx: usize,
    selected_steps: &Arc<Mutex<HashSet<usize>>>,
    plock_masks: &[[u64; MAX_STEPS / 64]],
) -> AllTrackStepBindingSyncProfile {
    let mut profile = AllTrackStepBindingSyncProfile::default();
    sync_all_track_step_binding_fields_inner(
        rt,
        state,
        app,
        current_track_idx,
        selected_steps,
        plock_masks,
        Some(&mut profile),
    );
    profile
}

pub(super) fn sync_all_track_step_binding_fields_inner(
    rt: &mut Runtime,
    state: &Arc<SequencerState>,
    app: &app::App,
    current_track_idx: usize,
    selected_steps: &Arc<Mutex<HashSet<usize>>>,
    plock_masks: &[[u64; MAX_STEPS / 64]],
    mut profile: Option<&mut AllTrackStepBindingSyncProfile>,
) {
    const WORDS: usize = MAX_STEPS / 64;
    let total_started = profile.as_ref().map(|_| Instant::now());
    let selected = selected_steps.lock().unwrap();
    for track in 0..app.tracks.len() {
        let num_steps = state.pattern.track_params[track]
            .get_num_steps()
            .min(MAX_STEPS);

        // Compute all four boolean lanes as bitmasks in one pass.
        let pattern_bits = state.pattern.patterns[track].load_bits();
        let plock_bits = plock_masks
            .get(track)
            .copied()
            .unwrap_or_else(|| track_step_plock_mask(state, track, &app.graph.effect_descriptors));
        let mut active_mask = [0u64; WORDS];
        let mut duration_mask = [0u64; WORDS];
        let mut plocked_mask = [0u64; WORDS];
        let mut selected_mask = [0u64; WORDS];
        let render_values = plock_variant_step_render_values(state, track);
        let mut max_reach = f64::NEG_INFINITY;
        for step in 0..MAX_STEPS {
            let word = step / 64;
            let bit = 1u64 << (step % 64);
            let visible = step < num_steps;
            let is_active = pattern_bits[word] & bit != 0;
            if is_active {
                // duration > (target - source) <=> source + duration > target;
                // computed in f64 where both forms are exact.
                let duration = state.pattern.step_data[track]
                    .get(step, StepParam::Duration)
                    .max(0.0) as f64;
                let reach = step as f64 + duration;
                if reach > max_reach {
                    max_reach = reach;
                }
            }
            if visible {
                if is_active {
                    active_mask[word] |= bit;
                }
                if max_reach > step as f64 {
                    duration_mask[word] |= bit;
                }
                if plock_bits[word] & bit != 0 {
                    plocked_mask[word] |= bit;
                }
                if track == current_track_idx && selected.contains(&step) {
                    selected_mask[word] |= bit;
                }
            }
        }

        // Skip all per-step writes when this track's lanes are unchanged.
        let mut rev = String::with_capacity(WORDS * 4 * 16 + 3);
        for mask in [&active_mask, &duration_mask, &plocked_mask, &selected_mask] {
            for word in mask.iter() {
                use std::fmt::Write as _;
                let _ = write!(rev, "{word:016x}");
            }
        }
        for render in &render_values {
            use std::fmt::Write as _;
            let _ = write!(rev, "{:02x}", render.kind);
            for channel in render.color {
                let _ = write!(rev, "{:08x}", channel.to_bits());
            }
        }
        let rev_changed = rt
            .set_reactive(
                "SEQ",
                &track_step_binding_rev_field(track),
                Value::String(rev),
            )
            .changed;
        if !rev_changed {
            continue;
        }

        for step in 0..MAX_STEPS {
            let word = step / 64;
            let bit = 1u64 << (step % 64);
            let started = profile.as_ref().map(|_| Instant::now());
            let result = rt.set_reactive(
                "SEQ",
                &track_step_active_field(track, step),
                Value::Bool(active_mask[word] & bit != 0),
            );
            if let Some(profile) = profile.as_deref_mut() {
                profile.active_elapsed += started.expect("profile timer").elapsed();
                profile.active_sets.note(result);
            }

            let started = profile.as_ref().map(|_| Instant::now());
            let result = rt.set_reactive(
                "SEQ",
                &track_step_duration_field(track, step),
                Value::Bool(duration_mask[word] & bit != 0),
            );
            if let Some(profile) = profile.as_deref_mut() {
                profile.duration_elapsed += started.expect("profile timer").elapsed();
                profile.duration_sets.note(result);
            }

            let started = profile.as_ref().map(|_| Instant::now());
            let result = rt.set_reactive(
                "SEQ",
                &track_step_plocked_field(track, step),
                Value::Bool(plocked_mask[word] & bit != 0),
            );
            if let Some(profile) = profile.as_deref_mut() {
                profile.plocked_elapsed += started.expect("profile timer").elapsed();
                profile.plocked_sets.note(result);
            }

            let started = profile.as_ref().map(|_| Instant::now());
            let result = rt.set_reactive(
                "SEQ",
                &track_step_selected_field(track, step),
                Value::Bool(selected_mask[word] & bit != 0),
            );
            if let Some(profile) = profile.as_deref_mut() {
                profile.selected_elapsed += started.expect("profile timer").elapsed();
                profile.selected_sets.note(result);
            }

            let render = render_values[step];
            let _ = rt.set_reactive(
                "SEQ",
                &track_step_plock_kind_field(track, step),
                Value::Number(render.kind as f64),
            );
            for (channel, value) in ['r', 'g', 'b'].into_iter().zip(render.color) {
                let _ = rt.set_reactive(
                    "SEQ",
                    &track_step_variant_color_field(track, step, channel),
                    Value::Number(value as f64),
                );
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
    app: &app::App,
) -> Value {
    let tracks: Vec<Rc<RefCell<Value>>> = (0..app.tracks.len())
        .map(|track| Rc::new(RefCell::new(build_track_duration_spans_value(state, track))))
        .collect();
    Value::List(tracks)
}

pub(crate) fn build_all_track_playheads_value(
    state: &Arc<SequencerState>,
    app: &app::App,
) -> Value {
    let items: Vec<Rc<RefCell<Value>>> = (0..app.tracks.len())
        .map(|t| {
            Rc::new(RefCell::new(Value::Number(
                state.transport.track_playheads[t].load(Ordering::Relaxed) as f64,
            )))
        })
        .collect();
    Value::List(items)
}

pub(crate) fn build_all_track_step_has_plocks_from_masks(
    plock_masks: &[[u64; MAX_STEPS / 64]],
) -> Value {
    let tracks: Vec<Rc<RefCell<Value>>> = plock_masks
        .iter()
        .map(|mask| Rc::new(RefCell::new(build_step_has_plocks_from_mask(mask))))
        .collect();
    Value::List(tracks)
}

pub(crate) fn build_all_track_param_lists_value(
    state: &Arc<SequencerState>,
    app: &app::App,
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

pub(crate) fn track_playheads_snapshot(state: &Arc<SequencerState>, app: &app::App) -> Vec<u32> {
    (0..app.tracks.len())
        .map(|t| state.transport.track_playheads[t].load(Ordering::Relaxed))
        .collect()
}

pub(super) fn track_playhead_row_field(track: usize, row: usize) -> String {
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

pub(super) fn track_playhead_row_count(state: &Arc<SequencerState>, track: usize) -> usize {
    let num_steps = state.pattern.track_params[track]
        .get_num_steps()
        .max(1)
        .min(MAX_STEPS);
    (num_steps + PAGE_SIZE - 1) / PAGE_SIZE
}

pub(crate) fn sync_all_track_playhead_fields(
    rt: &mut Runtime,
    state: &Arc<SequencerState>,
    app: &app::App,
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

pub(crate) fn clear_all_track_playhead_fields(rt: &mut Runtime, app: &app::App) {
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
