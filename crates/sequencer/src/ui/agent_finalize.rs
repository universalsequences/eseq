use super::*;

pub(super) const AGENT_INSTRUMENT_STUB_DSP: &str = r#"; Provisional silent instrument used while Agent Mode is designing the real patch.
(def gate (in 1 @name gate))
(def pitch (in 2 @name pitch))
(def velocity (in 3 @name velocity))
(def trigger (in 4 @name trigger))

(param enabled @default 1 @min 0 @max 1)

(out 0 1 @name audio)
"#;

pub(super) const AGENT_INSTRUMENT_STUB_UI: &str = r#"(defsynth-ui
  (box :width 70 :height :fill :padding 0 :debug-name "agent-instrument-stub-skeleton"
    (agent-instrument-stub-bg :width 70 :height :fill)))
"#;

pub(super) fn ensure_agent_instrument_stub_track(
    app: &mut app::App,
    editor: &mut Editor,
    state: &Arc<SequencerState>,
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
    conv_id: sequencer::agent::store::ConvId,
) -> Result<usize, String> {
    let snapshot = app
        .agent_store
        .snapshot(conv_id)
        .ok_or_else(|| format!("Agent conversation {conv_id} not found"))?;
    if let Some(target) = snapshot.state.accepted_instrument_target {
        return Ok(target.track_index);
    }
    if let Some(target) = snapshot.state.stub_instrument_target {
        sequencer::lisp_host::save_instrument(&target.instrument_name, AGENT_INSTRUMENT_STUB_DSP)
            .map_err(|error| format!("Failed to refresh agent stub dsp.lisp: {error}"))?;
        sequencer::lisp_host::save_instrument_ui(&target.instrument_name, AGENT_INSTRUMENT_STUB_UI)
            .map_err(|error| format!("Failed to refresh agent stub ui.lisp: {error}"))?;
        if target.track_index < app.tracks.len()
            && app.graph.track_instrument_types.get(target.track_index)
                == Some(&sequencer::sequencer::InstrumentType::Custom)
        {
            app.replace_custom_instrument_track_sync(
                target.track_index,
                &target.instrument_name,
                AGENT_INSTRUMENT_STUB_DSP,
            )
            .map_err(|error| format!("Failed to refresh agent stub track: {error}"))?;
            reload_custom_instrument_ui(editor);
            sync_after_instrument_track_apply(
                app,
                editor,
                state,
                target.track_index,
                current_track,
                track_names,
                track_pan_ids,
                record_armed,
                selected_steps,
                accumulator_names,
                cached_track_peak_levels,
                cached_bus_peak_levels,
                ui_epoch,
                lg_raw,
            );
            return Ok(target.track_index);
        }
    }

    let inst_name = format!("agent-draft-{conv_id}/");
    sequencer::lisp_host::save_instrument(&inst_name, AGENT_INSTRUMENT_STUB_DSP)
        .map_err(|error| format!("Failed to save agent stub dsp.lisp: {error}"))?;
    sequencer::lisp_host::save_instrument_ui(&inst_name, AGENT_INSTRUMENT_STUB_UI)
        .map_err(|error| format!("Failed to save agent stub ui.lisp: {error}"))?;

    let idx = app
        .add_saved_instrument_track_sync(&inst_name)
        .map_err(|error| format!("Failed to create agent stub track: {error}"))?;
    let _ = app.force_instrument_enabled(idx);
    reload_custom_instrument_ui(editor);

    app.agent_store
        .set_stub_instrument_target(
            conv_id,
            sequencer::agent::store::AcceptedInstrumentTarget {
                track_index: idx,
                instrument_name: inst_name,
            },
        )
        .map_err(|error| format!("Failed to record agent stub target: {error}"))?;
    app.agent_store
        .push_system_message(
            conv_id,
            format!("Created working instrument track {}", idx + 1),
        )
        .map_err(|error| format!("Failed to record agent stub message: {error}"))?;

    sync_after_instrument_track_apply(
        app,
        editor,
        state,
        idx,
        current_track,
        track_names,
        track_pan_ids,
        record_armed,
        selected_steps,
        accumulator_names,
        cached_track_peak_levels,
        cached_bus_peak_levels,
        ui_epoch,
        lg_raw,
    );
    Ok(idx)
}

pub(super) fn apply_agent_draft_to_owned_instrument(
    app: &mut app::App,
    editor: &mut Editor,
    state: &Arc<SequencerState>,
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
    conv_id: sequencer::agent::store::ConvId,
) -> Result<AgentDraftApplyResult, String> {
    let snapshot = app
        .agent_store
        .snapshot(conv_id)
        .ok_or_else(|| format!("Agent conversation {conv_id} not found"))?;
    let draft = snapshot
        .state
        .draft
        .ok_or_else(|| format!("Agent conversation {conv_id} has no compiled draft"))?;

    let target = snapshot
        .state
        .accepted_instrument_target
        .or(snapshot.state.stub_instrument_target);
    let inst_name = target
        .as_ref()
        .map(|target| target.instrument_name.clone())
        .unwrap_or_else(|| format!("agent-draft-{conv_id}/"));

    sequencer::lisp_host::save_instrument(&inst_name, &draft.dsp_source)
        .map_err(|error| format!("Failed to save agent draft dsp.lisp: {error}"))?;
    sequencer::lisp_host::save_instrument_ui(&inst_name, &draft.ui_source)
        .map_err(|error| format!("Failed to save agent draft ui.lisp: {error}"))?;

    let (idx, created_track) = if let Some(target) = target {
        if target.track_index < app.tracks.len()
            && app.graph.track_instrument_types.get(target.track_index)
                == Some(&sequencer::sequencer::InstrumentType::Custom)
        {
            app.replace_custom_instrument_track_sync(
                target.track_index,
                &inst_name,
                &draft.dsp_source,
            )
            .map_err(|error| format!("Failed to update agent instrument: {error}"))?;
            (target.track_index, false)
        } else {
            let idx = app
                .add_saved_instrument_track_sync(&inst_name)
                .map_err(|error| format!("Failed to recreate agent instrument track: {error}"))?;
            (idx, true)
        }
    } else {
        let idx = app
            .add_saved_instrument_track_sync(&inst_name)
            .map_err(|error| format!("Failed to accept agent draft: {error}"))?;
        (idx, true)
    };
    if app.force_instrument_enabled(idx) {
        eprintln!(
            "[agent-ui] forced instrument enabled conv={conv_id} track={}",
            idx + 1
        );
    }
    reload_custom_instrument_ui(editor);
    editor.refresh_visible_layouts_for_buffer_named("*fx*");
    let track_name = app.tracks[idx].clone();

    if let Err(error) = app.agent_store.discard(conv_id) {
        eprintln!("[agent-ui] accepted conv={conv_id} but failed to discard draft: {error}");
    }
    if let Err(error) = app.agent_store.set_accepted_instrument_target(
        conv_id,
        sequencer::agent::store::AcceptedInstrumentTarget {
            track_index: idx,
            instrument_name: inst_name,
        },
    ) {
        eprintln!("[agent-ui] accepted conv={conv_id} but failed to record target: {error}");
    }
    if let Err(error) = app.agent_store.push_system_message(
        conv_id,
        if created_track {
            format!("Created instrument track {}: {}", idx + 1, track_name)
        } else {
            format!("Updated instrument track {}: {}", idx + 1, track_name)
        },
    ) {
        eprintln!("[agent-ui] accepted conv={conv_id} but failed to record success: {error}");
    }

    sync_after_instrument_track_apply(
        app,
        editor,
        state,
        idx,
        current_track,
        track_names,
        track_pan_ids,
        record_armed,
        selected_steps,
        accumulator_names,
        cached_track_peak_levels,
        cached_bus_peak_levels,
        ui_epoch,
        lg_raw,
    );

    Ok(AgentDraftApplyResult {
        track_index: idx,
        created_track,
    })
}

pub(super) fn finalized_instrument_storage_paths(slug: &str) -> (PathBuf, PathBuf) {
    let root = sequencer::app_paths::app_paths().user_instruments_dir();
    (root.join(slug), root.join(format!("{slug}.lisp")))
}

pub(super) fn patcher_layout_sidecar_path_for_dsp(dsp_path: &Path) -> PathBuf {
    if dsp_path.file_name().and_then(|name| name.to_str()) == Some("dsp.lisp") {
        dsp_path.with_file_name("dsp.layout.json")
    } else {
        let stem = dsp_path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .unwrap_or("dsp");
        dsp_path.with_file_name(format!("{stem}.layout.json"))
    }
}

pub(super) fn copy_patcher_layout_sidecar(source_dsp: &Path, target_dsp: &Path) -> std::io::Result<()> {
    let source_layout = patcher_layout_sidecar_path_for_dsp(source_dsp);
    if !source_layout.exists() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            format!("missing layout sidecar '{}'", source_layout.display()),
        ));
    }
    let target_layout = patcher_layout_sidecar_path_for_dsp(target_dsp);
    if let Some(parent) = target_layout.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::copy(source_layout, target_layout).map(|_| ())
}

pub(super) fn write_patcher_layout_sidecar(dsp_path: &Path, layout: &str) -> std::io::Result<()> {
    let layout_path = patcher_layout_sidecar_path_for_dsp(dsp_path);
    if let Some(parent) = layout_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp_path = layout_path.with_file_name(format!(
        ".{}.tmp",
        layout_path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("dsp.layout.json")
    ));
    std::fs::write(&tmp_path, layout)?;
    std::fs::rename(&tmp_path, &layout_path).or_else(|error| {
        let _ = std::fs::remove_file(&tmp_path);
        Err(error)
    })
}

pub(super) fn apply_compiled_effect_edit_session(
    app: &mut app::App,
    session: &EffectEditSession,
    name: &str,
    result: sequencer::lisp_host::CompileResult,
) -> Result<(), String> {
    match session.target {
        EffectEditTarget::Track { track, slot } => {
            app.apply_compiled_effect_to_slot_recorded(result, name, slot, track)
        }
        EffectEditTarget::Bus { bus, slot } => {
            app.apply_compiled_bus_effect_to_slot_recorded(bus, slot, name, result)
        }
    }
}

pub(super) fn finalized_effect_storage_paths(slug: &str) -> (PathBuf, PathBuf) {
    let root = sequencer::app_paths::app_paths().user_effects_dir();
    (root.join(slug), root.join(format!("{slug}.lisp")))
}

pub(super) fn display_instrument_name(name: &str) -> String {
    let trimmed = name.trim_end_matches('/');
    Path::new(trimmed)
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or(trimmed)
        .trim_end_matches(".lisp")
        .to_string()
}

pub(super) fn cleanup_agent_draft_storage(name: &str) {
    let slug = name.trim_end_matches('/');
    if !slug.starts_with("agent-draft-") {
        return;
    }
    let root = sequencer::app_paths::app_paths().user_instruments_dir();
    let dir = root.join(slug);
    if dir.is_dir() {
        if let Err(error) = std::fs::remove_dir_all(&dir) {
            eprintln!(
                "[agent-ui] finalized {name:?} but failed to remove draft directory {}: {error}",
                dir.display()
            );
        }
    }
    let legacy_file = root.join(format!("{slug}.lisp"));
    if legacy_file.exists() {
        if let Err(error) = std::fs::remove_file(&legacy_file) {
            eprintln!(
                "[agent-ui] finalized {name:?} but failed to remove legacy draft file {}: {error}",
                legacy_file.display()
            );
        }
    }
}

pub(super) fn cleanup_agent_effect_draft_storage(name: &str) {
    let slug = name.trim_end_matches('/');
    if !slug.starts_with("agent-effect-draft-") {
        return;
    }
    let root = sequencer::app_paths::app_paths().user_effects_dir();
    let dir = root.join(slug);
    if dir.is_dir() {
        if let Err(error) = std::fs::remove_dir_all(&dir) {
            eprintln!(
                "[agent-ui] finalized {name:?} but failed to remove draft effect directory {}: {error}",
                dir.display()
            );
        }
    }
    let legacy_file = root.join(format!("{slug}.lisp"));
    if legacy_file.exists() {
        if let Err(error) = std::fs::remove_file(&legacy_file) {
            eprintln!(
                "[agent-ui] finalized {name:?} but failed to remove legacy draft effect file {}: {error}",
                legacy_file.display()
            );
        }
    }
}

pub(super) fn save_effect_with_ui_rollback(
    name: &str,
    dsp_source: &str,
    ui_source: &str,
) -> Result<(), String> {
    let previous_source = sequencer::lisp_host::load_effect_source(name).ok();
    let previous_ui = sequencer::lisp_host::load_effect_ui_source(name).ok();
    sequencer::lisp_host::save_effect(name, dsp_source)
        .map_err(|error| format!("Failed to save effect dsp.lisp: {error}"))?;
    if let Err(error) = sequencer::lisp_host::save_effect_ui(name, ui_source) {
        restore_effect_files(name, previous_source.as_deref(), previous_ui.as_deref());
        return Err(format!("Failed to save effect ui.lisp: {error}"));
    }
    Ok(())
}

pub(super) fn restore_effect_files(name: &str, source: Option<&str>, ui_source: Option<&str>) {
    match source {
        Some(source) => {
            let _ = sequencer::lisp_host::save_effect(name, source);
        }
        None => {
            let _ = std::fs::remove_file(sequencer::lisp_host::effect_source_path(name));
        }
    }
    match ui_source {
        Some(ui_source) => {
            let _ = sequencer::lisp_host::save_effect_ui(name, ui_source);
        }
        None => {
            let _ = std::fs::remove_file(sequencer::lisp_host::effect_ui_path(name));
        }
    }
}

pub(super) fn apply_agent_draft_to_effect_slot(
    app: &mut app::App,
    editor: &mut Editor,
    state: &Arc<SequencerState>,
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
    conv_id: sequencer::agent::store::ConvId,
) -> Result<AgentEffectApplyResult, String> {
    if app.tracks.is_empty() {
        return Err("No current track is available for the effect artifact.".to_string());
    }
    let snapshot = app
        .agent_store
        .snapshot(conv_id)
        .ok_or_else(|| format!("Agent conversation {conv_id} not found"))?;
    let draft = snapshot
        .state
        .effect_draft
        .ok_or_else(|| format!("Agent conversation {conv_id} has no validated effect draft"))?;

    let existing_target = snapshot.state.accepted_effect_target;
    let track_index = existing_target
        .as_ref()
        .map(|target| target.track_index)
        .unwrap_or(app.ui.cursor_track);
    if track_index >= app.tracks.len() {
        return Err("The target track for this effect artifact no longer exists.".to_string());
    }
    let slot_index = match existing_target.as_ref() {
        Some(target) => target.slot_index,
        None => app
            .next_free_custom_slot()
            .ok_or_else(|| "The current track has no free custom effect slot.".to_string())?,
    };
    let effect_name = existing_target
        .as_ref()
        .map(|target| target.effect_name.clone())
        .unwrap_or_else(|| format!("agent-effect-draft-{conv_id}/"));

    let previous_source = sequencer::lisp_host::load_effect_source(&effect_name).ok();
    let previous_ui = sequencer::lisp_host::load_effect_ui_source(&effect_name).ok();
    save_effect_with_ui_rollback(&effect_name, &draft.dsp_source, &draft.ui_source)?;
    if let Err(error) = app.load_saved_effect_to_slot_recorded(
        track_index,
        slot_index,
        &effect_name,
    ) {
        restore_effect_files(
            &effect_name,
            previous_source.as_deref(),
            previous_ui.as_deref(),
        );
        return Err(format!("Failed to apply agent effect artifact: {error}"));
    }
    reload_custom_instrument_ui(editor);
    editor.refresh_visible_layouts_for_buffer_named("*fx*");

    app.agent_store
        .set_accepted_effect_target(
            conv_id,
            sequencer::agent::store::AcceptedEffectTarget {
                track_index,
                slot_index,
                effect_name: effect_name.clone(),
            },
        )
        .map_err(|error| format!("Failed to record effect target: {error}"))?;
    app.agent_store
        .push_system_message(
            conv_id,
            format!(
                "Applied effect artifact to track {} slot {}",
                track_index + 1,
                slot_index + 1
            ),
        )
        .map_err(|error| format!("Failed to record effect apply message: {error}"))?;

    sync_after_instrument_track_apply(
        app,
        editor,
        state,
        track_index,
        current_track,
        track_names,
        track_pan_ids,
        record_armed,
        selected_steps,
        accumulator_names,
        cached_track_peak_levels,
        cached_bus_peak_levels,
        ui_epoch,
        lg_raw,
    );

    Ok(AgentEffectApplyResult {
        track_index,
        slot_index,
    })
}

pub(super) fn finalize_agent_instrument(
    app: &mut app::App,
    editor: &mut Editor,
    state: &Arc<SequencerState>,
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
    conv_id: sequencer::agent::store::ConvId,
    requested_name: &str,
) -> Result<AgentFinalizeResult, String> {
    let final_slug = sequencer::agent::actions::normalize_patch_name(
        requested_name,
        &format!("agent-instrument-{conv_id}"),
    );
    let final_name = format!("{final_slug}/");
    let (final_dir, legacy_file) = finalized_instrument_storage_paths(&final_slug);
    if final_dir.exists() || legacy_file.exists() {
        return Err(format!("Instrument '{final_slug}' already exists."));
    }

    let snapshot = app
        .agent_store
        .snapshot(conv_id)
        .ok_or_else(|| format!("Agent conversation {conv_id} not found"))?;
    let target = snapshot
        .state
        .accepted_instrument_target
        .ok_or_else(|| "No applied agent artifact is available to finalize.".to_string())?;
    if target.track_index >= app.tracks.len()
        || app.graph.track_instrument_types.get(target.track_index)
            != Some(&sequencer::sequencer::InstrumentType::Custom)
    {
        return Err(
            "The applied agent artifact is no longer attached to a custom instrument track."
                .to_string(),
        );
    }

    let (dsp_source, ui_source) = if let Some(draft) = snapshot.state.draft {
        (draft.dsp_source, draft.ui_source)
    } else {
        (
            sequencer::lisp_host::load_instrument_source(&target.instrument_name)
                .map_err(|error| format!("Failed to read draft dsp.lisp: {error}"))?,
            sequencer::lisp_host::load_instrument_ui_source(&target.instrument_name)
                .map_err(|error| format!("Failed to read draft ui.lisp: {error}"))?,
        )
    };

    sequencer::lisp_host::save_instrument(&final_name, &dsp_source)
        .map_err(|error| format!("Failed to save finalized dsp.lisp: {error}"))?;
    if let Err(error) = sequencer::lisp_host::save_instrument_ui(&final_name, &ui_source) {
        let _ = std::fs::remove_dir_all(&final_dir);
        return Err(format!("Failed to save finalized ui.lisp: {error}"));
    }

    if let Err(error) =
        app.replace_custom_instrument_track_sync(target.track_index, &final_name, &dsp_source)
    {
        let _ = std::fs::remove_dir_all(&final_dir);
        return Err(format!("Failed to load finalized instrument: {error}"));
    }
    reload_custom_instrument_ui(editor);
    editor.refresh_visible_layouts_for_buffer_named("*fx*");

    app.agent_store
        .set_accepted_instrument_target(
            conv_id,
            sequencer::agent::store::AcceptedInstrumentTarget {
                track_index: target.track_index,
                instrument_name: final_name.clone(),
            },
        )
        .map_err(|error| format!("Failed to update artifact target: {error}"))?;
    app.agent_store
        .set_finalized_instrument_name(conv_id, final_name.clone())
        .map_err(|error| format!("Failed to mark artifact finalized: {error}"))?;
    app.agent_store
        .push_system_message(
            conv_id,
            format!("Saved artifact as {}", display_instrument_name(&final_name)),
        )
        .map_err(|error| format!("Failed to record finalize message: {error}"))?;

    sync_after_instrument_track_apply(
        app,
        editor,
        state,
        target.track_index,
        current_track,
        track_names,
        track_pan_ids,
        record_armed,
        selected_steps,
        accumulator_names,
        cached_track_peak_levels,
        cached_bus_peak_levels,
        ui_epoch,
        lg_raw,
    );
    cleanup_agent_draft_storage(&target.instrument_name);

    Ok(AgentFinalizeResult {
        track_index: target.track_index,
        instrument_name: final_name,
    })
}

pub(super) fn finalize_agent_effect(
    app: &mut app::App,
    editor: &mut Editor,
    state: &Arc<SequencerState>,
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
    conv_id: sequencer::agent::store::ConvId,
    requested_name: &str,
) -> Result<AgentEffectFinalizeResult, String> {
    let final_slug = sequencer::agent::actions::normalize_patch_name(
        requested_name,
        &format!("agent-effect-{conv_id}"),
    );
    let final_name = format!("{final_slug}/");
    let (final_dir, legacy_file) = finalized_effect_storage_paths(&final_slug);
    if final_dir.exists() || legacy_file.exists() {
        return Err(format!("Effect '{final_slug}' already exists."));
    }

    let snapshot = app
        .agent_store
        .snapshot(conv_id)
        .ok_or_else(|| format!("Agent conversation {conv_id} not found"))?;
    let target = snapshot.state.accepted_effect_target;
    let (dsp_source, ui_source) = if let Some(target) = target.as_ref() {
        (
            sequencer::lisp_host::load_effect_source(&target.effect_name)
                .map_err(|error| format!("Failed to read draft effect dsp.lisp: {error}"))?,
            sequencer::lisp_host::load_effect_ui_source(&target.effect_name)
                .map_err(|error| format!("Failed to read draft effect ui.lisp: {error}"))?,
        )
    } else {
        let draft = snapshot
            .state
            .effect_draft
            .ok_or_else(|| "No effect artifact is available to finalize.".to_string())?;
        (draft.dsp_source, draft.ui_source)
    };

    save_effect_with_ui_rollback(&final_name, &dsp_source, &ui_source)?;

    if let Some(target) = target.as_ref() {
        if target.track_index >= app.tracks.len() {
            let _ = std::fs::remove_dir_all(&final_dir);
            return Err("The applied effect artifact target track no longer exists.".to_string());
        }
        if let Err(error) =
            app.load_saved_effect_to_slot_recorded(
                target.track_index,
                target.slot_index,
                &final_name,
            )
        {
            let _ = std::fs::remove_dir_all(&final_dir);
            return Err(format!("Failed to load finalized effect: {error}"));
        }
        reload_custom_instrument_ui(editor);
        editor.refresh_visible_layouts_for_buffer_named("*fx*");
        app.agent_store
            .set_accepted_effect_target(
                conv_id,
                sequencer::agent::store::AcceptedEffectTarget {
                    track_index: target.track_index,
                    slot_index: target.slot_index,
                    effect_name: final_name.clone(),
                },
            )
            .map_err(|error| format!("Failed to update effect artifact target: {error}"))?;
        sync_after_instrument_track_apply(
            app,
            editor,
            state,
            target.track_index,
            current_track,
            track_names,
            track_pan_ids,
            record_armed,
            selected_steps,
            accumulator_names,
            cached_track_peak_levels,
            cached_bus_peak_levels,
            ui_epoch,
            lg_raw,
        );
        cleanup_agent_effect_draft_storage(&target.effect_name);
    }

    app.agent_store
        .set_finalized_effect_name(conv_id, final_name.clone())
        .map_err(|error| format!("Failed to mark effect finalized: {error}"))?;
    app.agent_store
        .push_system_message(
            conv_id,
            format!(
                "Saved effect artifact as {}",
                display_instrument_name(&final_name)
            ),
        )
        .map_err(|error| format!("Failed to record effect finalize message: {error}"))?;

    Ok(AgentEffectFinalizeResult {
        track_index: target.as_ref().map(|target| target.track_index),
        slot_index: target.as_ref().map(|target| target.slot_index),
        effect_name: final_name,
    })
}

pub(super) fn agent_generation_watermark(app: &app::App) -> u64 {
    app.agent_store
        .list()
        .into_iter()
        .filter_map(|id| app.agent_store.snapshot(id).map(|snapshot| snapshot.state))
        .fold(0u64, |acc, state| {
            acc.wrapping_add(state.id)
                .wrapping_add(state.generation.wrapping_mul(31))
        })
}
