use super::*;

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
    let items = sequencer::effects::builtin_effect_names()
        .into_iter()
        .map(|name| Rc::new(RefCell::new(Value::String(name.to_string()))))
        .collect();
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

pub(super) const METER_FLOOR_DBFS: f32 = -60.0;

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

pub(crate) fn sync_project_state(rt: &mut Runtime, app: &app::App) {
    rt.set_reactive(
        "SEQ",
        "current-project-name",
        Value::String(app.current_project_name.clone().unwrap_or_default()),
    );
    rt.set_reactive("SEQ", "sound-presets", build_sound_presets_value());
    rt.set_reactive("SEQ", "kit-presets", build_kit_presets_value());
}

/// Builds `SEQ.kit-presets`: the drum-rack kits the browser's Kits tab lists
/// (docs/drum-rack-v2-spec.md, "Polish"). One entry per `.kit` file, with the
/// pad count so a kit reads as a kit and not as another Sound.
pub(crate) fn build_kit_presets_value() -> Value {
    let kits = sequencer::project::list_kit_presets().unwrap_or_default();
    list_value(kits.into_iter().filter_map(|path| {
        let kit = sequencer::project::load_kit_preset(&path).ok()?;
        let label = if kit.metadata.name.trim().is_empty() {
            path.file_stem()?.to_str()?.to_string()
        } else {
            kit.metadata.name
        };
        Some(map_value([
            ("kind", Value::String("kit".to_string())),
            ("label", Value::String(label.clone())),
            ("name", Value::String(label)),
            ("path", Value::String(path.to_string_lossy().to_string())),
            ("pads", Value::Number(kit.pads.len() as f64)),
            ("author", Value::String(kit.metadata.author)),
            (
                "tags",
                list_value(kit.metadata.tags.into_iter().map(Value::String)),
            ),
        ]))
    }))
}

pub(crate) fn build_sound_presets_value() -> Value {
    let sounds = sequencer::project::list_sound_presets().unwrap_or_default();
    list_value(sounds.into_iter().filter_map(|path| {
        let preset = sequencer::project::load_sound_preset(&path).ok()?;
        let label = if preset.metadata.name.trim().is_empty() {
            path.file_stem()?.to_str()?.to_string()
        } else {
            preset.metadata.name
        };
        Some(map_value([
            ("kind", Value::String("sound".to_string())),
            ("label", Value::String(label.clone())),
            ("name", Value::String(label)),
            ("path", Value::String(path.to_string_lossy().to_string())),
            ("author", Value::String(preset.metadata.author)),
            (
                "tags",
                list_value(preset.metadata.tags.into_iter().map(Value::String)),
            ),
        ]))
    }))
}

pub(crate) const PROJECT_SCRATCH_BUFFER_NAME: &str = "*scratch*";

pub(super) fn project_scratch_source_path() -> PathBuf {
    sequencer::paths::project_scratch_source_path()
}

pub(crate) fn clear_project_script_tabs(editor: &mut Editor) -> Result<(), String> {
    editor
        .runtime_mut()
        .eval_str("(eseq.seq-step-tabs/seq-clear-project-script-tabs)")
        .map_err(|error| format!("Failed to clear project script tabs: {error:?}"))?;
    editor.refresh_runtime_side_effects();
    Ok(())
}

pub(super) fn project_script_load_path(line: &str) -> Option<String> {
    let tokens = Parser::new(line.to_string()).parse().ok()?;
    let expressions = ASTParser::new(tokens).parse().ok()?;
    let [Expression::List(items)] = expressions.as_slice() else {
        return None;
    };
    match items.as_slice() {
        [Expression::Symbol(load), Expression::String(path)] if load == "load" => {
            Some(path.clone())
        }
        _ => None,
    }
}

pub(super) fn canonical_project_script_path(path: &str) -> PathBuf {
    let path = Path::new(path);
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        sequencer::app_paths::app_paths()
            .project_script_root()
            .join(path)
    };
    std::fs::canonicalize(&absolute).unwrap_or(absolute)
}

pub(crate) fn remove_project_script_from_scratch(editor: &mut Editor, source_path: &str) -> bool {
    let target = canonical_project_script_path(source_path);
    let Some(buffer) = editor
        .buffers
        .iter_mut()
        .find(|buffer| buffer.name == PROJECT_SCRATCH_BUFFER_NAME)
    else {
        return false;
    };

    let mut removed = false;
    let mut previous_blank = true;
    let mut kept = Vec::with_capacity(buffer.lines.len());
    for line in &buffer.lines {
        let matches_target = project_script_load_path(line)
            .is_some_and(|path| canonical_project_script_path(&path) == target);
        if matches_target {
            removed = true;
            continue;
        }
        let blank = line.trim().is_empty();
        if blank && previous_blank {
            continue;
        }
        kept.push(line.clone());
        previous_blank = blank;
    }
    while kept.last().is_some_and(|line| line.trim().is_empty()) {
        kept.pop();
    }
    if removed {
        buffer.set_text(&kept.join("\n"));
        editor.mark_needs_redraw();
    }
    removed
}

pub(crate) fn push_project_scratch_to_named_buffer(editor: &mut Editor, app: &app::App) {
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
    app: &app::App,
) -> Result<(), String> {
    let scratch_text = app.state.scratch_source();
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
        let failure = report.failure_message();
        eprintln!(
            "metal_seq: project scratch UI eval failed path={} error={failure}",
            project_scratch_source_path().display()
        );
        Err(failure)
    };
    editor.process_lisp_reload_report(report);
    if let Some(status) = editor.runtime_mut().take_status_message() {
        editor.show_transient_message(status);
    }
    result
}

pub(crate) fn pull_named_scratch_buffer_into_project(editor: &Editor, app: &mut app::App) {
    let Some(buffer) = editor
        .buffers
        .iter()
        .find(|buffer| buffer.name == PROJECT_SCRATCH_BUFFER_NAME)
    else {
        return;
    };

    let text = buffer.text();
    let cursor = buffer.cursor;
    if app.editor.scratch_buffer != text || app.editor.scratch_cursor != cursor {
        // Keep the draft current for project persistence, but do not publish it to
        // the scheduler. Editing *scratch* must have no execution side effects.
        app.editor.scratch_buffer = text;
        app.editor.scratch_cursor = cursor;
    }
}

pub(crate) fn publish_evaluated_project_scratch(
    editor: &Editor,
    app: &mut app::App,
    transaction_id: u64,
    success: bool,
) -> bool {
    if !success {
        return false;
    }
    let Some(buffer) = editor.buffers.iter().find(|buffer| {
        buffer.id as u64 == transaction_id && buffer.name == PROJECT_SCRATCH_BUFFER_NAME
    }) else {
        return false;
    };

    app.state.set_scratch_source(buffer.text());
    app.editor.scratch_runtime = None;
    true
}

pub(crate) fn current_custom_instrument_name(app: &app::App, track: usize) -> Option<String> {
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

pub(crate) fn sync_sidebar_browser(rt: &mut Runtime, app: &app::App, track: usize) {
    rt.set_reactive(
        "SEQ",
        "project-instrument-engines",
        build_string_list(&project_instrument_engine_names(app)),
    );
    if app.graph.track_instrument_types.get(track)
        == Some(&sequencer::sequencer::InstrumentType::Sampler)
    {
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

    let is_rack = app.graph.track_instrument_types.get(track)
        == Some(&sequencer::sequencer::InstrumentType::Rack);
    let instrument_name = if is_rack {
        app.tracks.get(track).cloned().unwrap_or_default()
    } else {
        current_custom_instrument_name(app, track).unwrap_or_default()
    };
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
    app: &mut app::App,
    track: usize,
    preset_name: &str,
) -> Result<(), String> {
    let instrument_name = current_custom_instrument_name(app, track)
        .ok_or_else(|| "Current track is not a custom instrument".to_string())?;
    let presets = sequencer::lisp_host::load_instrument_presets_shared(&instrument_name)
        .map_err(|e| e.to_string())?;
    let preset = presets
        .iter()
        .find(|preset| preset.name == preset_name)
        .cloned()
        .ok_or_else(|| format!("Preset '{preset_name}' not found"))?;
    let desc = app
        .graph
        .instrument_descriptors
        .get(track)
        .cloned()
        .ok_or_else(|| "Instrument descriptor unavailable".to_string())?;

    let engine_id = app.graph.track_engine_ids.get(track).and_then(|id| *id);
    let preset_label = preset.name.clone();
    sequencer::app::edit::apply_recorded_instrument_values_mutation(
        app,
        track,
        format!("Load preset '{preset_label}'"),
        move |app| {
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
            sequencer::effects::restore_key_locks_by_param_name(
                slot,
                &desc,
                &preset.key_locks,
            );
            app.state.pattern.instrument_base_note_offsets[track]
                .store(preset.base_note_offset.to_bits(), Ordering::Relaxed);
            app.state.schedule_mod_resync();
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
        },
    )
    .map(|_| ())
    .map_err(|error| format!("{error:?}"))
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

pub(crate) fn extract_i32_from_payload(payload: &Value, key: &str) -> Option<i32> {
    if let Value::Map(map) = payload {
        if let Some(cell) = map.get(key) {
            if let Value::Number(n) = &*cell.borrow() {
                return Some(*n as i32);
            }
        }
    }
    None
}

pub(crate) fn extract_f32_from_payload(payload: &Value, key: &str) -> Option<f32> {
    if let Value::Map(map) = payload {
        if let Some(cell) = map.get(key) {
            if let Value::Number(n) = &*cell.borrow() {
                return Some(*n as f32);
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
fn sync_track_param_fields(
    rt: &mut Runtime,
    app: &app::App,
    state: &Arc<SequencerState>,
    track: usize,
    selected: &Arc<Mutex<HashSet<usize>>>,
) {
    let tp = &state.pattern.track_params[track];
    rt.set_reactive("SEQ", "tp-attack", Value::Number(tp.get_attack_ms() as f64));
    rt.set_reactive(
        "SEQ",
        "tp-release",
        Value::Number(tp.get_release_ms() as f64),
    );
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
    // For a Rack track, playback polyphony is governed per-slot
    // (RackSlotSnapshot::max_polyphony, read by fire_rack_slot_note /
    // fire_live_keyboard_rack_note) — the track-level TrackParams poly/voices
    // fields below are never consulted for Sampler/Custom rack slots. Surface
    // the *selected slot's* values here (and which slot they'd be writing to)
    // so this panel's poly/voices controls can be routed to the right place
    // instead of silently editing a value playback ignores.
    let rack_slot_poly = (app.graph.track_instrument_types.get(track)
        == Some(&sequencer::sequencer::InstrumentType::Rack))
    .then(|| {
        let rack = app
            .state
            .pattern
            .rack_tracks
            .lock()
            .unwrap()
            .get(track)
            .cloned()
            .flatten()?;
        let selected_slot = app.selected_rack_slot_index_for_rack(track, &rack)?;
        let max_polyphony = rack.slots.get(selected_slot)?.max_polyphony;
        Some((selected_slot, max_polyphony))
    })
    .flatten();
    rt.set_reactive("SEQ", "tp-is-rack", Value::Bool(rack_slot_poly.is_some()));
    rt.set_reactive(
        "SEQ",
        "tp-rack-slot-idx",
        Value::Number(rack_slot_poly.map(|(slot_idx, _)| slot_idx).unwrap_or(0) as f64),
    );
    let (tp_poly, max_polyphony) = match rack_slot_poly {
        Some((_, max_polyphony)) => (max_polyphony > 1, max_polyphony),
        // Non-rack tracks: `is_polyphonic` is its own independently-toggled
        // flag, distinct from the voice-count value — don't derive it from
        // max_polyphony or the toggle button's state gets stomped every
        // render.
        None => (tp.is_polyphonic(), tp.get_max_polyphony()),
    };
    rt.set_reactive("SEQ", "tp-poly", Value::Bool(tp_poly));
    rt.set_reactive(
        "SEQ",
        "tp-max-polyphony",
        Value::Number(max_polyphony as f64),
    );
    let _ = sync_track_selection_param_binding_fields(rt, state, track, selected);
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
        "tp-mute-group",
        Value::String(mute_group_label(tp.get_mute_group())),
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
    rt.set_reactive("SEQ", "mute-group-options", build_mute_group_options());
    rt.set_reactive("SEQ", "accum-mode-options", build_accum_mode_options());
}

pub(crate) fn sync_track_params(
    rt: &mut Runtime,
    app: &app::App,
    state: &Arc<SequencerState>,
    track: usize,
    selected: &Arc<Mutex<HashSet<usize>>>,
) {
    sync_track_param_fields(rt, app, state, track, selected);
    rt.set_reactive(
        "SEQ",
        "track-plocks",
        build_track_plocks_value(app, state, track, selected),
    );
    rt.set_reactive(
        "SEQ",
        "track-plock-variants",
        build_track_plock_variants_value(state, track, selected),
    );
}

/// Refreshes only track-parameter fields whose displayed value follows the
/// selected step's p-lock. Selection changes should use this instead of
/// rebuilding every track parameter and option list.
pub(crate) fn sync_track_selection_param_binding_fields(
    rt: &mut Runtime,
    state: &Arc<SequencerState>,
    track: usize,
    selected: &Arc<Mutex<HashSet<usize>>>,
) -> bool {
    let tp = &state.pattern.track_params[track];
    let selected_step = selected_plock_step(selected);
    let display_step = displayed_plock_step(state, track, selected_step);
    let swing = display_step
        .and_then(|step| state.pattern.swing_plocks[track].get(step))
        .unwrap_or_else(|| tp.get_swing());
    let timebase = display_step
        .and_then(|step| state.pattern.timebase_plocks[track].get(step))
        .unwrap_or_else(|| tp.get_timebase());
    let swing_resolution = display_step
        .and_then(|step| state.pattern.swing_resolution_plocks[track].get(step))
        .unwrap_or_else(|| tp.get_swing_resolution());

    let mut dirty = rt
        .set_reactive("SEQ", "tp-swing", Value::Number(swing as f64))
        .effects_dirty;
    dirty |= rt
        .set_reactive(
            "SEQ",
            "tp-timebase",
            Value::String(timebase.label().to_string()),
        )
        .effects_dirty;
    dirty |= rt
        .set_reactive(
            "SEQ",
            "tp-swing-resolution",
            Value::String(swing_resolution.label().to_string()),
        )
        .effects_dirty;
    dirty
}

pub(crate) fn sync_track_params_with_neural_selection(
    rt: &mut Runtime,
    app: &app::App,
    state: &Arc<SequencerState>,
    track: usize,
    selected: &Arc<Mutex<HashSet<usize>>>,
    selected_neural_neurons: Option<
        &std::collections::BTreeSet<sequencer::lisp_host::SelectedNeuralNeuron>,
    >,
) {
    sync_track_param_fields(rt, app, state, track, selected);
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
    rt.set_reactive(
        "SEQ",
        "track-plock-variants",
        build_track_plock_variants_value(state, track, selected),
    );
}
