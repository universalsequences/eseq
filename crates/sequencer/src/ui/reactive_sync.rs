use super::*;

pub(super) fn sync_after_instrument_track_apply(
    app: &mut app::App,
    editor: &mut Editor,
    state: &Arc<SequencerState>,
    track_index: usize,
    current_track: &Arc<AtomicUsize>,
    track_names: &mut Vec<String>,
    track_pan_ids: &Arc<Mutex<Vec<i32>>>,
    record_armed: &Arc<Mutex<Vec<bool>>>,
    selected_steps: &Arc<Mutex<HashSet<usize>>>,
    accumulator_names: &Arc<Mutex<Vec<String>>>,
    cached_track_peak_levels: &[f64],
    cached_bus_peak_levels: &[f64],
    ui_epoch: &Arc<AtomicUsize>,
    lg_raw: *mut sequencer::audiograph::LiveGraph,
) {
    sync_after_instrument_track_apply_with_selection(
        app, editor, state, track_index, current_track, track_names, track_pan_ids,
        record_armed, selected_steps, accumulator_names, cached_track_peak_levels,
        cached_bus_peak_levels, ui_epoch, lg_raw, false,
    );
}

pub(super) fn sync_after_instrument_track_apply_with_selection(
    app: &mut app::App,
    editor: &mut Editor,
    state: &Arc<SequencerState>,
    track_index: usize,
    current_track: &Arc<AtomicUsize>,
    track_names: &mut Vec<String>,
    track_pan_ids: &Arc<Mutex<Vec<i32>>>,
    record_armed: &Arc<Mutex<Vec<bool>>>,
    selected_steps: &Arc<Mutex<HashSet<usize>>>,
    accumulator_names: &Arc<Mutex<Vec<String>>>,
    cached_track_peak_levels: &[f64],
    cached_bus_peak_levels: &[f64],
    ui_epoch: &Arc<AtomicUsize>,
    lg_raw: *mut sequencer::audiograph::LiveGraph,
    preserve_track_selection: bool,
) {
    let selected_track = host_commands::selection_after_track_apply(
        track_index,
        preserve_track_selection,
        current_track,
        app.tracks.len(),
    );
    current_track.store(selected_track, Ordering::Relaxed);
    app.ui.cursor_track = selected_track;
    let track_name = app.tracks[track_index].clone();
    if track_names.len() < app.tracks.len() {
        track_names.push(track_name);
    } else if let Some(name) = track_names.get_mut(track_index) {
        *name = track_name;
    }
    {
        let mut pan_ids = track_pan_ids.lock().unwrap();
        if pan_ids.len() < app.graph.track_node_ids.len() {
            pan_ids.push(app.graph.track_node_ids[track_index].pan_id);
        }
        push_solo_mutes(lg_raw, app, state);
    }
    if record_armed.lock().unwrap().len() < app.tracks.len() {
        record_armed.lock().unwrap().push(false);
    }

    let rt = editor.runtime_mut();
    rt.set_reactive("SEQ", "num-tracks", Value::Number(track_names.len() as f64));
    rt.set_reactive("SEQ", "track-ids", build_track_ids(app));
    set_current_track_reactive(rt, app.tracks.len(), selected_track);
    rt.set_reactive("SEQ", "track-names", build_track_names(track_names));
    sync_all_track_sequencer_state(rt, state, app, selected_track, selected_steps);
    rt.set_reactive("SEQ", "steps", build_steps_value(state, selected_track));
    sync_step_param_lists(rt, state, selected_track);
    sync_track_mixer_state(rt, app, state);
    sync_bus_mixer_state(rt, app);
    sync_track_peak_fields(rt, cached_track_peak_levels);
    sync_bus_peak_fields(rt, cached_bus_peak_levels);
    rt.set_reactive(
        "SEQ",
        "effects",
        build_effects_value(
            state,
            selected_track,
            &app.graph.effect_descriptors,
            selected_steps,
        ),
    );
    rt.set_reactive(
        "SEQ",
        "midi-effects",
        build_midi_effects_value(state, selected_track, selected_steps),
    );
    rt.set_reactive(
        "SEQ",
        "instrument-panel",
        build_instrument_panel_value(app, selected_track, selected_steps),
    );
    *accumulator_names.lock().unwrap() = build_accumulator_names(app);
    sync_track_params(rt, app, state, selected_track, selected_steps);
    sync_selected_track_bus_send_binding_fields(
        rt,
        app,
        state,
        selected_track,
        selected_steps,
    );
    sync_fx_param_binding_fields(rt, app, state, selected_track, selected_steps);
    rt.set_reactive(
        "SEQ",
        "step-has-plocks",
        build_step_has_plocks(state, selected_track, &app.graph.effect_descriptors),
    );
    sync_sidebar_browser(rt, app, selected_track);
    rt.run_reactive_cycle();
    editor.refresh_runtime_side_effects();
    refresh_visible_track_topology_layouts(editor);
    ui_epoch.fetch_add(1, Ordering::Relaxed);
}

pub(super) fn refresh_visible_track_topology_layouts(editor: &mut Editor) {
    for buffer_name in [
        "*sequencer*",
        "*samples*",
        "*mixer*",
        "*patch-mixer*",
        "*track*",
        "*fx*",
        "*piano-roll*",
    ] {
        editor.refresh_visible_layouts_for_buffer_named(buffer_name);
    }
}

pub(super) fn refresh_instrument_panel_reactive(
    editor: &mut Editor,
    app: &app::App,
    track: usize,
    selected_steps: &Arc<Mutex<HashSet<usize>>>,
    ui_epoch: &AtomicUsize,
) {
    let selected_step = selected_plock_step(selected_steps);
    let display_step = displayed_plock_step(&app.state, track, selected_step);
    let rt = editor.runtime_mut();
    let mut dirty = sync_all_rack_slot_selection_binding_fields(rt, app);
    dirty |= rt
        .set_reactive(
            "SEQ",
            "instrument-panel",
            build_instrument_panel_value(app, track, selected_steps),
        )
        .effects_dirty;
    dirty |= sync_rack_macro_value_fields(rt, app, track, display_step);
    dirty |= sync_rack_panel_param_value_fields(rt, app, track, display_step);
    if dirty {
        editor.refresh_runtime_side_effects();
        editor.mark_needs_redraw();
    }
    ui_epoch.fetch_add(1, Ordering::Relaxed);
}

pub(super) fn refresh_rack_macro_value_reactive(
    editor: &mut Editor,
    app: &app::App,
    track: usize,
    id: sequencer::sequencer::RackMacroId,
    selected_steps: &Arc<Mutex<HashSet<usize>>>,
    ui_epoch: &AtomicUsize,
) {
    let display_step = displayed_plock_step(&app.state, track, selected_plock_step(selected_steps));
    let rt = editor.runtime_mut();
    let mut dirty = sync_rack_macro_value_field(rt, app, track, id, display_step);
    dirty |= sync_rack_macro_target_value_fields(rt, app, track, id, display_step);
    flush_reactive_display_edit(editor, dirty);
    ui_epoch.fetch_add(1, Ordering::Relaxed);
}

pub(super) fn refresh_rack_macro_plock_reactive(
    editor: &mut Editor,
    app: &app::App,
    state: &Arc<SequencerState>,
    track: usize,
    id: sequencer::sequencer::RackMacroId,
    selected_steps: &Arc<Mutex<HashSet<usize>>>,
    rebuild_plock_rows: bool,
) {
    let display_step = displayed_plock_step(state, track, selected_plock_step(selected_steps));
    let rt = editor.runtime_mut();
    let mut dirty = sync_rack_macro_value_field(rt, app, track, id, display_step);
    dirty |= sync_rack_macro_target_value_fields(rt, app, track, id, display_step);
    if rebuild_plock_rows {
        let result = rt.set_reactive(
            "SEQ",
            "track-plocks",
            build_track_plocks_value(app, state, track, selected_steps),
        );
        dirty |= result.effects_dirty || result.widgets_dirty;
    }
    flush_reactive_display_edit(editor, dirty);
}

#[derive(Clone, Copy)]
pub(super) enum RackDirectDisplayTarget {
    SlotParam {
        slot_idx: usize,
        param: RackSlotParam,
    },
    InstrumentParam {
        slot_idx: usize,
        param_idx: usize,
    },
    EffectParam {
        rack_slot: usize,
        effect_slot: usize,
        param_idx: usize,
    },
}

pub(super) fn refresh_rack_direct_param_reactive(
    editor: &mut Editor,
    app: &app::App,
    state: &Arc<SequencerState>,
    track: usize,
    target: RackDirectDisplayTarget,
    selected_steps: &Arc<Mutex<HashSet<usize>>>,
    sync_plock_rows: bool,
    ui_epoch: &AtomicUsize,
) {
    let display_step = displayed_plock_step(state, track, selected_plock_step(selected_steps));
    let rt = editor.runtime_mut();
    let mut dirty = match target {
        RackDirectDisplayTarget::SlotParam { slot_idx, param } => {
            sync_rack_slot_control_value_field(rt, app, track, slot_idx, param, display_step)
        }
        RackDirectDisplayTarget::InstrumentParam {
            slot_idx,
            param_idx,
        } => sync_rack_slot_instrument_param_value_field(
            rt,
            app,
            track,
            slot_idx,
            param_idx,
            display_step,
        ),
        RackDirectDisplayTarget::EffectParam {
            rack_slot,
            effect_slot,
            param_idx,
        } => sync_rack_slot_effect_param_value_field(
            rt,
            app,
            track,
            rack_slot,
            effect_slot,
            param_idx,
            display_step,
        ),
    };
    if sync_plock_rows {
        let result = rt.set_reactive(
            "SEQ",
            "track-plocks",
            build_track_plocks_value(app, state, track, selected_steps),
        );
        dirty |= result.effects_dirty || result.widgets_dirty;
    }
    flush_reactive_display_edit(editor, dirty);
    ui_epoch.fetch_add(1, Ordering::Relaxed);
}

pub(super) fn apply_rack_macro_host_command(
    name: &str,
    map: &HashMap<String, Rc<RefCell<Value>>>,
    editor: &mut Editor,
    app: &mut app::App,
    state: &Arc<SequencerState>,
    selected_steps: &Arc<Mutex<HashSet<usize>>>,
    ui_epoch: &AtomicUsize,
    fx_epoch: &AtomicUsize,
) -> bool {
    let (Some(track), Some(id), Some(value)) = (
        map_usize(map, "track"),
        map_usize(map, "id").and_then(sequencer::sequencer::RackMacroId::from_index),
        map_number(map, "value").map(|value| value as f32),
    ) else {
        return false;
    };
    match name {
        "set-rack-macro-value" => {
            if !app.set_rack_macro_value(track, id, value) {
                return false;
            }
            refresh_rack_macro_value_reactive(editor, app, track, id, selected_steps, ui_epoch);
        }
        "set-rack-macro-plock" => {
            let steps = selected_steps
                .lock()
                .unwrap()
                .iter()
                .copied()
                .collect::<Vec<_>>();
            let display_step =
                displayed_plock_step(state, track, selected_plock_step(selected_steps));
            let plock_row_exists = {
                let racks = state.pattern.rack_tracks.lock().unwrap();
                display_step.is_some_and(|step| {
                    racks
                        .get(track)
                        .and_then(Option::as_ref)
                        .and_then(|rack| rack.macros.get(id.index()))
                        .and_then(|rack_macro| rack_macro.plocks.get(step))
                        .is_some_and(Option::is_some)
                })
            };
            let outcome = app::try_apply_command(
                app,
                app::AppCommand::SetRackMacroPlockMulti {
                    track,
                    steps,
                    macro_idx: id.index(),
                    value,
                },
            );
            if !outcome.is_ok_and(|outcome| outcome != app::edit::EditOutcome::NoOp) {
                return false;
            }
            refresh_rack_macro_plock_reactive(
                editor,
                app,
                state,
                track,
                id,
                selected_steps,
                !plock_row_exists,
            );
            fx_epoch.fetch_add(1, Ordering::Relaxed);
            ui_epoch.fetch_add(1, Ordering::Relaxed);
        }
        _ => return false,
    }
    true
}

pub(super) fn sync_rack_slot_instrument_authoring_display(
    editor: &mut Editor,
    app: &app::App,
    state: &Arc<SequencerState>,
    track: usize,
    selected_steps: &Arc<Mutex<HashSet<usize>>>,
) {
    let rt = editor.runtime_mut();
    let mut dirty = rt
        .set_reactive(
            "SEQ",
            "instrument-panel",
            build_instrument_panel_value(app, track, selected_steps),
        )
        .effects_dirty;
    let display_step = displayed_plock_step(state, track, selected_plock_step(selected_steps));
    dirty |= sync_rack_macro_value_fields(rt, app, track, display_step);
    dirty |= sync_rack_panel_param_value_fields(rt, app, track, display_step);
    dirty |= sync_instrument_plock_presence_fields(
        rt,
        state,
        &app.graph.effect_descriptors,
        track,
        selected_steps,
    );
    flush_reactive_display_edit(editor, dirty);
}

pub(super) fn step_param_fields(param: StepParam) -> Option<(&'static str, &'static str, usize)> {
    match param {
        StepParam::Velocity => Some(("velocities", "track-velocities", 0)),
        StepParam::Duration => Some(("durations", "track-durations", 1)),
        StepParam::AuxA => Some(("auxas", "track-auxas", 2)),
        StepParam::Transpose => Some(("transposes", "track-transposes", 3)),
        StepParam::Pan => Some(("pans", "track-pans", 4)),
        StepParam::Sync => Some(("syncs", "track-syncs", 5)),
        StepParam::Delay => Some(("delays", "track-delays", 6)),
        _ => None,
    }
}

pub(super) fn step_param_slider_value(param: StepParam, value: f32) -> f64 {
    if param == StepParam::Duration {
        param.normalize(value) as f64
    } else {
        value as f64
    }
}

pub(super) fn sync_track_step_param_list_bindings(
    rt: &mut Runtime,
    state: &Arc<SequencerState>,
    track: usize,
    current_track_idx: usize,
) -> bool {
    let mut dirty = false;
    for param in [
        StepParam::Velocity,
        StepParam::Duration,
        StepParam::AuxA,
        StepParam::Transpose,
        StepParam::Pan,
        StepParam::Sync,
        StepParam::Delay,
    ] {
        let Some((current_field, track_field, _)) = step_param_fields(param) else {
            continue;
        };
        let value = build_param_list(state, track, param);
        dirty |= rt
            .set_reactive_list_index("SEQ", track_field, track, value.clone())
            .effects_dirty;
        if track == current_track_idx {
            dirty |= rt.set_reactive("SEQ", current_field, value).effects_dirty;
        }
    }
    dirty
}

/// The per-track slice of `sync_track_step_param_list_bindings` for a SINGLE
/// param, minus the current-track flat list.
///
/// `sync_single_step_param_binding` already writes the flat
/// `SEQ.{velocities,durations,...}` list at the edited INDEX, which is the
/// cheap index-aware write; rewriting the whole list here would re-dirty every
/// effect that reads it (that whole-list rewrite is exactly what made a
/// velocity drag cost ~4.4ms of reactive cycle under the old ui_epoch resync).
/// The list-of-lists `SEQ.track-{velocities,durations,...}` has no per-index
/// writer, though, and `seqv-track-param-values` reads it for every
/// non-current track's expanded lane, so it still needs the per-track write.
pub(super) fn sync_track_step_param_list_binding_for_param(
    rt: &mut Runtime,
    state: &Arc<SequencerState>,
    track: usize,
    param: StepParam,
) -> bool {
    let Some((_, track_field, _)) = step_param_fields(param) else {
        return false;
    };
    rt.set_reactive_list_index(
        "SEQ",
        track_field,
        track,
        build_param_list(state, track, param),
    )
    .effects_dirty
}

/// `track-duration-spans` at one track index — the list-of-lists half of the
/// duration-bar surface whose per-step half is
/// `sync_track_duration_span_binding_fields`.
pub(super) fn sync_track_duration_spans_list_binding(
    rt: &mut Runtime,
    state: &Arc<SequencerState>,
    track: usize,
) -> bool {
    rt.set_reactive_list_index(
        "SEQ",
        "track-duration-spans",
        track,
        build_track_duration_spans_value(state, track),
    )
    .effects_dirty
}

pub(super) fn sync_single_step_param_binding(
    rt: &mut Runtime,
    state: &Arc<SequencerState>,
    track: usize,
    step: usize,
    param: StepParam,
    current_track_idx: usize,
    selected_steps: &Arc<Mutex<HashSet<usize>>>,
    expanded_step_projection: &Arc<ExpandedStepProjectionRegistry>,
) -> bool {
    let Some((current_field, _, mode)) = step_param_fields(param) else {
        return false;
    };
    let value = state.pattern.step_data[track].get(step, param);
    let mut dirty = false;
    dirty |= rt
        .set_reactive(
            "SEQ",
            &track_step_param_slider_field(track, mode, step),
            Value::Number(step_param_slider_value(param, value)),
        )
        .effects_dirty;
    dirty |= rt
        .set_reactive(
            "SEQ",
            &track_step_param_haptic_field(track, mode, step),
            Value::Number(value as f64),
        )
        .effects_dirty;
    if track == current_track_idx {
        dirty |= rt
            .set_reactive_list_index("SEQ", current_field, step, Value::Number(value as f64))
            .effects_dirty;
        let parameter_step = selected_plock_step(selected_steps)
            .unwrap_or_else(|| fx_step_cursor_from_runtime(rt));
        if parameter_step == step {
            if let Some(field) = fx_step_param_value_field(param) {
                dirty |= rt
                    .set_reactive("SEQ", field, Value::Number(value as f64))
                    .effects_dirty;
            }
        }
    }
    for viewport in expanded_step_projection.viewports_for_track(track) {
        dirty |= sync_expanded_step_cursor_param_change(rt, state, viewport, mode, step);
        if let Some(slot) = visible_slot_for_step(viewport, step) {
            dirty |= sync_expanded_step_param_slot(rt, state, viewport, mode, slot);
        }
    }
    dirty
}

pub(super) fn sync_single_track_step_binding_fields(
    rt: &mut Runtime,
    state: &Arc<SequencerState>,
    app: &app::App,
    track: usize,
    current_track_idx: usize,
    selected_steps: &Arc<Mutex<HashSet<usize>>>,
    plock_mask: &[u64; MAX_STEPS / 64],
) -> bool {
    const WORDS: usize = MAX_STEPS / 64;
    if track >= app.tracks.len() {
        return false;
    }

    let num_steps = state.pattern.track_params[track]
        .get_num_steps()
        .min(MAX_STEPS);
    let pattern_bits = state.pattern.patterns[track].load_bits();
    let selected = selected_steps.lock().unwrap();
    let mut active_mask = [0u64; WORDS];
    let mut duration_mask = [0u64; WORDS];
    let mut plocked_mask = [0u64; WORDS];
    let mut selected_mask = [0u64; WORDS];
    let mut max_reach = f64::NEG_INFINITY;
    for step in 0..MAX_STEPS {
        let word = step / 64;
        let bit = 1u64 << (step % 64);
        let visible = step < num_steps;
        let is_active = pattern_bits[word] & bit != 0;
        if is_active {
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
            if plock_mask[word] & bit != 0 {
                plocked_mask[word] |= bit;
            }
            if track == current_track_idx && selected.contains(&step) {
                selected_mask[word] |= bit;
            }
        }
    }

    let mut rev = String::with_capacity(WORDS * 4 * 16 + 3);
    for mask in [&active_mask, &duration_mask, &plocked_mask, &selected_mask] {
        for word in mask.iter() {
            use std::fmt::Write as _;
            let _ = write!(rev, "{word:016x}");
        }
    }
    let rev_result = rt.set_reactive(
        "SEQ",
        &track_step_binding_rev_field(track),
        Value::String(rev),
    );
    let mut dirty = rev_result.effects_dirty;
    if !rev_result.changed {
        return dirty;
    }

    for step in 0..MAX_STEPS {
        let word = step / 64;
        let bit = 1u64 << (step % 64);
        dirty |= rt
            .set_reactive(
                "SEQ",
                &track_step_active_field(track, step),
                Value::Bool(active_mask[word] & bit != 0),
            )
            .effects_dirty;
        dirty |= rt
            .set_reactive(
                "SEQ",
                &track_step_duration_field(track, step),
                Value::Bool(duration_mask[word] & bit != 0),
            )
            .effects_dirty;
        dirty |= rt
            .set_reactive(
                "SEQ",
                &track_step_plocked_field(track, step),
                Value::Bool(plocked_mask[word] & bit != 0),
            )
            .effects_dirty;
        dirty |= rt
            .set_reactive(
                "SEQ",
                &track_step_selected_field(track, step),
                Value::Bool(selected_mask[word] & bit != 0),
            )
            .effects_dirty;
    }
    dirty
}

pub(super) fn sync_single_track_sequencer_state(
    rt: &mut Runtime,
    state: &Arc<SequencerState>,
    app: &app::App,
    track: usize,
    current_track_idx: usize,
    selected_steps: &Arc<Mutex<HashSet<usize>>>,
    expanded_step_projection: &Arc<ExpandedStepProjectionRegistry>,
) -> bool {
    if track >= app.tracks.len() {
        return false;
    }

    let mut dirty = false;
    dirty |= rt
        .set_reactive_list_index("SEQ", "track-steps", track, build_steps_value(state, track))
        .effects_dirty;
    dirty |= rt
        .set_reactive_list_index(
            "SEQ",
            "track-duration-spans",
            track,
            build_track_duration_spans_value(state, track),
        )
        .effects_dirty;

    let plock_mask = track_step_plock_mask(state, track, &app.graph.effect_descriptors);
    let step_has_plocks = build_step_has_plocks_from_mask(&plock_mask);
    dirty |= rt
        .set_reactive_list_index(
            "SEQ",
            "track-step-has-plocks",
            track,
            step_has_plocks.clone(),
        )
        .effects_dirty;
    let step_plock_kinds = build_step_plock_kinds(state, track);
    let step_variant_r = build_step_variant_color_channel(state, track, 0);
    let step_variant_g = build_step_variant_color_channel(state, track, 1);
    let step_variant_b = build_step_variant_color_channel(state, track, 2);
    dirty |= rt
        .set_reactive_list_index(
            "SEQ",
            "track-step-plock-kinds",
            track,
            step_plock_kinds.clone(),
        )
        .effects_dirty;
    dirty |= rt
        .set_reactive_list_index("SEQ", "track-step-variant-r", track, step_variant_r.clone())
        .effects_dirty;
    dirty |= rt
        .set_reactive_list_index("SEQ", "track-step-variant-g", track, step_variant_g.clone())
        .effects_dirty;
    dirty |= rt
        .set_reactive_list_index("SEQ", "track-step-variant-b", track, step_variant_b.clone())
        .effects_dirty;
    dirty |= sync_track_step_param_list_bindings(rt, state, track, current_track_idx);

    if track == current_track_idx {
        dirty |= rt
            .set_reactive("SEQ", "steps", build_steps_value(state, track))
            .effects_dirty;
        dirty |= rt
            .set_reactive("SEQ", "step-has-plocks", step_has_plocks)
            .effects_dirty;
        dirty |= rt
            .set_reactive("SEQ", "step-plock-kinds", step_plock_kinds)
            .effects_dirty;
        dirty |= rt
            .set_reactive("SEQ", "step-variant-r", step_variant_r)
            .effects_dirty;
        dirty |= rt
            .set_reactive("SEQ", "step-variant-g", step_variant_g)
            .effects_dirty;
        dirty |= rt
            .set_reactive("SEQ", "step-variant-b", step_variant_b)
            .effects_dirty;
    }

    dirty |= sync_single_track_step_binding_fields(
        rt,
        state,
        app,
        track,
        current_track_idx,
        selected_steps,
        &plock_mask,
    );
    dirty |= sync_expanded_step_viewports_for_track(
        rt,
        state,
        app,
        selected_steps,
        current_track_idx,
        expanded_step_projection,
        track,
    );
    dirty
}

pub(super) fn sync_single_step_structural_bindings(
    rt: &mut Runtime,
    state: &Arc<SequencerState>,
    app: &app::App,
    track: usize,
    step: usize,
    current_track_idx: usize,
    selected_steps: &Arc<Mutex<HashSet<usize>>>,
    expanded_step_projection: &Arc<ExpandedStepProjectionRegistry>,
) -> bool {
    sync_step_batch_structural_bindings(
        rt,
        state,
        app,
        track,
        &[step],
        current_track_idx,
        selected_steps,
        expanded_step_projection,
    )
}

pub(super) fn sync_step_batch_structural_bindings(
    rt: &mut Runtime,
    state: &Arc<SequencerState>,
    app: &app::App,
    track: usize,
    steps: &[usize],
    current_track_idx: usize,
    selected_steps: &Arc<Mutex<HashSet<usize>>>,
    expanded_step_projection: &Arc<ExpandedStepProjectionRegistry>,
) -> bool {
    if track >= app.tracks.len() || steps.is_empty() {
        return false;
    }
    // Direct per-step writes bypass the per-track lane digest used by
    // sync_all_track_step_binding_fields; invalidate it so the next full sync
    // rewrites this track.
    let _ = rt.set_reactive("SEQ", &track_step_binding_rev_field(track), Value::Nil);
    let num_steps = state.pattern.track_params[track]
        .get_num_steps()
        .min(MAX_STEPS);
    let selected = selected_steps.lock().unwrap();
    let mut dirty = false;
    let render_values = plock_variant_step_render_values(state, track);
    for &step in steps {
        if step >= MAX_STEPS {
            continue;
        }
        let visible = step < num_steps;
        dirty |= rt
            .set_reactive(
                "SEQ",
                &track_step_active_field(track, step),
                Value::Bool(visible && state.pattern.patterns[track].is_active(step)),
            )
            .effects_dirty;
        dirty |= rt
            .set_reactive(
                "SEQ",
                &track_step_duration_field(track, step),
                Value::Bool(visible && track_step_duration_covered(state, track, step)),
            )
            .effects_dirty;
        dirty |= rt
            .set_reactive(
                "SEQ",
                &track_step_plocked_field(track, step),
                Value::Bool(
                    visible
                        && track_step_has_plock(
                            state,
                            track,
                            &app.graph.effect_descriptors,
                            step,
                        ),
                ),
            )
            .effects_dirty;
        dirty |= rt
            .set_reactive(
                "SEQ",
                &track_step_selected_field(track, step),
                Value::Bool(visible && track == current_track_idx && selected.contains(&step)),
            )
            .effects_dirty;
        dirty |= sync_track_step_plock_render_fields(rt, track, step, render_values[step]);
        for viewport in expanded_step_projection.viewports_for_track(track) {
            if let Some(slot) = visible_slot_for_step(viewport, step) {
                dirty |= sync_expanded_step_slot(
                    rt,
                    state,
                    app,
                    &selected,
                    current_track_idx,
                    viewport,
                    slot,
                    &render_values,
                );
            }
        }
    }
    if track == current_track_idx {
        let cursor_step = fx_step_cursor_from_runtime(rt);
        dirty |= sync_fx_step_cursor_binding_fields(
            rt,
            state,
            track,
            cursor_step,
            selected.iter().copied().min(),
            selected.len(),
        );
    }
    dirty
}

/// Accumulate a `(track, steps)` entry for a deferred per-track fan-out inside
/// `apply_ui_invalidations`, de-duplicating both the track and the step.
fn push_deferred_track_step(entries: &mut Vec<(usize, Vec<usize>)>, track: usize, step: usize) {
    match entries.iter_mut().find(|(entry, _)| *entry == track) {
        Some((_, steps)) => {
            if !steps.contains(&step) {
                steps.push(step);
            }
        }
        None => entries.push((track, vec![step])),
    }
}

/// Write the compact step shell's per-step p-lock *render* bindings
/// (`seq-track-step-plock-kind-{track}-{step}` plus the three
/// `seq-track-step-variant-{r,g,b}-{track}-{step}` fields).
///
/// These are the fields the compact grid's tick and variant tint bind to. They
/// are otherwise only published by the full `ui_epoch`-driven sync
/// (`sync_all_track_step_binding_fields_inner`) and by
/// `sync_step_batch_structural_bindings`; the p-lock authoring path publishes
/// them through this helper so the tick appears on the first knob touch.
/// Values/gating match the full sync exactly (render values are written
/// ungated by step visibility, like the full sync does).
pub(super) fn sync_track_step_plock_render_fields(
    rt: &mut Runtime,
    track: usize,
    step: usize,
    render: PlockVariantStepRender,
) -> bool {
    let mut dirty = rt
        .set_reactive(
            "SEQ",
            &track_step_plock_kind_field(track, step),
            Value::Number(render.kind as f64),
        )
        .effects_dirty;
    for (channel, value) in ['r', 'g', 'b'].into_iter().zip(render.color) {
        dirty |= rt
            .set_reactive(
                "SEQ",
                &track_step_variant_color_field(track, step, channel),
                Value::Number(value as f64),
            )
            .effects_dirty;
    }
    dirty
}

pub(super) fn sync_track_duration_span_binding_fields(
    rt: &mut Runtime,
    state: &Arc<SequencerState>,
    track: usize,
    start_step: usize,
) -> bool {
    let _ = rt.set_reactive("SEQ", &track_step_binding_rev_field(track), Value::Nil);
    let num_steps = state.pattern.track_params[track]
        .get_num_steps()
        .min(MAX_STEPS);
    let mut dirty = false;
    for step in start_step.min(MAX_STEPS)..MAX_STEPS {
        dirty |= rt
            .set_reactive(
                "SEQ",
                &track_step_duration_field(track, step),
                Value::Bool(step < num_steps && track_step_duration_covered(state, track, step)),
            )
            .effects_dirty;
    }
    dirty
}

pub(super) fn sync_step_selection_bindings(
    rt: &mut Runtime,
    state: &Arc<SequencerState>,
    app: Option<&app::App>,
    track: usize,
    selected_steps: &Arc<Mutex<HashSet<usize>>>,
    current_track_idx: usize,
    expanded_step_projection: &Arc<ExpandedStepProjectionRegistry>,
    changed_steps: &[usize],
    sync_legacy_list: bool,
) -> bool {
    let _ = rt.set_reactive("SEQ", &track_step_binding_rev_field(track), Value::Nil);
    let selected = selected_steps.lock().unwrap();
    let num_steps = state.pattern.track_params[track]
        .get_num_steps()
        .min(MAX_STEPS);
    let cursor_step = fx_step_cursor_from_runtime(rt);
    let mut dirty = sync_fx_step_cursor_binding_fields(
        rt,
        state,
        track,
        cursor_step,
        selected.iter().copied().min(),
        selected.len(),
    );
    for &step in changed_steps {
        if step >= MAX_STEPS {
            continue;
        }
        let is_selected = step < num_steps && selected.contains(&step);
        dirty |= rt
            .set_reactive(
                "SEQ",
                &track_step_selected_field(track, step),
                Value::Bool(is_selected),
            )
            .effects_dirty;
        if sync_legacy_list {
            dirty |= rt
                .set_reactive_list_index("SEQ", "selected-steps", step, Value::Bool(is_selected))
                .effects_dirty;
        }
    }
    if let Some(app) = app {
        let viewports = expanded_step_projection.viewports_for_track(track);
        if !viewports.is_empty() {
            let render_values = plock_variant_step_render_values(state, track);
            for viewport in viewports {
                for &step in changed_steps {
                    let Some(slot) = visible_slot_for_step(viewport, step) else {
                        continue;
                    };
                    dirty |= sync_expanded_step_slot(
                        rt,
                        state,
                        app,
                        &selected,
                        current_track_idx,
                        viewport,
                        slot,
                        &render_values,
                    );
                }
            }
        }
    }
    dirty
}

pub(super) fn neural_neuron_selected_field(pattern_idx: usize, network_id: u64, neuron_idx: usize) -> String {
    format!("neural-neuron-selected-{pattern_idx}-{network_id}-{neuron_idx}")
}

// Mirrors step selection: row widgets bind to targeted fields so selection dirties only those rows.
pub(super) fn sync_selected_neural_neuron_bindings(
    rt: &mut Runtime,
    state: &Arc<SequencerState>,
    selection: &BTreeSet<sequencer::lisp_host::SelectedNeuralNeuron>,
) -> bool {
    let mut dirty = rt
        .set_reactive(
            "SEQ",
            "selected-neural-neurons",
            sequencer::lisp_host::selected_neural_neurons_to_value(selection),
        )
        .effects_dirty;
    let pattern_idx = state.current_scene_index();
    for network in state.current_neural_networks() {
        let neuron_count = network.num_neurons.min(sequencer::neural::NUM_NEURONS);
        for neuron_idx in 0..neuron_count {
            dirty |= rt
                .set_reactive(
                    "SEQ",
                    &neural_neuron_selected_field(pattern_idx, network.id, neuron_idx),
                    Value::Bool(
                        selection.contains(&sequencer::lisp_host::SelectedNeuralNeuron {
                            pattern_idx,
                            network_id: network.id,
                            neuron_idx,
                        }),
                    ),
                )
                .effects_dirty;
        }
    }
    dirty
}

pub(super) fn sync_track_plocks_for_neural_selection(
    rt: &mut Runtime,
    app: &app::App,
    state: &Arc<SequencerState>,
    track: usize,
    selected_steps: &Arc<Mutex<HashSet<usize>>>,
    selection: &BTreeSet<sequencer::lisp_host::SelectedNeuralNeuron>,
) -> bool {
    if selection.is_empty()
        && selected_plock_step(selected_steps).is_some_and(|step| {
            !track_step_has_plock(state, track, &app.graph.effect_descriptors, step)
        })
    {
        // The selected step has no locks, so the row table is empty — but the
        // variant strip is track-scoped, not step-scoped. Keep publishing it so
        // a lock-free step can still be stamped with an existing variant.
        let mut dirty = rt
            .set_reactive("SEQ", "track-plocks", Value::List(Vec::new()))
            .effects_dirty;
        dirty |= rt
            .set_reactive(
                "SEQ",
                "track-plock-variants",
                build_track_plock_variants_value(state, track, selected_steps),
            )
            .effects_dirty;
        return dirty;
    }
    let mut dirty = rt
        .set_reactive(
            "SEQ",
            "track-plocks",
            build_track_plocks_value_with_neural_selection(
                app,
                state,
                track,
                selected_steps,
                Some(selection),
            ),
        )
        .effects_dirty;
    dirty |= rt
        .set_reactive(
            "SEQ",
            "track-plock-variants",
            build_track_plock_variants_value(state, track, selected_steps),
        )
        .effects_dirty;
    dirty
}

pub(super) fn sync_track_plock_variant_preview(
    rt: &mut Runtime,
    app: &app::App,
    state: &Arc<SequencerState>,
    track: usize,
    selected_steps: &Arc<Mutex<HashSet<usize>>>,
    preview: Option<&(usize, String)>,
) -> bool {
    if !selected_steps.lock().unwrap().is_empty() {
        return false;
    }
    let Some((preview_track, label)) = preview else {
        return false;
    };
    if *preview_track != track {
        return false;
    }
    let mut dirty = false;
    dirty |= rt
        .set_reactive(
            "SEQ",
            "track-plocks",
            build_track_plocks_value_for_variant_label(app, state, track, label),
        )
        .effects_dirty;
    dirty |= rt
        .set_reactive(
            "SEQ",
            "track-plock-variants",
            build_track_plock_variants_value_with_preview(
                state,
                track,
                selected_steps,
                Some(label),
            ),
        )
        .effects_dirty;
    dirty
}

pub(super) fn sync_instrument_plock_presence_fields(
    rt: &mut Runtime,
    state: &Arc<SequencerState>,
    effect_descriptors: &[Vec<sequencer::effects::EffectDescriptor>],
    track: usize,
    selected_steps: &Arc<Mutex<HashSet<usize>>>,
) -> bool {
    let steps: Vec<usize> = selected_steps.lock().unwrap().iter().copied().collect();
    let mut dirty = false;
    dirty |= rt
        .set_reactive(
            "SEQ",
            "step-has-plocks",
            build_step_has_plocks(state, track, effect_descriptors),
        )
        .effects_dirty;
    let render_values = plock_variant_step_render_values(state, track);
    let step_plock_kinds = build_step_plock_kinds_from_render(&render_values);
    let step_variant_r = build_step_variant_color_channel_from_render(&render_values, 0);
    let step_variant_g = build_step_variant_color_channel_from_render(&render_values, 1);
    let step_variant_b = build_step_variant_color_channel_from_render(&render_values, 2);
    dirty |= rt
        .set_reactive("SEQ", "step-plock-kinds", step_plock_kinds.clone())
        .effects_dirty;
    dirty |= rt
        .set_reactive("SEQ", "step-variant-r", step_variant_r.clone())
        .effects_dirty;
    dirty |= rt
        .set_reactive("SEQ", "step-variant-g", step_variant_g.clone())
        .effects_dirty;
    dirty |= rt
        .set_reactive("SEQ", "step-variant-b", step_variant_b.clone())
        .effects_dirty;
    dirty |= rt
        .set_reactive_list_index("SEQ", "track-step-plock-kinds", track, step_plock_kinds)
        .effects_dirty;
    dirty |= rt
        .set_reactive_list_index("SEQ", "track-step-variant-r", track, step_variant_r)
        .effects_dirty;
    dirty |= rt
        .set_reactive_list_index("SEQ", "track-step-variant-g", track, step_variant_g)
        .effects_dirty;
    dirty |= rt
        .set_reactive_list_index("SEQ", "track-step-variant-b", track, step_variant_b)
        .effects_dirty;
    dirty |= rt
        .set_reactive(
            "SEQ",
            "track-plock-variants",
            build_track_plock_variants_value(state, track, selected_steps),
        )
        .effects_dirty;
    // Per-step compact-grid bindings for the touched steps. The compact step
    // shell binds its p-lock tick to the per-step `-plock-kind-` number and its
    // tint to the per-step `-variant-{r,g,b}-` fields, not to the list forms
    // published above, so those must be written here too — otherwise the tick
    // only appears on the next `ui_epoch`-driven full sync (i.e. after an
    // unrelated selection change).
    let num_steps = state.pattern.track_params[track]
        .get_num_steps()
        .min(MAX_STEPS);
    for step in steps {
        if step >= MAX_STEPS {
            continue;
        }
        // `visible` gating matches the full sync (out-of-range steps report no
        // p-lock); previously this write was ungated.
        let visible = step < num_steps;
        dirty |= rt
            .set_reactive(
                "SEQ",
                &track_step_plocked_field(track, step),
                Value::Bool(
                    visible && track_step_has_plock(state, track, effect_descriptors, step),
                ),
            )
            .effects_dirty;
        dirty |= sync_track_step_plock_render_fields(rt, track, step, render_values[step]);
    }
    dirty
}

pub(super) fn record_selected_neural_instrument_plock(
    editor: &mut Editor,
    state: &Arc<SequencerState>,
    selected_neural_neurons: &sequencer::lisp_host::SharedSelectedNeuralNeurons,
    track: usize,
    param_idx: usize,
    value: f32,
) -> (
    BTreeSet<sequencer::lisp_host::SelectedNeuralNeuron>,
    bool,
    Option<sequencer::sequencer::ProjectScenes>,
) {
    let neural_selection = selected_neural_neurons.lock().unwrap().clone();
    let history_before = (!neural_selection.is_empty())
        .then(|| state.capture_project_scenes());
    let wrote_neural_plock = write_selected_neural_instrument_plock(
        editor,
        state,
        &neural_selection,
        track,
        param_idx,
        value,
    );
    (
        neural_selection,
        wrote_neural_plock,
        history_before.filter(|_| wrote_neural_plock),
    )
}

pub(super) fn write_selected_neural_instrument_plock(
    editor: &mut Editor,
    state: &Arc<SequencerState>,
    neural_selection: &BTreeSet<sequencer::lisp_host::SelectedNeuralNeuron>,
    track: usize,
    param_idx: usize,
    value: f32,
) -> bool {
    sequencer::lisp_host::set_selected_neural_instrument_plocks(
        state,
        neural_selection,
        track,
        param_idx,
        value,
    )
    .unwrap_or_else(|error| {
        editor.handle_host_event(HostEvent::Status(format!(
            "Error setting neuron instrument p-lock: {error}"
        )));
        !neural_selection.is_empty()
    })
}

pub(super) fn record_selected_neural_effect_plock(
    editor: &mut Editor,
    state: &Arc<SequencerState>,
    selected_neural_neurons: &sequencer::lisp_host::SharedSelectedNeuralNeurons,
    track: usize,
    slot_idx: usize,
    param_idx: usize,
    value: f32,
) -> (
    BTreeSet<sequencer::lisp_host::SelectedNeuralNeuron>,
    bool,
    Option<sequencer::sequencer::ProjectScenes>,
) {
    let neural_selection = selected_neural_neurons.lock().unwrap().clone();
    let history_before = (!neural_selection.is_empty())
        .then(|| state.capture_project_scenes());
    let wrote_neural_plock = write_selected_neural_effect_plock(
        editor,
        state,
        &neural_selection,
        track,
        slot_idx,
        param_idx,
        value,
    );
    (
        neural_selection,
        wrote_neural_plock,
        history_before.filter(|_| wrote_neural_plock),
    )
}

pub(super) fn write_selected_neural_effect_plock(
    editor: &mut Editor,
    state: &Arc<SequencerState>,
    neural_selection: &BTreeSet<sequencer::lisp_host::SelectedNeuralNeuron>,
    track: usize,
    slot_idx: usize,
    param_idx: usize,
    value: f32,
) -> bool {
    sequencer::lisp_host::set_selected_neural_effect_plocks(
        state,
        neural_selection,
        track,
        slot_idx,
        param_idx,
        value,
    )
    .unwrap_or_else(|error| {
        editor.handle_host_event(HostEvent::Status(format!(
            "Error setting neuron effect p-lock: {error}"
        )));
        !neural_selection.is_empty()
    })
}

pub(super) struct InstrumentParamDisplaySync<'a> {
    pub(super) app: &'a app::App,
    pub(super) state: &'a Arc<SequencerState>,
    pub(super) selected_steps: &'a Arc<Mutex<HashSet<usize>>>,
    pub(super) selection: &'a BTreeSet<sequencer::lisp_host::SelectedNeuralNeuron>,
    pub(super) expanded_step_projection: &'a Arc<ExpandedStepProjectionRegistry>,
    pub(super) track: usize,
    /// The track the *fx* panel is showing. `track` may name another one, and
    /// the current-track-relative `fx-instrument-param-*` fields must only be
    /// published when the two match (as in `apply_ui_invalidations`).
    pub(super) current_track_idx: usize,
    pub(super) param_idx: usize,
    pub(super) display_step: Option<usize>,
    pub(super) sync_plock_list: bool,
    pub(super) sync_plock_presence: bool,
    pub(super) sync_sampler_times: bool,
}

/// Republish every p-lock *presence* surface an instrument p-lock write can
/// change: the compact grid values plus the expanded lanes' per-slot p-lock
/// ticks. The expanded ticks used to be refreshed by the reactive tick's
/// `ui_epoch`-driven full viewport resync, which the p-lock authoring path no
/// longer triggers (see `sync_expanded_step_plocked_fields_for_steps`).
pub(super) fn sync_instrument_plock_presence_display_fields(
    rt: &mut Runtime,
    state: &Arc<SequencerState>,
    app: &app::App,
    expanded_step_projection: &Arc<ExpandedStepProjectionRegistry>,
    track: usize,
    selected_steps: &Arc<Mutex<HashSet<usize>>>,
) -> bool {
    let mut dirty = sync_instrument_plock_presence_fields(
        rt,
        state,
        &app.graph.effect_descriptors,
        track,
        selected_steps,
    );
    let steps: Vec<usize> = selected_steps.lock().unwrap().iter().copied().collect();
    dirty |= sync_expanded_step_plocked_fields_for_steps(
        rt,
        state,
        app,
        expanded_step_projection,
        track,
        &steps,
    );
    dirty
}

pub(super) fn sync_instrument_param_authoring_display(
    editor: &mut Editor,
    sync: InstrumentParamDisplaySync<'_>,
) {
    let mut ui_dirty = false;
    if sync.sync_plock_list {
        ui_dirty |= sync_track_plocks_for_neural_selection(
            editor.runtime_mut(),
            sync.app,
            sync.state,
            sync.track,
            sync.selected_steps,
            sync.selection,
        );
    }
    if sync.sync_plock_presence {
        ui_dirty |= sync_instrument_plock_presence_display_fields(
            editor.runtime_mut(),
            sync.state,
            sync.app,
            sync.expanded_step_projection,
            sync.track,
            sync.selected_steps,
        );
    }
    ui_dirty |= if sync.track == sync.current_track_idx {
        sync_fx_instrument_param_value_field_with_neural_selection(
            editor.runtime_mut(),
            sync.app,
            sync.track,
            sync.param_idx,
            sync.display_step,
            Some(sync.selection),
        )
    } else {
        sync_instrument_param_value_field_with_neural_selection(
            editor.runtime_mut(),
            sync.app,
            sync.track,
            sync.param_idx,
            sync.display_step,
            Some(sync.selection),
        )
    };
    if sync.sync_sampler_times && (sync.param_idx == 2 || sync.param_idx == 3) {
        ui_dirty |= sync_sampler_selection_time_fields(
            editor.runtime_mut(),
            sync.app,
            sync.track,
            sync.display_step,
        );
    }
    flush_reactive_display_edit(editor, ui_dirty);
}

pub(super) struct EffectParamDisplaySync<'a> {
    pub(super) state: &'a Arc<SequencerState>,
    pub(super) effect_descriptors: &'a [Vec<sequencer::effects::EffectDescriptor>],
    pub(super) app: &'a app::App,
    pub(super) selected_steps: &'a Arc<Mutex<HashSet<usize>>>,
    pub(super) selection: &'a BTreeSet<sequencer::lisp_host::SelectedNeuralNeuron>,
    pub(super) track: usize,
    pub(super) slot_idx: usize,
    pub(super) param_idx: usize,
    pub(super) display_step: Option<usize>,
    pub(super) sync_plock_list: bool,
}

pub(super) fn sync_effect_param_authoring_display(editor: &mut Editor, sync: EffectParamDisplaySync<'_>) {
    let mut ui_dirty = false;
    if sync.sync_plock_list {
        ui_dirty |= sync_track_plocks_for_neural_selection(
            editor.runtime_mut(),
            sync.app,
            sync.state,
            sync.track,
            sync.selected_steps,
            sync.selection,
        );
    }
    ui_dirty |= sync_track_effect_param_value_field_with_neural_selection(
        editor.runtime_mut(),
        sync.app,
        sync.track,
        sync.slot_idx,
        sync.param_idx,
        sync.display_step,
        Some(sync.selection),
    );
    flush_reactive_display_edit(editor, ui_dirty);
}

pub(super) fn sync_instrument_param_batch_display(
    editor: &mut Editor,
    app: &app::App,
    state: &Arc<SequencerState>,
    selected_steps: &Arc<Mutex<HashSet<usize>>>,
    selection: &BTreeSet<sequencer::lisp_host::SelectedNeuralNeuron>,
    track: usize,
    current_track_idx: usize,
    param_indices: &[usize],
    display_step: Option<usize>,
    plocks_changed: bool,
) {
    let mut ui_dirty = false;
    if plocks_changed {
        ui_dirty |= sync_instrument_plock_presence_fields(
            editor.runtime_mut(),
            state,
            &app.graph.effect_descriptors,
            track,
            selected_steps,
        );
    }
    // A batch can name any track; only the current one owns the visible *fx*
    // panel's current-track-relative fields (cf. `apply_ui_invalidations`).
    let publish_fx_relative = track == current_track_idx;
    for &param_idx in param_indices {
        ui_dirty |= if publish_fx_relative {
            sync_fx_instrument_param_value_field_with_neural_selection(
                editor.runtime_mut(),
                app,
                track,
                param_idx,
                display_step,
                Some(selection),
            )
        } else {
            sync_instrument_param_value_field_with_neural_selection(
                editor.runtime_mut(),
                app,
                track,
                param_idx,
                display_step,
                Some(selection),
            )
        };
    }
    if param_indices.iter().any(|param_idx| *param_idx == 2 || *param_idx == 3) {
        ui_dirty |= sync_sampler_selection_time_fields(
            editor.runtime_mut(),
            app,
            track,
            display_step,
        );
    }
    flush_reactive_display_edit(editor, ui_dirty);
}

pub(super) fn sync_effect_param_batch_display(
    editor: &mut Editor,
    app: &app::App,
    selection: &BTreeSet<sequencer::lisp_host::SelectedNeuralNeuron>,
    track: usize,
    slot_idx: usize,
    param_indices: &[usize],
    display_step: Option<usize>,
) {
    let mut ui_dirty = false;
    for &param_idx in param_indices {
        ui_dirty |= sync_track_effect_param_value_field_with_neural_selection(
            editor.runtime_mut(),
            app,
            track,
            slot_idx,
            param_idx,
            display_step,
            Some(selection),
        );
    }
    flush_reactive_display_edit(editor, ui_dirty);
}

pub(super) fn flush_reactive_display_edit(editor: &mut Editor, dirty: bool) {
    if dirty {
        editor.runtime_mut().run_reactive_cycle();
        editor.refresh_runtime_side_effects();
        editor.mark_needs_redraw();
    }
}

pub(super) fn sync_expanded_step_viewports_for_track(
    rt: &mut Runtime,
    state: &Arc<SequencerState>,
    app: &app::App,
    selected_steps: &Arc<Mutex<HashSet<usize>>>,
    current_track_idx: usize,
    expanded_step_projection: &Arc<ExpandedStepProjectionRegistry>,
    track: usize,
) -> bool {
    let selected = selected_steps.lock().unwrap();
    let mut dirty = false;
    for viewport in expanded_step_projection.viewports_for_track(track) {
        dirty |=
            sync_expanded_step_viewport(rt, state, app, &selected, current_track_idx, viewport);
    }
    dirty
}

/// Repaint just the p-lock ticks of the expanded lanes that show `steps` on
/// `track`.
///
/// `seqv-slot-plocked-*` used to be refreshed by the reactive tick's
/// `sync_all_expanded_step_viewports`, which rode along with the `ui_epoch`
/// bump the instrument p-lock authoring path used to do on every drag update.
/// That bump is gone (it rebuilt the whole fx widget source per drag event),
/// so the authoring path writes the affected slots itself instead of paying
/// for a full viewport sync.
pub(super) fn sync_expanded_step_plocked_fields_for_steps(
    rt: &mut Runtime,
    state: &Arc<SequencerState>,
    app: &app::App,
    expanded_step_projection: &Arc<ExpandedStepProjectionRegistry>,
    track: usize,
    steps: &[usize],
) -> bool {
    if track >= app.tracks.len() {
        return false;
    }
    let viewports = expanded_step_projection.viewports_for_track(track);
    if viewports.is_empty() {
        return false;
    }
    let num_steps = state.pattern.track_params[track]
        .get_num_steps()
        .min(MAX_STEPS);
    let mut dirty = false;
    let render_values = plock_variant_step_render_values(state, track);
    for viewport in viewports {
        for &step in steps {
            let Some(slot) = visible_slot_for_step(viewport, step) else {
                continue;
            };
            let visible = step < num_steps;
            dirty |= rt
                .set_reactive(
                    "SEQ",
                    &expanded_step_slot_plocked_field(viewport.track_id, slot),
                    Value::Bool(
                        visible
                            && track_step_has_plock(
                                state,
                                track,
                                &app.graph.effect_descriptors,
                                step,
                            ),
                    ),
                )
                .effects_dirty;
            dirty |= sync_expanded_step_slot_plock_render_fields(
                rt,
                viewport,
                slot,
                visible,
                &render_values,
            );
        }
    }
    dirty
}

pub(super) fn sync_all_expanded_step_viewports(
    rt: &mut Runtime,
    state: &Arc<SequencerState>,
    app: &app::App,
    selected_steps: &Arc<Mutex<HashSet<usize>>>,
    current_track_idx: usize,
    expanded_step_projection: &Arc<ExpandedStepProjectionRegistry>,
) -> bool {
    let selected = selected_steps.lock().unwrap();
    let mut dirty = false;
    for viewport in expanded_step_projection.all_viewports() {
        dirty |=
            sync_expanded_step_viewport(rt, state, app, &selected, current_track_idx, viewport);
    }
    dirty
}

pub(super) fn sync_shared_track_collapsed(track_collapsed: &Arc<Mutex<Vec<bool>>>, app: &app::App) {
    *track_collapsed.lock().unwrap() = app.track_collapsed.clone();
}

pub(super) fn mod_route_destination_status_label(
    app: &app::App,
    destination: sequencer::sequencer::ModDestination,
) -> String {
    match destination {
        sequencer::sequencer::ModDestination::Track(track) => format!("track {}", track + 1),
        sequencer::sequencer::ModDestination::Bus(bus_id) => app
            .buses
            .iter()
            .find(|bus| bus.id == bus_id)
            .map(|bus| bus.name.clone())
            .unwrap_or_else(|| format!("bus {}", bus_id.0)),
    }
}

pub(super) struct UiInvalidationApplyCtx<'a> {
    pub(super) app: &'a mut app::App,
    pub(super) editor: &'a mut Editor,
    pub(super) state: &'a Arc<SequencerState>,
    pub(super) track_collapsed: &'a Arc<Mutex<Vec<bool>>>,
    pub(super) bus_state: &'a Arc<Mutex<Vec<app::BusChannelState>>>,
    pub(super) current_track_idx: usize,
    pub(super) selected_steps: &'a Arc<Mutex<HashSet<usize>>>,
    pub(super) selected_neural_neurons: &'a BTreeSet<sequencer::lisp_host::SelectedNeuralNeuron>,
    pub(super) piano_roll_selection: &'a Arc<Mutex<HashSet<u64>>>,
    pub(super) accumulator_names: &'a Arc<Mutex<Vec<String>>>,
    pub(super) cached_track_peak_levels: &'a [f64],
    pub(super) cached_bus_peak_levels: &'a [f64],
    pub(super) record_armed: &'a Arc<Mutex<Vec<bool>>>,
    pub(super) active_delete_target: &'a Arc<Mutex<Option<ActiveDeleteTarget>>>,
    pub(super) active_delete_target_version: &'a Arc<AtomicUsize>,
    pub(super) expanded_step_projection: &'a Arc<ExpandedStepProjectionRegistry>,
    pub(super) fx_visible: bool,
    pub(super) sequencer_visible: bool,
    pub(super) mixer_visible: bool,
}

pub(super) fn apply_ui_invalidations(
    invalidations: Vec<UiInvalidation>,
    ctx: UiInvalidationApplyCtx<'_>,
) -> bool {
    if invalidations.is_empty() {
        return false;
    }

    let UiInvalidationApplyCtx {
        app,
        editor,
        state,
        track_collapsed,
        bus_state,
        current_track_idx,
        selected_steps,
        selected_neural_neurons,
        piano_roll_selection,
        accumulator_names,
        cached_track_peak_levels,
        cached_bus_peak_levels,
        record_armed,
        active_delete_target,
        active_delete_target_version,
        expanded_step_projection,
        fx_visible,
        sequencer_visible,
        mixer_visible,
    } = ctx;

    let mut needs_reactive_cycle = false;
    let mut bus_state_pulled = false;
    // Step-param edits fan out to two surfaces that have no per-step writer:
    // the `SEQ.track-{velocities,durations,...}` list-of-lists (read by every
    // non-current track's expanded lane) and `SEQ.track-duration-spans`. Both
    // are per-TRACK rewrites, so a drag over a 64-step selection must not do
    // them once per step — collect the distinct (track, param) pairs here and
    // flush them once after the loop. `set-step-param-history` used to reach
    // them by bumping ui_epoch, which resynced every track instead.
    let mut step_param_track_lists: Vec<(usize, StepParam)> = Vec::new();
    let mut duration_span_tracks: Vec<usize> = Vec::new();
    // The compact step shell's p-lock tick/variant tint is derived from
    // `live_track_has_seq_lock`, which is true as soon as ANY StepParam leaves
    // its default — so a transpose/velocity/duration edit flips the step's
    // render kind. Computing the render vector needs a per-track registry
    // reconcile over all MAX_STEPS, so collect the touched steps here and do
    // one reconcile per track after the loop.
    let mut plock_render_steps: Vec<(usize, Vec<usize>)> = Vec::new();
    // The piano roll renders notes from transpose/velocity/duration, so a
    // step-param edit on the current track moves them. One sync per apply,
    // never one per step.
    let mut piano_roll_step_params_dirty = false;
    let active_track_count = state.active_track_count().min(app.tracks.len());
    let legacy_step_grid_visible = editor_has_visible_buffer(editor, "*metal*");
    let rt = editor.runtime_mut();

    for invalidation in invalidations {
        let track_domain = match &invalidation {
            UiInvalidation::CurrentTrack { current, .. } => Some(*current),
            UiInvalidation::TrackTopology(TrackTopologyInvalidation::InstrumentType { track }) => {
                Some(*track)
            }
            UiInvalidation::Pattern(PatternInvalidation::WholeTrack { track })
            | UiInvalidation::Pattern(PatternInvalidation::TrackLength { track })
            | UiInvalidation::Pattern(PatternInvalidation::TrackTiming { track })
            | UiInvalidation::Step { track, .. }
            | UiInvalidation::StepBatch { track, .. }
            | UiInvalidation::StepSelection { track, .. }
            | UiInvalidation::ExpandedStepViewport { track, .. }
            | UiInvalidation::TrackMixer { track, .. }
            | UiInvalidation::TrackBusSend { track, .. }
            | UiInvalidation::TrackRoute { track }
            | UiInvalidation::TrackParam { track, .. }
            | UiInvalidation::TrackParamPanel { track }
            | UiInvalidation::ProcessChain { track }
            | UiInvalidation::Instrument { track, .. }
            | UiInvalidation::TrackFx { track, .. }
            | UiInvalidation::MidiFx { track, .. }
            | UiInvalidation::PianoRoll { track, .. }
            | UiInvalidation::Sidebar { track, .. } => Some(*track),
            _ => None,
        };
        if track_domain.is_some_and(|track| track >= active_track_count) {
            continue;
        }
        if matches!(&invalidation, UiInvalidation::Step { step, .. } if *step >= MAX_STEPS) {
            continue;
        }
        let bus_domain = match &invalidation {
            UiInvalidation::BusMixer { bus, .. }
            | UiInvalidation::BusFx { bus, .. }
            | UiInvalidation::TrackBusSend { bus, .. } => Some(*bus),
            _ => None,
        };
        if bus_domain.is_some_and(|bus| bus >= app.buses.len()) {
            continue;
        }

        match invalidation {
            UiInvalidation::Full(_)
            | UiInvalidation::TrackTopology(_)
            | UiInvalidation::BusTopology
            | UiInvalidation::ProjectState
            | UiInvalidation::CurrentTrack { .. } => {
                needs_reactive_cycle = true;
            }
            UiInvalidation::Pattern(PatternInvalidation::WholeTrack { track }) => {
                if track == current_track_idx {
                    needs_reactive_cycle |= rt
                        .set_reactive("SEQ", "steps", build_steps_value(state, track))
                        .effects_dirty;
                    sync_step_param_lists(rt, state, track);
                }
                if sequencer_visible {
                    sync_all_track_sequencer_state(
                        rt,
                        state,
                        app,
                        current_track_idx,
                        selected_steps,
                    );
                    let _ = sync_expanded_step_viewports_for_track(
                        rt,
                        state,
                        app,
                        selected_steps,
                        current_track_idx,
                        expanded_step_projection,
                        track,
                    );
                    needs_reactive_cycle = true;
                }
            }
            UiInvalidation::Pattern(PatternInvalidation::TrackLength { track }) => {
                if track == current_track_idx {
                    needs_reactive_cycle |= rt
                        .set_reactive(
                            "SEQ",
                            "tp-num-steps",
                            Value::Number(state.pattern.track_params[track].get_num_steps() as f64),
                        )
                        .effects_dirty;
                }
                needs_reactive_cycle |= rt
                    .set_reactive_list_index(
                        "SEQ",
                        "track-num-steps",
                        track,
                        Value::Number(state.pattern.track_params[track].get_num_steps() as f64),
                    )
                    .effects_dirty;
                if sequencer_visible {
                    needs_reactive_cycle |= sync_expanded_step_viewports_for_track(
                        rt,
                        state,
                        app,
                        selected_steps,
                        current_track_idx,
                        expanded_step_projection,
                        track,
                    );
                }
            }
            UiInvalidation::Pattern(PatternInvalidation::AllTracks)
            | UiInvalidation::Pattern(PatternInvalidation::TrackTiming { .. }) => {
                sync_pattern_state(rt, state);
                if sequencer_visible {
                    let _ = sync_all_expanded_step_viewports(
                        rt,
                        state,
                        app,
                        selected_steps,
                        current_track_idx,
                        expanded_step_projection,
                    );
                }
                needs_reactive_cycle = true;
            }
            UiInvalidation::Step {
                track,
                step,
                change,
            } => match change {
                StepInvalidation::Param(param) => {
                    let param = param.to_step_param();
                    needs_reactive_cycle |= sync_single_step_param_binding(
                        rt,
                        state,
                        track,
                        step,
                        param,
                        current_track_idx,
                        selected_steps,
                        expanded_step_projection,
                    );
                    if !step_param_track_lists.contains(&(track, param)) {
                        step_param_track_lists.push((track, param));
                    }
                    push_deferred_track_step(&mut plock_render_steps, track, step);
                    if track == current_track_idx {
                        piano_roll_step_params_dirty = true;
                    }
                }
                StepInvalidation::DurationSpan => {
                    needs_reactive_cycle |=
                        sync_track_duration_span_binding_fields(rt, state, track, step);
                    if !duration_span_tracks.contains(&track) {
                        duration_span_tracks.push(track);
                    }
                }
                StepInvalidation::Active
                | StepInvalidation::Payload
                | StepInvalidation::PlockPresence
                | StepInvalidation::Selected => {
                    needs_reactive_cycle |= sync_single_step_structural_bindings(
                        rt,
                        state,
                        app,
                        track,
                        step,
                        current_track_idx,
                        selected_steps,
                        expanded_step_projection,
                    );
                }
            },
            UiInvalidation::StepBatch { track, steps } => {
                needs_reactive_cycle |= sync_step_batch_structural_bindings(
                    rt,
                    state,
                    app,
                    track,
                    &steps,
                    current_track_idx,
                    selected_steps,
                    expanded_step_projection,
                );
            }
            UiInvalidation::StepSelection {
                track,
                changed_steps,
            } => {
                needs_reactive_cycle |= sync_step_selection_bindings(
                    rt,
                    state,
                    Some(&*app),
                    track,
                    selected_steps,
                    current_track_idx,
                    expanded_step_projection,
                    &changed_steps,
                    legacy_step_grid_visible,
                );
                if track == current_track_idx {
                    needs_reactive_cycle |= sync_selected_track_bus_send_binding_fields(
                        rt,
                        app,
                        state,
                        track,
                        selected_steps,
                    );
                    if fx_visible {
                        let next_display = displayed_plock_step(
                            state,
                            track,
                            selected_plock_step(selected_steps),
                        );
                        // Read the single field directly: `global_value("SEQ")`
                        // clones the entire SEQ namespace map (thousands of
                        // per-track/per-step fields) just to look at one entry.
                        let previous_display = match rt
                            .reactive_field_value("SEQ", "fx-step-display-step")
                        {
                            Some(Value::Number(step)) if *step >= 0.0 => Some(*step as usize),
                            _ => None,
                        };
                        needs_reactive_cycle |= sync_track_plocks_for_neural_selection(
                            rt,
                            app,
                            state,
                            track,
                            selected_steps,
                            selected_neural_neurons,
                        );
                        let display_values_can_differ = !selected_neural_neurons.is_empty()
                            || previous_display.is_some_and(|step| {
                                track_step_has_plock(
                                    state,
                                    track,
                                    &app.graph.effect_descriptors,
                                    step,
                                )
                            })
                            || next_display.is_some_and(|step| {
                                track_step_has_plock(
                                    state,
                                    track,
                                    &app.graph.effect_descriptors,
                                    step,
                                )
                            });
                        if display_values_can_differ {
                            needs_reactive_cycle |= sync_track_selection_param_binding_fields(
                                rt,
                                state,
                                track,
                                selected_steps,
                            );
                            needs_reactive_cycle |=
                                sync_fx_param_binding_fields(rt, app, state, track, selected_steps);
                        }
                        needs_reactive_cycle |= rt
                            .set_reactive(
                                "SEQ",
                                "fx-step-display-step",
                                next_display
                                    .map(|step| Value::Number(step as f64))
                                    .unwrap_or(Value::Number(-1.0)),
                            )
                            .effects_dirty;
                    }
                }
            }
            UiInvalidation::ExpandedStepViewport { track: _, track_id } => {
                if let Some(viewport) = expanded_step_projection.viewport(track_id) {
                    let selected = selected_steps.lock().unwrap();
                    needs_reactive_cycle |= sync_expanded_step_viewport(
                        rt,
                        state,
                        app,
                        &selected,
                        current_track_idx,
                        viewport,
                    );
                }
            }
            UiInvalidation::TrackMixer { track, change } => match change {
                TrackMixerInvalidation::Volume => {
                    sync_track_volume_binding_field(rt, state, track);
                    needs_reactive_cycle |= rt
                        .set_reactive_list_index(
                            "SEQ",
                            "track-volumes",
                            track,
                            Value::Number(state.pattern.track_params[track].get_volume() as f64),
                        )
                        .effects_dirty;
                }
                TrackMixerInvalidation::Pan => {
                    sync_track_pan_binding_field(rt, state, track);
                    needs_reactive_cycle |= rt
                        .set_reactive_list_index(
                            "SEQ",
                            "track-mixer-pans",
                            track,
                            Value::Number(state.pattern.track_params[track].get_pan() as f64),
                        )
                        .effects_dirty;
                }
                TrackMixerInvalidation::Mute => {
                    needs_reactive_cycle |= rt
                        .set_reactive_list_index(
                            "SEQ",
                            "track-mutes",
                            track,
                            Value::Bool(state.pattern.track_params[track].is_muted()),
                        )
                        .effects_dirty;
                    needs_reactive_cycle |= sync_track_mute_visual_binding_fields(
                        rt,
                        app,
                        state,
                        std::iter::once(track),
                        false,
                    );
                }
                TrackMixerInvalidation::Solo => {
                    needs_reactive_cycle |= rt
                        .set_reactive_list_index(
                            "SEQ",
                            "track-solos",
                            track,
                            Value::Bool(state.pattern.track_params[track].is_solo()),
                        )
                        .effects_dirty;
                    needs_reactive_cycle |= sync_track_mute_visual_binding_fields(
                        rt,
                        app,
                        state,
                        0..active_track_count,
                        true,
                    );
                }
                TrackMixerInvalidation::RecordArm => {
                    needs_reactive_cycle |= rt
                        .set_reactive(
                            "SEQ",
                            "record-armed",
                            build_record_armed_value(&record_armed.lock().unwrap()),
                        )
                        .effects_dirty;
                }
                TrackMixerInvalidation::Output => {
                    needs_reactive_cycle |= rt
                        .set_reactive("SEQ", "track-outputs", build_track_outputs(app, state))
                        .effects_dirty;
                    // Routing decides whether a bus solo mutes this track.
                    needs_reactive_cycle |= sync_track_mute_visual_binding_fields(
                        rt,
                        app,
                        state,
                        std::iter::once(track),
                        true,
                    );
                }
                TrackMixerInvalidation::Collapsed => {
                    let collapsed = track_collapsed.lock().unwrap().clone();
                    if let Err(error) = app.apply_recorded_track_collapsed(collapsed) {
                        *track_collapsed.lock().unwrap() = app.track_collapsed.clone();
                        eprintln!("Could not change track collapse state: {error}");
                    }
                    needs_reactive_cycle |= rt
                        .set_reactive("SEQ", "track-collapsed", build_track_collapsed(app))
                        .effects_dirty;
                }
            },
            UiInvalidation::BusMixer { bus, change } => {
                if !bus_state_pulled {
                    pull_shared_bus_state(app, bus_state);
                    bus_state_pulled = true;
                }
                if app.buses.get(bus).is_some() {
                    match change {
                        BusMixerInvalidation::Volume => {
                            sync_bus_mixer_control_state(rt, app);
                            needs_reactive_cycle = true;
                        }
                        BusMixerInvalidation::Mute => {
                            sync_bus_mixer_control_state(rt, app);
                            needs_reactive_cycle = true;
                        }
                        BusMixerInvalidation::Solo => {
                            sync_bus_mixer_control_state(rt, app);
                            // Bus solos mute tracks outside the soloed group,
                            // so the per-track dim state follows them too.
                            sync_track_mute_visual_binding_fields(
                                rt,
                                app,
                                state,
                                0..active_track_count,
                                true,
                            );
                            needs_reactive_cycle = true;
                        }
                        BusMixerInvalidation::Steps | BusMixerInvalidation::Timing => {
                            sync_bus_mixer_state(rt, app);
                            needs_reactive_cycle = true;
                        }
                    }
                }
            }
            UiInvalidation::TrackBusSend { track, bus } => {
                sync_track_bus_send_binding_field(rt, app, state, track, bus);
                if track == current_track_idx {
                    sync_current_track_bus_send_binding_field(rt, app, state, track, bus);
                }
                needs_reactive_cycle |= rt
                    .set_reactive(
                        "SEQ",
                        "track-bus-sends",
                        build_all_track_bus_sends(app, state),
                    )
                    .effects_dirty;
            }
            UiInvalidation::TrackRoute { .. } => {
                sync_track_mixer_state(rt, app, state);
                needs_reactive_cycle = true;
            }
            UiInvalidation::ModRoutes => {
                needs_reactive_cycle |= rt
                    .set_reactive("SEQ", "mod-routes", build_mod_routes(state))
                    .effects_dirty;
            }
            UiInvalidation::TrackParam { track, change } => {
                if change == TrackParamInvalidation::NumSteps {
                    needs_reactive_cycle |= rt
                        .set_reactive_list_index(
                            "SEQ",
                            "track-num-steps",
                            track,
                            Value::Number(state.pattern.track_params[track].get_num_steps() as f64),
                        )
                        .effects_dirty;
                }
                if track == current_track_idx {
                    sync_track_params_with_neural_selection(
                        rt,
                        app,
                        state,
                        track,
                        selected_steps,
                        Some(selected_neural_neurons),
                    );
                    needs_reactive_cycle = true;
                }
            }
            UiInvalidation::TrackParamPanel { track } => {
                if track == current_track_idx {
                    sync_track_params_with_neural_selection(
                        rt,
                        app,
                        state,
                        track,
                        selected_steps,
                        Some(selected_neural_neurons),
                    );
                    needs_reactive_cycle = true;
                }
            }
            UiInvalidation::ProcessChain { track } => {
                sync_process_chain_state(rt, state, app.tracks.len(), current_track_idx);
                if sequencer_visible {
                    let _ = sync_all_expanded_step_viewports(
                        rt,
                        state,
                        app,
                        selected_steps,
                        current_track_idx,
                        expanded_step_projection,
                    );
                }
                if track == current_track_idx {
                    needs_reactive_cycle = true;
                }
            }
            UiInvalidation::Instrument { track, change } => {
                let display_step =
                    displayed_plock_step(state, track, selected_plock_step(selected_steps));
                match change {
                    InstrumentInvalidation::Param { param } => {
                        needs_reactive_cycle |= if track == current_track_idx {
                            sync_fx_instrument_param_value_field(
                                rt, app, track, param, display_step,
                            )
                        } else {
                            sync_instrument_param_value_field(rt, app, track, param, display_step)
                        };
                    }
                    InstrumentInvalidation::Plock { param } => {
                        needs_reactive_cycle |= if track == current_track_idx {
                            sync_fx_instrument_param_value_field(
                                rt, app, track, param, display_step,
                            )
                        } else {
                            sync_instrument_param_value_field(rt, app, track, param, display_step)
                        };
                        if track == current_track_idx {
                            needs_reactive_cycle |= sync_instrument_plock_presence_fields(
                                rt,
                                state,
                                &app.graph.effect_descriptors,
                                track,
                                selected_steps,
                            );
                        }
                    }
                    InstrumentInvalidation::BaseNote => {
                        needs_reactive_cycle |= if track == current_track_idx {
                            sync_fx_instrument_base_note_value_field(rt, app, track)
                        } else {
                            sync_instrument_base_note_value_field(rt, app, track)
                        };
                    }
                    InstrumentInvalidation::SamplerSelectionTime => {
                        needs_reactive_cycle |=
                            sync_sampler_selection_time_fields(rt, app, track, display_step);
                    }
                    InstrumentInvalidation::PanelTopology | InstrumentInvalidation::Analysis => {
                        if fx_visible && track == current_track_idx {
                            rt.set_reactive(
                                "SEQ",
                                "instrument-panel",
                                build_instrument_panel_value(app, track, selected_steps),
                            );
                            needs_reactive_cycle = true;
                        }
                    }
                    InstrumentInvalidation::Playhead => {
                        if app.is_sampler_track(track) {
                            let ph = read_sampler_playhead_seconds(app, track);
                            if ph > 0.0 {
                                needs_reactive_cycle |= rt
                                    .set_reactive("SEQ", "sampler-playhead", Value::Number(ph))
                                    .effects_dirty;
                            }
                        }
                    }
                }
            }
            UiInvalidation::TrackFx { track, change } => match change {
                TrackFxInvalidation::Param { slot, param }
                | TrackFxInvalidation::Plock { slot, param } => {
                    let display_step =
                        displayed_plock_step(state, track, selected_plock_step(selected_steps));
                    needs_reactive_cycle |= sync_track_effect_param_value_field(
                        rt,
                        app,
                        track,
                        slot,
                        param,
                        display_step,
                    );
                    if track == current_track_idx {
                        needs_reactive_cycle |= rt
                            .set_reactive(
                                "SEQ",
                                "step-has-plocks",
                                build_step_has_plocks(state, track, &app.graph.effect_descriptors),
                            )
                            .effects_dirty;
                    }
                }
                TrackFxInvalidation::Topology | TrackFxInvalidation::PanelTree => {
                    if fx_visible && track == current_track_idx {
                        rt.set_reactive(
                            "SEQ",
                            "effects",
                            build_effects_value(
                                state,
                                track,
                                &app.graph.effect_descriptors,
                                selected_steps,
                            ),
                        );
                        needs_reactive_cycle = true;
                    }
                }
            },
            UiInvalidation::MidiFx { track, change } => match change {
                MidiFxInvalidation::Param { slot, param } => {
                    let display_step =
                        displayed_plock_step(state, track, selected_plock_step(selected_steps));
                    needs_reactive_cycle |=
                        sync_midi_fx_param_value_field(rt, state, track, slot, param, display_step);
                }
                MidiFxInvalidation::Topology => {
                    if fx_visible && track == current_track_idx {
                        rt.set_reactive(
                            "SEQ",
                            "midi-effects",
                            build_midi_effects_value(state, track, selected_steps),
                        );
                        needs_reactive_cycle = true;
                    }
                }
            },
            UiInvalidation::BusFx { bus, change } => match change {
                BusFxInvalidation::Param { slot, param } => {
                    sync_bus_effect_param_value_field(rt, app, bus, slot, param);
                }
                BusFxInvalidation::Topology => {
                    if mixer_visible || fx_visible {
                        sync_bus_mixer_state(rt, app);
                        needs_reactive_cycle = true;
                    }
                }
                BusFxInvalidation::PanelTree => {
                    if fx_visible {
                        rt.set_reactive(
                            "SEQ",
                            "bus-effects",
                            build_bus_effects_value_for_selection(app, Some(selected_steps)),
                        );
                        needs_reactive_cycle = true;
                    }
                }
            },
            UiInvalidation::PianoRoll { track, change } => {
                if track == current_track_idx {
                    sync_piano_roll_state(rt, app, state, track, piano_roll_selection);
                    needs_reactive_cycle = true;
                }
                if matches!(change, PianoRollInvalidation::Items) {
                    needs_reactive_cycle |= sync_single_track_sequencer_state(
                        rt,
                        state,
                        app,
                        track,
                        current_track_idx,
                        selected_steps,
                        expanded_step_projection,
                    );
                }
            }
            UiInvalidation::Transport(_) => {
                needs_reactive_cycle = true;
            }
            UiInvalidation::Recording(change) => match change {
                RecordingInvalidation::RecordingEnabled => {
                    needs_reactive_cycle |= rt
                        .set_reactive(
                            "SEQ",
                            "recording",
                            Value::Bool(
                                app.ui.recording
                                    || record_armed.lock().unwrap().iter().any(|armed| *armed),
                            ),
                        )
                        .effects_dirty;
                }
                RecordingInvalidation::ArmedTracks => {
                    needs_reactive_cycle |= rt
                        .set_reactive(
                            "SEQ",
                            "record-armed",
                            build_record_armed_value(&record_armed.lock().unwrap()),
                        )
                        .effects_dirty;
                }
            },
            UiInvalidation::DeleteTarget => {
                needs_reactive_cycle |= rt
                    .set_reactive(
                        "SEQ",
                        "delete-target-version",
                        Value::Number(active_delete_target_version.load(Ordering::Relaxed) as f64),
                    )
                    .effects_dirty;
                sync_mixer_delete_target_binding_fields(
                    rt,
                    app.tracks.len(),
                    &state,
                    active_delete_target.lock().unwrap().as_ref(),
                );
            }
            UiInvalidation::AutoFollow => {
                needs_reactive_cycle = true;
            }
            UiInvalidation::Sidebar { track, .. } => {
                sync_sidebar_browser(rt, app, track);
                needs_reactive_cycle = true;
            }
            UiInvalidation::Browser(_) => {
                needs_reactive_cycle = true;
            }
        }
    }

    // Deferred per-track fan-out of the step-param invalidations (see the
    // declarations above): one write per distinct (track, param), not one per
    // invalidated step.
    for (track, param) in step_param_track_lists {
        needs_reactive_cycle |=
            sync_track_step_param_list_binding_for_param(rt, state, track, param);
    }
    for track in duration_span_tracks {
        needs_reactive_cycle |= sync_track_duration_spans_list_binding(rt, state, track);
    }
    // One registry reconcile per track, then per-step writes only: the p-lock
    // tick has no other writer on a step-param edit now that the funnel does
    // not bump ui_epoch.
    for (track, steps) in plock_render_steps {
        let render_values = plock_variant_step_render_values(state, track);
        for step in steps {
            if let Some(render) = render_values.get(step).copied() {
                needs_reactive_cycle |=
                    sync_track_step_plock_render_fields(rt, track, step, render);
            }
        }
    }
    if piano_roll_step_params_dirty {
        sync_piano_roll_state(rt, app, state, current_track_idx, piano_roll_selection);
        needs_reactive_cycle = true;
    }

    if needs_reactive_cycle {
        *accumulator_names.lock().unwrap() = build_accumulator_names(app);
        sync_track_peak_fields(rt, cached_track_peak_levels);
        sync_bus_peak_fields(rt, cached_bus_peak_levels);
    }
    needs_reactive_cycle
}

pub(super) fn reset_sampler_waveform_view(editor: &mut Editor) {
    if let Err(error) = editor.runtime_mut().eval_str("(eseq.effects.sampler-panel/sampler-reset-view)") {
        eprintln!("waveform: failed to reset sampler viewport: {error:?}");
    }
}

pub(super) struct SamplerTrackLoadResult {
    pub(super) name: String,
    pub(super) reset_summary: Option<InstrumentSlotResetSummary>,
}

pub(super) fn load_or_convert_sampler_track(
    app: &mut app::App,
    editor: &mut Editor,
    state: &Arc<SequencerState>,
    current_track: &Arc<AtomicUsize>,
    track_names: &mut Vec<String>,
    selected_steps: &Arc<Mutex<HashSet<usize>>>,
    lg_raw: *mut sequencer::audiograph::LiveGraph,
    track: usize,
    path: Option<&Path>,
    preserve_track_selection: bool,
) -> Result<SamplerTrackLoadResult, String> {
    if track >= app.tracks.len() {
        return Err(format!("Track {} does not exist", track + 1));
    }
    let instrument_type = app.graph.track_instrument_types[track];
    if !matches!(
        instrument_type,
        InstrumentType::Sampler | InstrumentType::Custom | InstrumentType::Rack
    ) {
        return Err(
            "Samples can only replace sampler, custom instrument, or rack tracks".to_string(),
        );
    }
    if instrument_type == InstrumentType::Sampler && path.is_none() {
        return Ok(SamplerTrackLoadResult {
            name: app.tracks[track].clone(),
            reset_summary: None,
        });
    }

    let resolved_path = path
        .map(Path::to_path_buf)
        .or_else(|| app.sampler_path_for_track(track));
    let (new_buffer_id, sample_rate, new_name) = if let Some(path) = resolved_path.as_deref() {
        let loaded = sequencer::instruments::sampler::load_wav_buffer(lg_raw, path)?;
        app.submit_sample_analysis(&loaded);
        let name = sequencer::sample_db::display_title_for_sample_path(path)
            .unwrap_or(loaded.name.clone());
        register_waveform_sample(path);
        (loaded.buffer_id, loaded.sample_rate, name)
    } else {
        (
            sequencer::instruments::sampler::create_silent_buffer(lg_raw)?,
            app.graph.sample_rate,
            format!("Sampler {}", track + 1),
        )
    };

    let history_path = resolved_path.clone();
    let reset_summary = app.apply_recorded_instrument_binding_mutation(
        track,
        "Replace instrument",
        |app| {
            let reset_summary = match instrument_type {
                InstrumentType::Sampler => {
                    app.graph_controller()
                        .send_sample_to_all_voices(track, new_buffer_id, sample_rate);
                    app.graph.track_buffer_ids[track] = new_buffer_id;
                    app.graph.track_sample_rates[track] = sample_rate;
                    app.tracks[track] = new_name.clone();
                    app.state.seed_unset_pattern_sample_ids(
                        track,
                        (new_buffer_id, new_name.clone(), sample_rate),
                    );
                    None
                }
                InstrumentType::Custom => Some(
                    app.graph_controller().convert_custom_track_to_sampler(
                        track,
                        new_buffer_id,
                        sample_rate,
                        &new_name,
                    )?,
                ),
                InstrumentType::Rack => {
                    let summary = app.graph_controller().replace_rack_track_with_sampler(
                        track,
                        new_buffer_id,
                        sample_rate,
                        &new_name,
                    )?;
                    Some(summary)
                }
                other => {
                    return Err(format!(
                        "Track {} has instrument type {other:?}, which cannot load a sample",
                        track + 1
                    ));
                }
            };
            if let Some(path) = history_path.as_ref() {
                app.register_loaded_sample_path(&new_name, new_buffer_id, path.clone());
                if track < app.sampler_paths.len() {
                    app.sampler_paths[track] = Some(path.clone());
                }
            }
            app.reset_sampler_bpm_for_analysis(track);
            app.publish_sampler_analysis_runtime(track);
            Ok(reset_summary)
        },
    )?;
    reset_sampler_waveform_view(editor);
    if let Some(track_name) = track_names.get_mut(track) {
        *track_name = new_name.clone();
    }
    let selected_track = host_commands::selection_after_track_apply(
        track,
        preserve_track_selection,
        current_track,
        app.tracks.len(),
    );
    current_track.store(selected_track, Ordering::Relaxed);
    app.ui.cursor_track = selected_track;

    let rt = editor.runtime_mut();
    set_current_track_reactive(rt, app.tracks.len(), selected_track);
    rt.set_reactive("SEQ", "track-names", build_track_names(track_names));
    rt.set_reactive(
        "SEQ",
        "instrument-panel",
        build_instrument_panel_value(app, selected_track, selected_steps),
    );
    if selected_track == track {
        sync_sampler_selection_time_fields(
            rt,
            app,
            selected_track,
            selected_steps.lock().unwrap().iter().copied().min(),
        );
    }
    sync_track_mixer_state(rt, app, state);
    sync_sidebar_browser(rt, app, selected_track);
    rt.run_reactive_cycle();
    editor.refresh_runtime_side_effects();
    Ok(SamplerTrackLoadResult {
        name: new_name,
        reset_summary,
    })
}
