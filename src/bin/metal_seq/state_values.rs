use super::*;

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

pub(crate) fn sync_track_name_state(
    rt: &mut Runtime,
    track_names: &mut Vec<String>,
    app: &ui::App,
) {
    if *track_names == app.tracks {
        return;
    }
    *track_names = app.tracks.clone();
    rt.set_reactive("SEQ", "num-tracks", Value::Number(track_names.len() as f64));
    rt.set_reactive("SEQ", "track-names", build_track_names(track_names));
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

pub(crate) fn sync_track_mixer_state(rt: &mut Runtime, state: &Arc<SequencerState>) {
    rt.set_reactive("SEQ", "track-volumes", build_track_volumes(state));
    rt.set_reactive("SEQ", "track-mutes", build_track_mutes(state));
    rt.set_reactive("SEQ", "track-solos", build_track_solos(state));
    rt.set_reactive(
        "SEQ",
        "track-muted-by-solo",
        build_track_muted_by_solo(state),
    );
}

pub(crate) fn sync_track_mixer_empty_state(rt: &mut Runtime) {
    rt.set_reactive("SEQ", "track-volumes", Value::List(vec![]));
    rt.set_reactive("SEQ", "track-mutes", Value::List(vec![]));
    rt.set_reactive("SEQ", "track-solos", Value::List(vec![]));
    rt.set_reactive("SEQ", "track-muted-by-solo", Value::List(vec![]));
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

pub(crate) fn read_track_peak_levels(
    lg: sequencer::audiograph::LiveGraphPtr,
    pan_ids: &[i32],
) -> Vec<f64> {
    const PANNER_STATE_LEN: usize = sequencer::stereo_panner::STEREO_PANNER_STATE_SIZE;
    const PANNER_STATE_BYTES: usize = PANNER_STATE_LEN * std::mem::size_of::<f32>();
    pan_ids
        .iter()
        .map(|&pan_id| {
            if pan_id < 0 {
                return 0.0;
            }
            let mut state_size = 0usize;
            let mut state = [0.0_f32; PANNER_STATE_LEN];
            let copied = unsafe {
                sequencer::audiograph::get_node_state_into(
                    lg.0,
                    pan_id,
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
        })
        .collect()
}

pub(crate) fn build_track_peaks_value(levels: &[f64]) -> Value {
    let items: Vec<Rc<RefCell<Value>>> = levels
        .iter()
        .map(|&level| Rc::new(RefCell::new(Value::Number(level))))
        .collect();
    Value::List(items)
}

pub(crate) fn sync_track_peak_fields(rt: &mut Runtime, levels: &[f64]) {
    for (idx, &level) in levels.iter().enumerate() {
        rt.set_reactive("SEQ", &format!("track-peak-{idx}"), Value::Number(level));
    }
}

pub(crate) fn sync_track_peak_field_delta(rt: &mut Runtime, previous: &[f64], levels: &[f64]) {
    if previous.len() != levels.len() {
        sync_track_peak_fields(rt, levels);
        for idx in levels.len()..previous.len() {
            rt.set_reactive("SEQ", &format!("track-peak-{idx}"), Value::Number(0.0));
        }
        return;
    }

    for (idx, (&old_level, &level)) in previous.iter().zip(levels.iter()).enumerate() {
        if old_level != level {
            rt.set_reactive("SEQ", &format!("track-peak-{idx}"), Value::Number(level));
        }
    }
}

pub(crate) fn sync_playhead_fields(rt: &mut Runtime, playhead: usize, num_steps: usize) {
    let clamped_steps = num_steps.max(1).min(MAX_STEPS);
    let active_step = playhead.min(clamped_steps.saturating_sub(1));
    rt.set_reactive(
        "SEQ",
        "playhead-page",
        Value::Number((active_step / PAGE_SIZE) as f64),
    );
    rt.set_reactive("SEQ", "playhead", Value::Number(active_step as f64));
    for idx in 0..MAX_STEPS {
        rt.set_reactive(
            "SEQ",
            &format!("playhead-active-{idx}"),
            Value::Bool(idx == active_step && idx < clamped_steps),
        );
    }
}

pub(crate) fn sync_playhead_field_delta(
    rt: &mut Runtime,
    prev_playhead: usize,
    playhead: usize,
    num_steps: usize,
) {
    let clamped_steps = num_steps.max(1).min(MAX_STEPS);
    let prev_active = prev_playhead.min(clamped_steps.saturating_sub(1));
    let active_step = playhead.min(clamped_steps.saturating_sub(1));
    rt.set_reactive(
        "SEQ",
        "playhead-page",
        Value::Number((active_step / PAGE_SIZE) as f64),
    );
    rt.set_reactive("SEQ", "playhead", Value::Number(active_step as f64));
    if prev_active != active_step {
        rt.set_reactive(
            "SEQ",
            &format!("playhead-active-{prev_active}"),
            Value::Bool(false),
        );
        rt.set_reactive(
            "SEQ",
            &format!("playhead-active-{active_step}"),
            Value::Bool(true),
        );
    }
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
    sync_pattern_state(rt, state);
    rt.set_reactive(
        "SEQ",
        "current-track",
        Value::Number(current_track_idx as f64),
    );
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
        sync_track_mixer_empty_state(rt);
        rt.set_reactive("SEQ", "effects", Value::List(vec![]));
        rt.set_reactive("SEQ", "midi-effects", Value::List(vec![]));
        rt.set_reactive("SEQ", "instrument-panel", Value::List(vec![]));
        rt.set_reactive("SEQ", "step-has-plocks", Value::List(vec![]));
        return;
    }

    sync_playhead_fields(
        rt,
        state.transport.track_playheads[current_track_idx].load(Ordering::Relaxed) as usize,
        state.pattern.track_params[current_track_idx].get_num_steps(),
    );
    rt.set_reactive("SEQ", "steps", build_steps_value(state, current_track_idx));
    sync_piano_roll_state(rt, state, current_track_idx, piano_roll_selection);
    sync_step_param_lists(rt, state, current_track_idx);
    sync_track_mixer_state(rt, state);
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
        Value::Number(state.pattern.current_pattern.load(Ordering::Relaxed) as f64),
    );
    rt.set_reactive(
        "SEQ",
        "num-patterns",
        Value::Number(state.pattern.num_patterns.load(Ordering::Relaxed) as f64),
    );
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

            let params: Vec<Rc<RefCell<Value>>> = desc
                .params
                .iter()
                .enumerate()
                .map(|(param_idx, pdesc)| {
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
                        .map(|s| s.defaults.get(param_idx))
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
                        ParamKind::Continuous { .. } => {
                            if desc.name == "Delay" && param_idx == 2 && delay_synced {
                                let labels: Vec<String> = SyncDivision::ALL
                                    .iter()
                                    .map(|d| d.label().to_string())
                                    .collect();
                                let selected = labels
                                    .get(current_val.round() as usize)
                                    .cloned()
                                    .unwrap_or_default();
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

    Value::List(slots)
}

pub(crate) fn build_midi_effects_value(
    state: &Arc<SequencerState>,
    track: usize,
    selected: &Arc<Mutex<HashSet<usize>>>,
) -> Value {
    use sequencer::effects::{EffectDescriptor, ParamKind};
    use std::collections::HashMap;

    let descriptors = sequencer::lisp_effect::load_midi_fx_descriptors();
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
                        .map(|s| s.defaults.get(param_idx))
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
    let registered_sample = app
        .sampler_paths
        .get(track)
        .and_then(|p| p.as_ref())
        .and_then(|p| eseqlisp::audio::sample::get_registered_sample(&p.display().to_string()));
    let buffer_value = registered_sample.as_ref().map(|s| s.to_value());
    let sample_duration = registered_sample
        .as_ref()
        .map(|s| s.duration_seconds)
        .unwrap_or(1.0);

    let mut params: Vec<Rc<RefCell<Value>>> = Vec::new();
    for (param_idx, pdesc) in desc.params.iter().enumerate() {
        let default_val = slot.defaults.get(param_idx);
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
        params.push(Rc::new(RefCell::new(Value::Map(pmap))));
    }

    let mut panel_map: HashMap<String, Rc<RefCell<Value>>> = HashMap::new();
    panel_map.insert(
        "type".to_string(),
        Rc::new(RefCell::new(Value::String("sampler".to_string()))),
    );
    if let Some(buf_val) = buffer_value {
        panel_map.insert("buffer".to_string(), Rc::new(RefCell::new(buf_val)));
    }
    panel_map.insert(
        "params".to_string(),
        Rc::new(RefCell::new(Value::List(params))),
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
    panel_map.insert(
        "end-time".to_string(),
        Rc::new(RefCell::new(Value::Number(
            (end_raw as f64) * sample_duration,
        ))),
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

    const MOD_PARAM_BASE: u32 = 1_000_000;
    const PARAM_LFO1_RATE_HZ: usize = 13;
    const PARAM_LFO1_SYNC: usize = 14;
    const PARAM_LFO1_DIV: usize = 15;
    const PARAM_LFO1_SHAPE: usize = 16;
    const PARAM_LFO1_PW: usize = 17;
    const PARAM_LFO1_RETRIGGER: usize = 18;
    const PARAM_LFO2_RATE_HZ: usize = 19;
    const PARAM_LFO2_SYNC: usize = 20;
    const PARAM_LFO2_DIV: usize = 21;
    const PARAM_LFO2_SHAPE: usize = 22;
    const PARAM_LFO2_PW: usize = 23;
    const PARAM_LFO2_RETRIGGER: usize = 24;
    const PARAM_LFO3_RATE_HZ: usize = 25;
    const PARAM_LFO3_SYNC: usize = 26;
    const PARAM_LFO3_DIV: usize = 27;
    const PARAM_LFO3_SHAPE: usize = 28;
    const PARAM_LFO3_PW: usize = 29;
    const PARAM_LFO3_RETRIGGER: usize = 30;
    const PARAM_ENV_ATTACK_MS: usize = 31;
    const PARAM_ENV_DECAY_MS: usize = 32;
    const PARAM_ENV_SUSTAIN: usize = 33;
    const PARAM_ENV_RELEASE_MS: usize = 34;
    const PARAM_RAND_RATE_HZ: usize = 35;
    const PARAM_RAND_SYNC: usize = 36;
    const PARAM_RAND_DIV: usize = 37;
    const PARAM_RAND_SLEW: usize = 38;
    const PARAM_DRIFT_RATE: usize = 39;
    const PARAM_DRIFT_SYNC: usize = 40;
    const PARAM_DRIFT_DIV: usize = 41;

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

    fn push_param(
        out: &mut Vec<Rc<RefCell<Value>>>,
        name: String,
        control: &str,
        idx: Option<usize>,
        value: f32,
        min: f32,
        max: f32,
        options: Option<&Vec<String>>,
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
        out.push(Rc::new(RefCell::new(Value::Map(pmap))));
    }

    fn is_mod_param(name: &str) -> bool {
        name.starts_with("mod ")
    }

    fn is_source_param(node_param_idx: u32) -> bool {
        node_param_idx >= MOD_PARAM_BASE
    }

    fn source_section_name(node_param_idx: u32) -> &'static str {
        if (MOD_PARAM_BASE + PARAM_LFO1_RATE_HZ as u32
            ..=MOD_PARAM_BASE + PARAM_LFO1_RETRIGGER as u32)
            .contains(&node_param_idx)
        {
            "LFO 1"
        } else if (MOD_PARAM_BASE + PARAM_ENV_ATTACK_MS as u32
            ..=MOD_PARAM_BASE + PARAM_ENV_RELEASE_MS as u32)
            .contains(&node_param_idx)
        {
            "ENV 1"
        } else if (MOD_PARAM_BASE + PARAM_RAND_RATE_HZ as u32
            ..=MOD_PARAM_BASE + PARAM_RAND_SLEW as u32)
            .contains(&node_param_idx)
        {
            "RAND"
        } else if (MOD_PARAM_BASE + PARAM_DRIFT_RATE as u32
            ..=MOD_PARAM_BASE + PARAM_DRIFT_DIV as u32)
            .contains(&node_param_idx)
        {
            "DRIFT"
        } else if (MOD_PARAM_BASE + PARAM_LFO2_RATE_HZ as u32
            ..=MOD_PARAM_BASE + PARAM_LFO2_RETRIGGER as u32)
            .contains(&node_param_idx)
        {
            "LFO 2"
        } else {
            "LFO 3"
        }
    }

    fn rename_source_param(name: &str) -> String {
        if name.ends_with("_div") || name.ends_with("_rate") {
            "rate".to_string()
        } else if name.ends_with("_sync") {
            "sync".to_string()
        } else if name.ends_with("_shape") {
            "shape".to_string()
        } else if name.ends_with("_pw") {
            "pulse width".to_string()
        } else if name.ends_with("_retrigger") {
            "retrigger".to_string()
        } else if name == "mod_rand_slew" {
            "slew".to_string()
        } else if name == "mod_env_attack" {
            "attack".to_string()
        } else if name == "mod_env_decay" {
            "decay".to_string()
        } else if name == "mod_env_sustain" {
            "sustain".to_string()
        } else if name == "mod_env_release" {
            "release".to_string()
        } else {
            name.to_string()
        }
    }

    let source_indices: Vec<usize> = desc
        .params
        .iter()
        .enumerate()
        .filter_map(|(i, p)| is_source_param(p.node_param_idx).then_some(i))
        .collect();

    let find_idx_by_node = |node_param_idx: u32| {
        source_indices
            .iter()
            .copied()
            .find(|&idx| desc.params.get(idx).map(|p| p.node_param_idx) == Some(node_param_idx))
    };

    let lfo_sync = |sync_idx: u32| -> bool {
        find_idx_by_node(sync_idx)
            .map(|idx| slot.defaults.get(idx) > 0.5)
            .unwrap_or(false)
    };
    let lfo_shape_is_pulse = |shape_idx: u32| -> bool {
        find_idx_by_node(shape_idx)
            .map(|idx| slot.defaults.get(idx).round() as i32 == 2)
            .unwrap_or(false)
    };

    let mut source_actual: Vec<usize> = Vec::new();
    let push_lfo = |out: &mut Vec<usize>,
                    rate_idx: usize,
                    sync_idx: usize,
                    div_idx: usize,
                    shape_idx: usize,
                    pw_idx: usize,
                    retrig_idx: usize| {
        let rate_node = MOD_PARAM_BASE + rate_idx as u32;
        let sync_node = MOD_PARAM_BASE + sync_idx as u32;
        let div_node = MOD_PARAM_BASE + div_idx as u32;
        let shape_node = MOD_PARAM_BASE + shape_idx as u32;
        let pw_node = MOD_PARAM_BASE + pw_idx as u32;
        let retrig_node = MOD_PARAM_BASE + retrig_idx as u32;

        if let Some(idx) = if lfo_sync(sync_node) {
            find_idx_by_node(div_node)
        } else {
            find_idx_by_node(rate_node)
        } {
            out.push(idx);
        }
        if let Some(idx) = find_idx_by_node(sync_node) {
            out.push(idx);
        }
        if let Some(idx) = find_idx_by_node(shape_node) {
            out.push(idx);
        }
        if let Some(idx) = find_idx_by_node(retrig_node) {
            out.push(idx);
        }
        if lfo_shape_is_pulse(shape_node) {
            if let Some(idx) = find_idx_by_node(pw_node) {
                out.push(idx);
            }
        }
    };

    push_lfo(
        &mut source_actual,
        PARAM_LFO1_RATE_HZ,
        PARAM_LFO1_SYNC,
        PARAM_LFO1_DIV,
        PARAM_LFO1_SHAPE,
        PARAM_LFO1_PW,
        PARAM_LFO1_RETRIGGER,
    );
    for idx_const in [
        PARAM_ENV_ATTACK_MS,
        PARAM_ENV_DECAY_MS,
        PARAM_ENV_SUSTAIN,
        PARAM_ENV_RELEASE_MS,
    ] {
        if let Some(idx) = find_idx_by_node(MOD_PARAM_BASE + idx_const as u32) {
            source_actual.push(idx);
        }
    }
    if let Some(idx) = if lfo_sync(MOD_PARAM_BASE + PARAM_RAND_SYNC as u32) {
        find_idx_by_node(MOD_PARAM_BASE + PARAM_RAND_DIV as u32)
    } else {
        find_idx_by_node(MOD_PARAM_BASE + PARAM_RAND_RATE_HZ as u32)
    } {
        source_actual.push(idx);
    }
    if let Some(idx) = find_idx_by_node(MOD_PARAM_BASE + PARAM_RAND_SYNC as u32) {
        source_actual.push(idx);
    }
    if let Some(idx) = find_idx_by_node(MOD_PARAM_BASE + PARAM_RAND_SLEW as u32) {
        source_actual.push(idx);
    }
    if let Some(idx) = if lfo_sync(MOD_PARAM_BASE + PARAM_DRIFT_SYNC as u32) {
        find_idx_by_node(MOD_PARAM_BASE + PARAM_DRIFT_DIV as u32)
    } else {
        find_idx_by_node(MOD_PARAM_BASE + PARAM_DRIFT_RATE as u32)
    } {
        source_actual.push(idx);
    }
    if let Some(idx) = find_idx_by_node(MOD_PARAM_BASE + PARAM_DRIFT_SYNC as u32) {
        source_actual.push(idx);
    }
    push_lfo(
        &mut source_actual,
        PARAM_LFO2_RATE_HZ,
        PARAM_LFO2_SYNC,
        PARAM_LFO2_DIV,
        PARAM_LFO2_SHAPE,
        PARAM_LFO2_PW,
        PARAM_LFO2_RETRIGGER,
    );
    push_lfo(
        &mut source_actual,
        PARAM_LFO3_RATE_HZ,
        PARAM_LFO3_SYNC,
        PARAM_LFO3_DIV,
        PARAM_LFO3_SHAPE,
        PARAM_LFO3_PW,
        PARAM_LFO3_RETRIGGER,
    );

    let mut synth_params: Vec<Rc<RefCell<Value>>> = Vec::new();
    let mut mod_params: Vec<Rc<RefCell<Value>>> = Vec::new();
    push_param(
        &mut synth_params,
        "base_note".to_string(),
        "base-note",
        None,
        base_note_current,
        -48.0,
        48.0,
        None,
    );

    for (param_idx, pdesc) in desc.params.iter().enumerate() {
        let default_val = slot.defaults.get(param_idx);
        let current_val = plock_step
            .and_then(|step| slot.plocks.get(step, param_idx))
            .unwrap_or(default_val);
        let options = match &pdesc.kind {
            sequencer::effects::ParamKind::Enum { labels } => Some(labels),
            _ => None,
        };
        if is_source_param(pdesc.node_param_idx) {
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
            );
        }
    }

    let mut source_sections: Vec<Rc<RefCell<Value>>> = Vec::new();
    let mut source_names: Vec<Rc<RefCell<Value>>> = Vec::new();
    for section_name in ["LFO 1", "ENV 1", "RAND", "DRIFT", "LFO 2", "LFO 3"] {
        let mut params: Vec<Rc<RefCell<Value>>> = Vec::new();
        for &param_idx in &source_actual {
            let Some(pdesc) = desc.params.get(param_idx) else {
                continue;
            };
            if source_section_name(pdesc.node_param_idx) != section_name {
                continue;
            }
            let default_val = slot.defaults.get(param_idx);
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
            );
        }
        if params.is_empty() {
            continue;
        }
        source_names.push(Rc::new(RefCell::new(Value::String(
            section_name.to_string(),
        ))));
        let mut section_map: HashMap<String, Rc<RefCell<Value>>> = HashMap::new();
        section_map.insert(
            "name".to_string(),
            Rc::new(RefCell::new(Value::String(section_name.to_string()))),
        );
        section_map.insert(
            "params".to_string(),
            Rc::new(RefCell::new(Value::List(params))),
        );
        source_sections.push(Rc::new(RefCell::new(Value::Map(section_map))));
    }

    let mut panel_map: HashMap<String, Rc<RefCell<Value>>> = HashMap::new();
    let instrument_name =
        current_custom_instrument_name(app, track).unwrap_or_else(|| "Instrument".to_string());
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
    let items: Vec<Rc<RefCell<Value>>> = (0..MAX_STEPS)
        .map(|s| Rc::new(RefCell::new(Value::Bool(set.contains(&s)))))
        .collect();
    Value::List(items)
}

/// Build list of available effect names from the effects/ directory.
/// Prepends "+ New Effect" as a special entry for inline creation.
pub(crate) fn build_available_effects() -> Value {
    let names = sequencer::lisp_effect::list_saved_effects();
    let mut items: Vec<Rc<RefCell<Value>>> = vec![Rc::new(RefCell::new(Value::String(
        "+ New Effect".to_string(),
    )))];
    items.extend(
        names
            .into_iter()
            .map(|n| Rc::new(RefCell::new(Value::String(n)))),
    );
    Value::List(items)
}

pub(crate) fn build_available_midi_effects() -> Value {
    let mut names: Vec<String> = sequencer::lisp_effect::load_midi_fx_descriptors()
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
    sequencer::lisp_effect::load_midi_fx_descriptor(fx_name)
        .and_then(|desc| desc.params.get(param_idx).cloned())
        .and_then(|param| match param.kind {
            sequencer::effects::ParamKind::Enum { labels } => {
                labels.iter().position(|item| item == label)
            }
            _ => None,
        })
}

pub(crate) fn master_meter_level(peak: f32) -> f64 {
    if peak <= 0.0 {
        0.0
    } else {
        peak.sqrt().min(1.2) as f64
    }
}

pub(crate) fn quantize_meter_level(level: f64) -> f64 {
    ((level.clamp(0.0, 1.2) * METER_LEVEL_STEPS).round()) / METER_LEVEL_STEPS
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

pub(crate) fn push_project_scratch_to_named_buffer(editor: &mut Editor, app: &ui::App) {
    let scratch_text = app.editor.scratch_buffer.clone();
    let scratch_cursor = app.editor.scratch_cursor;

    editor.upsert_scratch_buffer(PROJECT_SCRATCH_BUFFER_NAME, &scratch_text);

    if editor.active_buffer().name == PROJECT_SCRATCH_BUFFER_NAME {
        let buffer = editor.active_buffer_mut();
        let row = scratch_cursor.0.min(buffer.lines.len().saturating_sub(1));
        let col = scratch_cursor.1.min(buffer.lines[row].len());
        buffer.cursor = (row, col);
    }
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
            .sampler_paths
            .get(track)
            .and_then(|path| path.as_ref())
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
    let presets = sequencer::lisp_effect::load_instrument_presets(&instrument_name)
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
    if let Value::Map(map) = payload {
        if let Some(cell) = map.get("path") {
            if let Value::String(s) = &*cell.borrow() {
                return Some(s.clone());
            }
        }
    }
    None
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
    let selected_step = {
        let sel = selected.lock().unwrap();
        sel.iter().copied().min()
    };
    rt.set_reactive("SEQ", "tp-attack", Value::Number(tp.get_attack_ms() as f64));
    rt.set_reactive(
        "SEQ",
        "tp-release",
        Value::Number(tp.get_release_ms() as f64),
    );
    let swing = selected_step
        .and_then(|step| state.pattern.swing_plocks[track].get(step))
        .unwrap_or_else(|| tp.get_swing());
    rt.set_reactive("SEQ", "tp-swing", Value::Number(swing as f64));
    rt.set_reactive("SEQ", "tp-send", Value::Number(tp.get_send() as f64));
    rt.set_reactive(
        "SEQ",
        "tp-num-steps",
        Value::Number(tp.get_num_steps() as f64),
    );
    rt.set_reactive("SEQ", "tp-gate", Value::Bool(tp.is_gate_on()));
    rt.set_reactive("SEQ", "tp-poly", Value::Bool(tp.is_polyphonic()));
    // Resolve timebase: show p-locked value from first selected step, otherwise track default
    let timebase_label = selected_step
        .and_then(|step| state.pattern.timebase_plocks[track].get(step))
        .unwrap_or_else(|| tp.get_timebase())
        .label()
        .to_string();
    rt.set_reactive("SEQ", "tp-timebase", Value::String(timebase_label));
    let swing_resolution = selected_step
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
}

/// Build a Lisp Value::Map of track parameters for the current track.
pub(crate) fn build_track_params(state: &Arc<SequencerState>, track: usize) -> Value {
    use std::collections::HashMap;
    let tp = &state.pattern.track_params[track];
    eprintln!(
        "build_track_params: track={track} attack={} gate={} vol={}",
        tp.get_attack_ms(),
        tp.is_gate_on(),
        tp.get_volume()
    );
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
    Value::Map(map)
}

/// Build a Lisp Value::List of bools indicating which steps have any p-locks on the given track.
pub(crate) fn build_step_has_plocks(
    state: &Arc<SequencerState>,
    track: usize,
    descriptors: &[Vec<sequencer::effects::EffectDescriptor>],
) -> Value {
    let chain = &state.pattern.effect_chains[track];
    let midi_fx_slots = &state.pattern.midi_fx_slots[track];
    let num_slots = descriptors.get(track).map(|d| d.len()).unwrap_or(0);
    let instrument_slot = &state.pattern.instrument_slots[track];
    let instrument_num_params = instrument_slot.num_params.load(Ordering::Relaxed) as usize;
    let timebase_plocks = &state.pattern.timebase_plocks[track];
    let swing_plocks = &state.pattern.swing_plocks[track];
    let swing_resolution_plocks = &state.pattern.swing_resolution_plocks[track];
    let items: Vec<Rc<RefCell<Value>>> = (0..MAX_STEPS)
        .map(|step| {
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
            let has_plock = effect_has_plock
                || midi_fx_has_plock
                || instrument_has_plock
                || timebase_plocks.has_plock(step)
                || swing_plocks.has_plock(step)
                || swing_resolution_plocks.has_plock(step);
            Rc::new(RefCell::new(Value::Bool(has_plock)))
        })
        .collect();
    Value::List(items)
}

#[cfg(test)]
mod tests {
    use super::*;
    use eseqlisp::parser::{ASTParser, Parser, ParserError, Token};

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
    fn metal_seq_mixer_lisp_parses() {
        let src = std::fs::read_to_string("metal-seq-mixer.lisp").expect("read mixer lisp");
        let tokens = Parser::new(src)
            .parse()
            .expect("tokenize metal-seq-mixer.lisp");
        let mut pos = 0;
        while pos < tokens.len() {
            if let Err(err) = parse_expression_at(&tokens, &mut pos) {
                let start = pos.saturating_sub(8);
                let end = (pos + 8).min(tokens.len());
                panic!(
                    "parse metal-seq-mixer.lisp at token {pos}: {err:?}\ncontext: {:?}",
                    &tokens[start..end]
                );
            }
        }
        ASTParser::new(tokens)
            .parse()
            .expect("parse metal-seq-mixer.lisp");
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

        assert_eq!(state.pattern.chord_data[track].count(step), 0);
        assert!(state.pattern.patterns[track].is_active(step));
        assert_eq!(
            state.pattern.step_data[track].get(step, StepParam::Transpose),
            7.0
        );
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
                (def base-note ()
                  (label "base" :font-size 10 :color :gray :bg :transparent))
                "#,
            )
            .expect("load custom UI test helpers");

        let custom_ui_source = build_custom_instrument_ui_source_with_overlay(None);
        runtime
            .eval_str(&custom_ui_source)
            .expect("load custom instrument UIs");

        for instrument_name in [
            "emulations/dx7-4op/",
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
                        (str "custom-midi-fx-ui-" midi-fx-ui-current-name "-" name))
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
        rt.run_reactive_cycle();
        editor.refresh_runtime_side_effects();
        fx_epoch.fetch_add(1, Ordering::Relaxed);
        editor.handle_host_event(HostEvent::Status(status));
    }
}
