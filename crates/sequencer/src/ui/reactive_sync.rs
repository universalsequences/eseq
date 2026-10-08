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
    accumulator_names: &Arc<Mutex<Vec<String>>>,
    ui_epoch: &Arc<AtomicUsize>,
    lg_raw: *mut sequencer::audiograph::LiveGraph,
) {
    sync_after_instrument_track_apply_with_selection(
        app,
        editor,
        state,
        track_index,
        current_track,
        track_names,
        track_pan_ids,
        record_armed,
        accumulator_names,
        ui_epoch,
        lg_raw,
        false,
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
    accumulator_names: &Arc<Mutex<Vec<String>>>,
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
    crate::param_words::set_track_word_names(track_names);

    let rt = editor.runtime_mut();
    *accumulator_names.lock().unwrap() = build_accumulator_names(app);
    sync_sidebar_browser(app, selected_track);
    rt.run_reactive_cycle();
    editor.refresh_runtime_side_effects();
    refresh_visible_track_topology_layouts(editor);
    ui_epoch.fetch_add(1, Ordering::Relaxed);
}

pub(super) fn refresh_visible_track_topology_layouts(editor: &mut Editor) {
    // Registered sequencer views (custom step tabs) relayout alongside the
    // factory buffers so a package view stays current after topology edits.
    for buffer_name in super::edit_sessions::registered_sequencer_view_buffers(editor) {
        editor.refresh_visible_layouts_for_buffer_named(&buffer_name);
    }
    for buffer_name in [
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

/// After a device panel edit the panels read through the host kinds: bump
/// `ui_epoch`, so the kinds' model sync carries it this tick.
pub(super) fn refresh_instrument_panel_reactive(ui_epoch: &AtomicUsize) {
    ui_epoch.fetch_add(1, Ordering::Relaxed);
}

pub(super) fn apply_rack_macro_rename_host_command(
    app: &mut app::App,
    map: &HashMap<String, Rc<RefCell<Value>>>,
    ui_epoch: &AtomicUsize,
) {
    let (Some(track), Some(id), Some(name)) = (
        map_usize(map, "track"), map_usize(map, "id"),
        map.get("name").and_then(|value| match &*value.borrow() {
            Value::String(name) => Some(name.clone()),
            _ => None,
        }),
    ) else { return; };
    let Some(id) = sequencer::sequencer::RackMacroId::from_index(id) else { return; };
    let name = sequencer::sequencer::RackMacroField::Name(name);
    apply_rack_macro_edit_reactive(app, track, id, name, ui_epoch);
}

/// The rack panel's rack macro edit (`set-rack-macro-value`,
/// `rename-rack-macro`, `set-rack-macro-range`, `set-rack-macro-curve`):
/// `field` of rack macro `id` of `track` through history
/// (`App::apply_rack_macro_edit`: a drag joins one entry), then its
/// refresh; returns whether the model changed.
pub(super) fn apply_rack_macro_edit_reactive(
    app: &mut app::App,
    track: usize,
    id: sequencer::sequencer::RackMacroId,
    field: sequencer::sequencer::RackMacroField,
    ui_epoch: &AtomicUsize,
) -> bool {
    let changed = rack_macro_edit_reactive(app, field, ui_epoch, |app, field| {
            let outcome = app.apply_rack_macro_edit(track, id, field);
            outcome
                .map(|outcome| outcome.changed())
                .map_err(|error| format!("{error:?}"))
        });
    changed.unwrap_or(false)
}

/// A rack macro edit, recorded by
/// `record` (which returns whether the model changed: the rack panel's
/// plain `App::apply_rack_macro_edit`, or a script's `ScriptEdit`), then
/// its refresh (`rack_macro_edit_applied`) when it did. Shared by the rack
/// panel's commands and the host kinds' setters.
pub(super) fn rack_macro_edit_reactive(
    app: &mut app::App,
    field: sequencer::sequencer::RackMacroField,
    ui_epoch: &AtomicUsize,
    record: impl FnOnce(&mut app::App, sequencer::sequencer::RackMacroField) -> Result<bool, String>,
) -> Result<bool, String> {
    let changed = record(app, field.clone())?;
    if changed {
        rack_macro_edit_applied(&field, ui_epoch);
    }
    Ok(changed)
}

/// Refresh what an edit of a rack macro's `field` changed: a mapping's
/// range or curve bumps `ui_epoch` (the mapping rows are model fields); a
/// name or value nothing (the panels bind the rack macro, which the host
/// kinds push).
fn rack_macro_edit_applied(
    field: &sequencer::sequencer::RackMacroField,
    ui_epoch: &AtomicUsize,
) {
    use sequencer::sequencer::RackMacroField;
    match field {
        // The rack panel binds the rack macro's name and value (the host
        // kinds push them).
        RackMacroField::Name(_) | RackMacroField::Value(_) => {}
        RackMacroField::Range { .. } | RackMacroField::Curve { .. } => {
            refresh_instrument_panel_reactive(ui_epoch);
        }
    }
}

/// After a rack macro p-lock edit (the rack panel's `set-rack-macro-plock`,
/// the host kinds' `lock-rack-macro!` / `unlock-rack-macro!`). Same policy
/// as `refresh_rack_direct_param_reactive`: the macro knob and its mapped
/// targets bind their instances (the host kinds push their values), so only
/// a write that moves the shown step's p-lock rows (`RowSetChanged`: a
/// first lock there, or a clear) runs the epoch-driven resync (the host
/// kinds rebuild the step panel's rows, `selection.plock-rows`, from the
/// lock's invalidation). A macro is always continuous, so it never bumps
/// `fx_epoch`.
pub(super) fn refresh_rack_macro_plock_reactive(
    ui_epoch: &AtomicUsize,
    plock_rows: RackPlockRowsSync,
) {
    if plock_rows == RackPlockRowsSync::RowSetChanged {
        ui_epoch.fetch_add(1, Ordering::Relaxed);
    }
}

/// What a rack direct-param edit owes the *step* p-lock surfaces.
///
/// Mirrors the `set-instrument-plock` policy (instrument_params.rs): a
/// knob drag repaints through the param it binds (the host kinds push its
/// value), so only the FIRST write of a lock in a gesture — the one event that can
/// change the *step* row set and light the step-grid presence tick — pays for
/// the presence sync and one `ui_epoch` resync.
/// Bumping the epochs on every drag event rebuilt the whole *fx* rack panel
/// (every slot) per mouse move, which is what made rack knobs feel like
/// molasses once a step was selected (eseq-lf72).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum RackPlockRowsSync {
    /// Base value edit, or a later drag event of a lock that already existed:
    /// the bound param alone repaints the knob and the LOCK readout.
    Unchanged,
    /// This write can add a *step* row / presence tick: republish once.
    RowSetChanged,
}

impl RackPlockRowsSync {
    /// `existed` = the displayed step already carried this lock before the
    /// write. A later drag event of the same gesture is therefore `Unchanged`.
    pub(super) fn for_plock_write(existed: bool) -> Self {
        if existed {
            Self::Unchanged
        } else {
            Self::RowSetChanged
        }
    }
}

/// After a rack direct-param edit (a slot strip control, a slot
/// instrument or effect param): the rack panel binds the param (the host
/// kinds push its value, and the step grids' `step.plocked` / `lock-kind`),
/// so only a lock's first write (`RowSetChanged`) resyncs once (the step
/// panel's p-lock rows).
pub(super) fn refresh_rack_direct_param_reactive(
    plock_rows: RackPlockRowsSync,
    ui_epoch: &AtomicUsize,
) {
    if plock_rows == RackPlockRowsSync::RowSetChanged {
        // Once per gesture (the first lock write), never per drag event.
        ui_epoch.fetch_add(1, Ordering::Relaxed);
    }
}

pub(super) fn apply_rack_macro_host_command(
    name: &str,
    map: &HashMap<String, Rc<RefCell<Value>>>,
    app: &mut app::App,
    state: &Arc<SequencerState>,
    selected_steps: &Arc<Mutex<HashSet<usize>>>,
    ui_epoch: &AtomicUsize,
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
            let value = sequencer::sequencer::RackMacroField::Value(value.clamp(0.0, 1.0));
            return apply_rack_macro_edit_reactive(
                app,
                track,
                id,
                value,
                ui_epoch,
            );
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
                ui_epoch,
                RackPlockRowsSync::for_plock_write(plock_row_exists),
            );
        }
        _ => return false,
    }
    true
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
    let history_before = (!neural_selection.is_empty()).then(|| state.capture_project_scenes());
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
    let history_before = (!neural_selection.is_empty()).then(|| state.capture_project_scenes());
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
    pub(super) accumulator_names: &'a Arc<Mutex<Vec<String>>>,
    pub(super) fx_visible: bool,
    pub(super) sequencer_visible: bool,
    pub(super) mixer_visible: bool,
}

/// Apply the tick's typed UI invalidations to the host-side state they
/// move: the shared bus state, the recorded track collapse, the scene slot
/// invalidations, the sidebar, the accumulator names. (The views read the
/// host kinds, which diff the model themselves; the queue's generations feed
/// them.) Returns whether a reactive cycle is due.
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
        accumulator_names,
        fx_visible,
        sequencer_visible,
        mixer_visible,
    } = ctx;

    let mut needs_reactive_cycle = false;
    let mut bus_state_pulled = false;
    let active_track_count = state.active_track_count().min(app.tracks.len());
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
            | UiInvalidation::StepInvalidationBatch { track, .. }
            | UiInvalidation::StepBatch { track, .. }
            | UiInvalidation::StepSelection { track, .. }
            | UiInvalidation::TrackMixer { track, .. }
            | UiInvalidation::TrackBusSend { track, .. }
            | UiInvalidation::TrackRoute { track }
            | UiInvalidation::TrackParam { track, .. }
            | UiInvalidation::TrackParamPanel { track }
            | UiInvalidation::ProcessLaneValues { track }
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
            | UiInvalidation::CurrentTrack { .. }
            | UiInvalidation::TrackRoute { .. }
            | UiInvalidation::Transport(_)
            | UiInvalidation::AutoFollow
            | UiInvalidation::Browser(_) => {
                needs_reactive_cycle = true;
            }
            UiInvalidation::Pattern(PatternInvalidation::WholeTrack { .. }) => {
                needs_reactive_cycle |= sequencer_visible;
            }
            UiInvalidation::Pattern(PatternInvalidation::AllTracks)
            | UiInvalidation::Pattern(PatternInvalidation::TrackTiming { .. }) => {
                sync_scene_slot_state(rt, state);
                needs_reactive_cycle = true;
            }
            // The host kinds diff the steps, the selection, the track
            // settings, the process lanes, the p-locks and the piano roll
            // themselves (`step.*`, `track.*`, `selection.*`).
            UiInvalidation::Pattern(PatternInvalidation::TrackLength { .. })
            | UiInvalidation::Step { .. }
            | UiInvalidation::StepInvalidationBatch { .. }
            | UiInvalidation::StepBatch { .. }
            | UiInvalidation::StepSelection { .. }
            | UiInvalidation::ProcessLaneValues { .. }
            | UiInvalidation::PianoRoll { .. }
            // The send controls read eseq.kinds `send` (live).
            | UiInvalidation::TrackBusSend { .. }
            // The host kinds re-derive the routes (`route`).
            | UiInvalidation::ModRoutes
            | UiInvalidation::DeleteTarget => {}
            UiInvalidation::TrackMixer { change, .. } => {
                // The host kinds push the mixer fields (`track.volume`,
                // `muted`, `audible`, `armed`, …); a collapse is recorded.
                if change == TrackMixerInvalidation::Collapsed {
                    let collapsed = track_collapsed.lock().unwrap().clone();
                    if let Err(error) = app.apply_recorded_track_collapsed(collapsed) {
                        *track_collapsed.lock().unwrap() = app.track_collapsed.clone();
                        eprintln!("Could not change track collapse state: {error}");
                    }
                }
            }
            UiInvalidation::BusMixer { bus, .. } => {
                if !bus_state_pulled {
                    pull_shared_bus_state(app, bus_state);
                    bus_state_pulled = true;
                }
                if app.buses.get(bus).is_some() {
                    needs_reactive_cycle = true;
                }
            }
            UiInvalidation::TrackParam { track, .. }
            | UiInvalidation::TrackParamPanel { track }
            | UiInvalidation::ProcessChain { track } => {
                needs_reactive_cycle |= track == current_track_idx;
            }
            UiInvalidation::Instrument { track, change } => match change {
                // The panels bind the instrument's params and the step grids
                // read `step.plocked` / `lock-kind` (the host kinds).
                InstrumentInvalidation::Param { .. }
                | InstrumentInvalidation::Plock { .. }
                | InstrumentInvalidation::BaseNote
                | InstrumentInvalidation::SamplerSelectionTime
                // The waveform binds device.playhead (live).
                | InstrumentInvalidation::Playhead => {}
                InstrumentInvalidation::PanelTopology | InstrumentInvalidation::Analysis => {
                    needs_reactive_cycle |= fx_visible && track == current_track_idx;
                }
            },
            UiInvalidation::TrackFx { track, change } => match change {
                // The panels bind the effect's params (the host kinds).
                TrackFxInvalidation::Param { .. } | TrackFxInvalidation::Plock { .. } => {}
                TrackFxInvalidation::Topology | TrackFxInvalidation::PanelTree => {
                    needs_reactive_cycle |= fx_visible && track == current_track_idx;
                }
            },
            UiInvalidation::MidiFx { track, change } => match change {
                // The panels bind the MIDI effect's params (the host kinds).
                MidiFxInvalidation::Param { .. } => {}
                MidiFxInvalidation::Topology => {
                    needs_reactive_cycle |= fx_visible && track == current_track_idx;
                }
            },
            UiInvalidation::BusFx { change, .. } => match change {
                // The panels bind the bus effect's params (the host kinds).
                BusFxInvalidation::Param { .. } => {}
                BusFxInvalidation::Topology => {
                    needs_reactive_cycle |= mixer_visible || fx_visible;
                }
                BusFxInvalidation::PanelTree => {
                    needs_reactive_cycle |= fx_visible;
                }
            },
            UiInvalidation::Sidebar { track, .. } => {
                sync_sidebar_browser(app, track);
                needs_reactive_cycle = true;
            }
        }
    }

    if needs_reactive_cycle {
        *accumulator_names.lock().unwrap() = build_accumulator_names(app);
    }
    needs_reactive_cycle
}

pub(super) fn reset_sampler_waveform_view(editor: &mut Editor) {
    const RESET_VIEW: &str = "eseq.effects.sampler-panel/sampler-reset-view";
    // The bare `noui` root never loads the sampler panel (eseq-750i).
    if !editor.runtime_mut().has_global(RESET_VIEW) {
        return;
    }
    if let Err(error) = editor
        .runtime_mut()
        .eval_str("(eseq.effects.sampler-panel/sampler-reset-view)")
    {
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
    current_track: &Arc<AtomicUsize>,
    track_names: &mut Vec<String>,
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
        InstrumentType::Empty | InstrumentType::Sampler | InstrumentType::Custom | InstrumentType::Rack
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

    // A single-layer rack container (what a kit pad or a Sound preset loads
    // as) takes the sample the way a sampler track does: into the pattern it
    // is playing, so a rack clip or scene can hold its own sample while the
    // others keep theirs. Replacing the whole container would rewrite every
    // pattern of the track.
    let single_layer_rack = instrument_type == InstrumentType::Rack
        && path.is_some()
        && app
            .state
            .live_rack_track_snapshot(track)
            .is_some_and(|rack| rack.slots.len() == 1);
    if single_layer_rack {
        let path = path.expect("checked above");
        app.apply_recorded_rack_slot_source_replacement(
            track,
            0,
            "Replace rack sample",
            |app| app.graph_controller().replace_rack_slot_with_sampler(track, 0, path),
        )?;
        register_waveform_sample(path);
        reset_sampler_waveform_view(editor);
        let selected_track = host_commands::selection_after_track_apply(
            track,
            preserve_track_selection,
            current_track,
            app.tracks.len(),
        );
        current_track.store(selected_track, Ordering::Relaxed);
        app.ui.cursor_track = selected_track;
        let rt = editor.runtime_mut();
        sync_sidebar_browser(app, selected_track);
        rt.run_reactive_cycle();
        editor.refresh_runtime_side_effects();
        let name = sequencer::sample_db::display_title_for_sample_path(path)
            .unwrap_or_else(|| app.tracks[track].clone());
        return Ok(SamplerTrackLoadResult {
            name,
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
    let reset_summary =
        app.apply_recorded_instrument_binding_mutation(track, "Replace instrument", |app| {
            let reset_summary = match instrument_type {
                InstrumentType::Sampler => {
                    app.graph_controller().send_sample_to_all_voices(
                        track,
                        new_buffer_id,
                        sample_rate,
                    );
                    app.graph.track_buffer_ids[track] = new_buffer_id;
                    app.graph.track_sample_rates[track] = sample_rate;
                    app.tracks[track] = new_name.clone();
                    app.state.seed_unset_pattern_sample_ids(
                        track,
                        (new_buffer_id, new_name.clone(), sample_rate),
                    );
                    None
                }
                InstrumentType::Custom => {
                    Some(app.graph_controller().convert_custom_track_to_sampler(
                        track,
                        new_buffer_id,
                        sample_rate,
                        &new_name,
                    )?)
                }
                InstrumentType::Empty | InstrumentType::Rack => {
                    let summary = app.graph_controller().replace_unvoiced_track_with_sampler(
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
        })?;
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
    crate::param_words::set_track_word_names(track_names);

    let rt = editor.runtime_mut();
    sync_sidebar_browser(app, selected_track);
    rt.run_reactive_cycle();
    editor.refresh_runtime_side_effects();
    Ok(SamplerTrackLoadResult {
        name: new_name,
        reset_summary,
    })
}
