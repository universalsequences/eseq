use crate::*;
use eseqlisp::metal_backend::MetalBackend;

type PendingPointerDrag = (crossterm::event::MouseEvent, (f32, f32));

fn flush_pending_pointer_drag(
    pending_drag: &mut Option<PendingPointerDrag>,
    mut dispatch: impl FnMut(crossterm::event::MouseEvent, f32, f32),
) -> bool {
    let Some((mouse, (precise_col, precise_row))) = pending_drag.take() else {
        return false;
    };
    dispatch(mouse, precise_col, precise_row);
    true
}

/// The metal_seq event loop: input polling, gestures, async polling, and
/// host-command dispatch, ending each iteration in the reactive tick.
#[allow(clippy::too_many_lines)]
pub(crate) fn run_event_loop(
    mut app: app::App,
    mut editor: Editor,
    mut backend: MetalBackend,
    mut track_names: Vec<String>,
    shared: SharedHandles,
) -> Result<(), Box<dyn std::error::Error>> {
    // 5. Metal event loop
    let idle_frame_interval = Duration::from_secs_f64(1.0 / 30.0);
    let animation_frame_interval = Duration::from_secs_f64(1.0 / 60.0);
    let mut last_render_at = Instant::now() - idle_frame_interval;
    let mut stub_animation_cache = StubAnimationRenderCache::new();
    let mut pending_drag: Option<PendingPointerDrag> = None;
    let mut sample_import_session: Option<SampleImportSession> = None;
    let mut scroll_accum_y: f32 = 0.0;
    let mut scroll_accum_x: f32 = 0.0;
    let mut soft_step_param_edit = SoftStepParamEdit::default();
    let mut lisp_hot_reload_watcher = LispHotReloadWatcher::start(watched_lisp_paths(&editor));
    let mut lisp_hot_reload_source_revision = editor.runtime().lisp_source_revision();
    let mut last_lisp_hot_reload_path_scan = Instant::now();

    let mut gesture = GestureState {
        // Pointer-rate rack edits update the live graph immediately. Their large
        // immutable scheduler snapshot is committed once at gesture end instead
        // of being rebuilt for every mouse pixel.
        rack_control_snapshot_dirty: false,
        piano_roll_history_gesture: None,
        preview_plock_variant: None,
    };

    // Inline editor session state (instrument/effect creation/editing)
    let mut sessions = EditSessionState {
        editor_buffer_name: None,
        editor_mode: None,
        instrument_edit_session: None,
        pending_instrument_preview: None,
        pending_instrument_cancel_restore: None,
        pending_saved_instrument_load: None,
        pending_key_lock_auditions: Vec::new(),
        effect_edit_session: None,
        pending_effect_preview: None,
        pending_effect_cancel_restore: None,
        script_draft_session: None,
        pending_agentic_bubbles: HashMap::new(),
        pending_learn_job: None,
        learn_param_preview: None,
        pending_lisp_history_transactions: HashMap::new(),
    };
    let mut frame = FrameDiffState {
        prev_editor_macro_action: (String::new(), String::new()),
        prev_editor_macro_action_fingerprint: u64::MAX,
        prev_editor_macro_sidebar_fingerprint: u64::MAX,
        prev_editor_open_macro: String::new(),
        prev_playing: false,
        prev_bpm: 0,
        prev_playhead: u32::MAX,
        prev_transport_playhead: u32::MAX,
        prev_pattern_epoch: 0,
        prev_song_row_mirror_epoch: 0,
        prev_current_track: usize::MAX,
        prev_cpu_load_bits: u32::MAX,
        prev_peak_l_level: -1.0f64,
        prev_peak_r_level: -1.0f64,
        recording_history_open: false,
        prev_master_recording: false,
        prev_selected_tracks: HashSet::new(),
        prev_groups: Vec::new(),
        prev_track_peak_levels: Vec::new(),
        prev_rack_slot_peak_levels: Vec::new(),
        prev_bus_peak_levels: Vec::new(),
        prev_modulator_phases: Vec::new(),
        prev_modulator_levels: Vec::new(),
        prev_bus_playheads: Vec::new(),
        prev_track_playheads: Vec::new(),
        prev_track_button_states: track_button_state_snapshot(&shared.state),
        prev_current_track_playhead_visible: false,
        prev_ui_epoch: 0,
        prev_fx_epoch: 0,
        prev_fx_value_epoch: 0,
        prev_sound_binding_epoch: 0,
        prev_delete_target_version: 0,
        track_param_sync_revision: None,
        fx_param_sync_revision: None,
        prev_focus_clip_surface: (None, None, u64::MAX),
        prev_instrument_active_notes: Vec::new(),
        prev_track_active_notes: Vec::new(),
        prev_active_buffer_name: editor.active_buffer().name.clone(),
        prev_selected_neural_neurons: shared.selected_neural_neurons.lock().unwrap().clone(),
        prev_agent_generation_watermark: agent_generation_watermark(&app),
        prev_sampler_analysis_key: None,
        prev_auto_follow: true,
        prev_browser_preview_playing: false,
        prev_queued_transport_scene: None,
        prev_queued_track_clips: Vec::new(),
        song: SongFrameState::default(),
        sound_palette: SoundPaletteFrameState::default(),
        watched_sampler_voice_track: None,
        watched_sampler_voice_ids: Vec::new(),
    };
    let (initial_modulator_phases, initial_modulator_levels) =
        read_modulator_display_values(app.graph.lg, &app);
    let mut meters = MeterCache {
        cached_peak_l_level: 0.0f64,
        cached_peak_r_level: 0.0f64,
        cached_track_peak_levels: vec![0.0; track_names.len()],
        cached_rack_slot_peak_levels: Vec::new(),
        cached_bus_peak_levels: read_bus_peak_levels(app.graph.lg, &app.graph.bus_node_ids),
        cached_modulator_phases: initial_modulator_phases,
        cached_modulator_levels: initial_modulator_levels,
        cached_cpu_load_bits: 0.0f32.to_bits(),
        last_meter_poll_at: Instant::now() - METER_POLL_INTERVAL,
        last_cpu_ui_poll_at: Instant::now() - CPU_UI_POLL_INTERVAL,
        last_neural_visualization_poll_at: Instant::now() - NEURAL_VISUALIZATION_POLL_INTERVAL,
        visualization_liveness: VisualizationLiveness::default(),
        last_voice_count_log_at: Instant::now() - VOICE_COUNT_LOG_INTERVAL,
    };
    let mut live_audio_analyzer = LiveAudioAnalyzerManager::new(app.graph.lg);
    let log_voice_counts = std::env::var_os("TINYSEQ_LOG_VOICE_COUNTS").is_some();
    if log_voice_counts {
        sequencer::instruments::voice_modulator::set_process_stats_enabled(true);
    }

    eprintln!("metal_seq: entering event loop");
    let mut ui_loop_stats = UiLoopStats::new();
    let mut pointer_is_down = false;

    loop {
        let mut pointer_released_this_loop = false;
        for result in app.drain_due_pattern_launches() {
            match result {
                Ok(outcome) => editor.handle_host_event(HostEvent::Status(format!(
                    "Applied quantized pattern launch {}",
                    outcome.token.unwrap_or_default()
                ))),
                Err(error) => editor.handle_host_event(HostEvent::Error(format!(
                    "Quantized pattern launch failed: {error:?}"
                ))),
            }
        }
        // Song playback notices (docs/song-mode-spec.md 10.2): mirror
        // scheduler-authoritative row transitions control-side (no epoch
        // bump), stop through the state machine on end, surface start
        // failures. Slice C capture will additionally consume `RowApplied`
        // records and must check `song_playback().take_notice_overflow()`
        // before committing a take.
        for notice in shared.state.drain_song_playback_notices() {
            match notice {
                sequencer::sequencer::SongPlaybackNotice::RowApplied(applied) => {
                    if let Err(error) = app.mirror_song_row_applied(&applied) {
                        editor.handle_host_event(HostEvent::Error(format!(
                            "Song row apply failed: {error}"
                        )));
                    }
                }
                sequencer::sequencer::SongPlaybackNotice::Ended { .. } => {
                    match app.handle_song_playback_ended() {
                        Ok(Some(status)) => editor.handle_host_event(HostEvent::Status(status)),
                        Ok(None) => {}
                        Err(error) => editor.handle_host_event(HostEvent::Error(format!(
                            "Song playback stop failed: {error}"
                        ))),
                    }
                }
                sequencer::sequencer::SongPlaybackNotice::StartFailed { error } => {
                    let message = app.handle_song_playback_start_failed(&error);
                    editor.handle_host_event(HostEvent::Error(message));
                }
            }
        }
        app.graph_controller().reap_due_rack_teardowns();
        let queued_transport_scene = shared
            .state
            .quantized_launches()
            .pending_target(sequencer::quantized_launch::QuantizedLaunchOwner::Transport)
            .and_then(|target| match target {
                sequencer::quantized_launch::PatternLaunchTarget::Scene { scene }
                | sequencer::quantized_launch::PatternLaunchTarget::SceneTracks { scene, .. } => {
                    Some(scene)
                }
                sequencer::quantized_launch::PatternLaunchTarget::TrackPattern { .. } => None,
            });
        if queued_transport_scene != frame.prev_queued_transport_scene {
            let rt = editor.runtime_mut();
            rt.set_reactive(
                "SEQ",
                "queued-scene",
                Value::Number(
                    queued_transport_scene
                        .map(|scene| scene as f64)
                        .unwrap_or(-1.0),
                ),
            );
            rt.run_reactive_cycle();
            editor.refresh_runtime_side_effects();
            if editor_has_visible_buffer(&editor, "*transport*") {
                editor.refresh_visible_layouts_for_buffer_named("*transport*");
            }
            editor.mark_needs_redraw();
            frame.prev_queued_transport_scene = queued_transport_scene;
        }
        // Pending quantized clip launches, as the pattern id each track has
        // queued (-1 = none). The queued clip is the just-assigned scene
        // cell (the click assigns the cell up front and defers the audible
        // restore), so resolve the pending SceneTracks target's cell. Drives
        // the mixer grid's blinking queued-cell background.
        let queued_track_clips: Vec<i64> = (0..app.tracks.len())
            .map(|track| {
                shared
                    .state
                    .quantized_launches()
                    .pending_target(
                        sequencer::quantized_launch::QuantizedLaunchOwner::TrackClip(track as u32),
                    )
                    .and_then(|target| match target {
                        sequencer::quantized_launch::PatternLaunchTarget::SceneTracks {
                            scene,
                            ..
                        } => shared
                            .state
                            .scene_track_pattern_id(scene, track)
                            .map(|id| id.0 as i64),
                        // Song-authority override launches name the pattern
                        // directly.
                        sequencer::quantized_launch::PatternLaunchTarget::TrackPattern {
                            pattern,
                            ..
                        } => Some(pattern as i64),
                        sequencer::quantized_launch::PatternLaunchTarget::Scene { .. } => None,
                    })
                    .unwrap_or(-1)
            })
            .collect();
        if queued_track_clips != frame.prev_queued_track_clips {
            let rt = editor.runtime_mut();
            rt.set_reactive(
                "SEQ",
                "queued-track-clips",
                Value::List(
                    queued_track_clips
                        .iter()
                        .map(|id| Rc::new(RefCell::new(Value::Number(*id as f64))))
                        .collect(),
                ),
            );
            rt.run_reactive_cycle();
            editor.refresh_runtime_side_effects();
            if editor_has_visible_mixer_buffer(&editor) {
                refresh_visible_mixer_layouts(&mut editor);
            }
            editor.mark_needs_redraw();
            frame.prev_queued_track_clips = queued_track_clips;
        }
        let sample_browser_ready = { shared.sample_browser.borrow_mut().poll_ready() };
        match sample_browser_ready {
            Ok(true) => {
                if let Err(error) = refresh_sample_browser_buffer(&mut editor) {
                    editor.handle_host_event(HostEvent::Error(format!(
                        "Failed to refresh sample browser search: {error}"
                    )));
                }
            }
            Ok(false) => {}
            Err(error) => {
                editor.handle_host_event(HostEvent::Error(format!(
                    "Failed to query samples.db browser state: {error}"
                )));
            }
        }
        if let Some(watcher) = lisp_hot_reload_watcher.as_mut() {
            let source_revision = editor.runtime().lisp_source_revision();
            if source_revision != lisp_hot_reload_source_revision
                || last_lisp_hot_reload_path_scan.elapsed() >= Duration::from_secs(1)
            {
                watcher.set_watched_paths(watched_lisp_paths(&editor));
                lisp_hot_reload_source_revision = source_revision;
                last_lisp_hot_reload_path_scan = Instant::now();
            }
            let changed_paths = watcher.poll_ready_paths();
            if !changed_paths.is_empty()
                && process_lisp_hot_reload_paths(&mut editor, changed_paths)
            {
                shared.ui_epoch.fetch_add(1, Ordering::Relaxed);
            }
        }
        if backend.poll_editable_shader_overrides() {
            editor.mark_needs_redraw();
        }
        release_due_key_lock_auditions(
            &mut sessions.pending_key_lock_auditions,
            &shared.keyboard_tx,
            Instant::now(),
        );
        pull_shared_bus_state(&mut app, &shared.bus_state);
        let recording_now = shared.recording.load(Ordering::Relaxed);
        if let Err(error) = app.sync_recording_history_boundary(
            recording_now,
            shared.state.is_playing(),
            &mut frame.recording_history_open,
        ) {
            shared.recording.store(false, Ordering::Relaxed);
            editor.handle_host_event(HostEvent::Error(format!(
                "Recording history failed: {error}"
            )));
        }
        if !app.has_pending_project_load() {
            pull_named_scratch_buffer_into_project(&editor, &mut app);
        }
        editor.update_timers();
        let active_buffer_name = editor.active_buffer().name.clone();
        if active_buffer_name != frame.prev_active_buffer_name {
            frame.prev_active_buffer_name = active_buffer_name;
            let mut guard = shared.active_delete_target.lock().unwrap();
            let should_clear = should_clear_active_delete_target_for_buffer(
                guard.as_ref(),
                &frame.prev_active_buffer_name,
            );
            if should_clear {
                guard.take();
                drop(guard);
                shared
                    .active_delete_target_version
                    .fetch_add(1, Ordering::Relaxed);
                shared.ui_epoch.fetch_add(1, Ordering::Relaxed);
            }
        }
        let agent_generation = agent_generation_watermark(&app);
        if agent_generation != frame.prev_agent_generation_watermark {
            eprintln!(
                "[agent-ui] generation changed {} -> {}; refreshing *agent*",
                frame.prev_agent_generation_watermark, agent_generation
            );
            frame.prev_agent_generation_watermark = agent_generation;
            {
                let rt = editor.runtime_mut();
                rt.set_reactive(
                    "AGENT",
                    "generation",
                    Value::Number(agent_generation as f64),
                );
                rt.run_reactive_cycle();
            }
            editor.refresh_runtime_side_effects();
            editor.refresh_visible_layouts_for_buffer_named("*agent*");
            editor.refresh_visible_layouts_for_buffer_named("*agent-artifacts*");
            editor.mark_needs_redraw();
        }
        let (cols, rows) = backend.viewport_size();
        let (cell_w, cell_h) = backend.cell_dimensions();
        if let Some((text_cell_w, text_cell_h)) = backend.sync_text_zoom(editor.text_zoom()) {
            editor.set_text_cell_dimensions(cell_w, cell_h, text_cell_w, text_cell_h);
        }
        if cell_w > 0.0 {
            editor.set_layout_aspect(cell_h / cell_w);
        }
        editor.update_tile_rects(cols as u16, rows as u16);
        editor.sync_reactive_bindings_for_visible_layouts();
        if live_audio_analyzer.sync_visible(&editor, &app) {
            editor.mark_needs_redraw();
        }
        if log_voice_counts && meters.last_voice_count_log_at.elapsed() >= VOICE_COUNT_LOG_INTERVAL
        {
            log_active_voice_counts(&shared.state, &track_names);
            meters.last_voice_count_log_at = Instant::now();
        }

        let viewport_size = (cols, rows);
        let stub_animation_active = stub_animation_cache.is_active(
            viewport_size,
            backend.agent_instrument_stub_animation_visible(),
        );
        let widget_animation_active = editor.visible_widgets_animating();
        let learn_ui_active = sessions.pending_learn_job.is_some();
        let frame_interval =
            if stub_animation_active || widget_animation_active || learn_ui_active {
                animation_frame_interval
            } else {
                idle_frame_interval
            };

        let sdf_animation_active =
            eseqlisp::widget_render::sdf_widget::sdf_visual_animations_active(
                backend.time_seconds(),
            );
        if sdf_animation_active {
            editor.mark_needs_redraw();
            let elapsed = last_render_at.elapsed();
            if elapsed < frame_interval {
                std::thread::sleep(frame_interval - elapsed);
            }
            let frame_build_started = Instant::now();
            let tiled_frame =
                eseqlisp::frame::build_tiled_render_frame_borderless(&mut editor, cols, rows);
            let frame_build_elapsed = frame_build_started.elapsed();
            let render_started = Instant::now();
            let render_status = backend
                .render_tiled(&tiled_frame)
                .map_err(|_| "render failed")?;
            let render_elapsed = render_started.elapsed();
            ui_loop_stats.note_frame(frame_build_elapsed, render_elapsed);
            match render_status {
                eseqlisp::metal_backend::TiledRenderStatus::Presented => {
                    editor.clear_needs_redraw();
                    last_render_at = Instant::now();
                }
                eseqlisp::metal_backend::TiledRenderStatus::NotPresented => {
                    eseqlisp::frame::requeue_unpresented_tiled_frame(&mut editor, &tiled_frame);
                    last_render_at = Instant::now();
                }
            }
            continue;
        }
        if widget_animation_active {
            editor.mark_needs_redraw();
        }

        // 1. Poll events FIRST
        let playing_now = shared.state.transport.playing.load(Ordering::Relaxed);
        let timeout = if playing_now {
            frame_interval
                .saturating_sub(last_render_at.elapsed())
                .max(Duration::from_millis(8))
        } else if learn_ui_active {
            frame_interval
                .saturating_sub(last_render_at.elapsed())
                .max(Duration::from_millis(4))
        } else if editor.needs_redraw() {
            Duration::from_millis(4)
        } else {
            Duration::from_millis(50)
        };
        if let Some(event) = backend.poll_backend_event(timeout) {
            let event_started = Instant::now();
            match event {
                BackendEvent::FileDrop(paths) => {
                    match SampleImportSession::from_drop(paths, Path::new("samples.db")) {
                        Ok(session) => {
                            if session.is_empty() {
                                editor.show_transient_message(
                                    "No supported audio files found in dropped items",
                                );
                            } else {
                                sample_import_session = Some(session);
                                if let Some(session) = sample_import_session.as_ref() {
                                    session.render_into_editor(&mut editor);
                                }
                                editor.show_transient_message("Sample import staged");
                            }
                        }
                        Err(error) => {
                            editor.show_transient_message(format!("Sample import failed: {error}"));
                        }
                    }
                }
                BackendEvent::Terminal(Event::Key(raw_key)) => {
                    if raw_key.kind == crossterm::event::KeyEventKind::Release {
                        app::edit::finish_active_gesture(&mut app);
                    }
                    if raw_key.kind == crossterm::event::KeyEventKind::Press
                        && editor.modal_is_open()
                    {
                        // Transport stays global while a modal is open; the
                        // focused-text-input guard inside keeps typed spaces
                        // in the modal's edit fields.
                        let key = normalize_command_shortcuts(raw_key);
                        if should_toggle_play_on_space(&editor, &key) {
                            let _ = editor.runtime_mut().eval_str("(seq-toggle-play)");
                            editor.refresh_runtime_side_effects();
                            ui_loop_stats.note_event(event_started.elapsed());
                            continue;
                        }
                        // Every other app-level shortcut yields to the modal.
                        // Editor routing sends the key to the modal's owning
                        // tile and consumes it there even when no modal
                        // control handles it.
                        editor.handle_key(key);
                        ui_loop_stats.note_event(event_started.elapsed());
                        continue;
                    }
                    if editor.active_buffer().name == "*sample-import*" {
                        let key = normalize_command_shortcuts(raw_key);
                        if let Some(session) = sample_import_session.as_mut() {
                            match session.handle_key(key) {
                                ImportKeyOutcome::Handled => {
                                    session.render_into_editor(&mut editor);
                                    ui_loop_stats.note_event(event_started.elapsed());
                                    continue;
                                }
                                ImportKeyOutcome::Cancel => {
                                    sample_import_session = None;
                                    switch_to_sequencer(&mut editor);
                                    editor.show_transient_message("Sample import canceled");
                                    ui_loop_stats.note_event(event_started.elapsed());
                                    continue;
                                }
                                ImportKeyOutcome::Commit => {
                                    let summary = session
                                        .commit(Path::new("samples.db"), Path::new("samples"));
                                    sample_import_session = None;
                                    switch_to_sequencer(&mut editor);
                                    match summary {
                                        Ok(summary) => {
                                            editor.show_transient_message(format!(
                                                "Imported {} sample(s), skipped {} duplicate(s), {} failed",
                                                summary.imported, summary.duplicates, summary.failed
                                            ));
                                            let _ = refresh_sample_browser_buffer(&mut editor);
                                        }
                                        Err(error) => {
                                            editor.show_transient_message(format!(
                                                "Sample import failed: {error}"
                                            ));
                                        }
                                    }
                                    ui_loop_stats.note_event(event_started.elapsed());
                                    continue;
                                }
                                ImportKeyOutcome::Ignored => {}
                            }
                        }
                    }
                    if raw_key.kind == crossterm::event::KeyEventKind::Press {
                        if raw_key.code == crossterm::event::KeyCode::Esc
                            && raw_key.modifiers == crossterm::event::KeyModifiers::NONE
                            && app.history.active_gesture().is_some()
                        {
                            match app::edit::cancel_active_gesture(&mut app) {
                                Ok(true) => {
                                    editor.show_transient_message("Parameter edit canceled")
                                }
                                Ok(false) => {}
                                Err(error) => editor.handle_host_event(HostEvent::Error(format!(
                                    "Could not cancel parameter edit: {error:?}"
                                ))),
                            }
                            pending_drag = None;
                            pointer_is_down = false;
                            shared.ui_epoch.fetch_add(1, Ordering::Relaxed);
                            ui_loop_stats.note_event(event_started.elapsed());
                            continue;
                        }
                        if let Some(gesture) = gesture.piano_roll_history_gesture.take() {
                            let track = gesture.track;
                            let cancel = raw_key.code == crossterm::event::KeyCode::Esc
                                && raw_key.modifiers == crossterm::event::KeyModifiers::NONE;
                            let finalized = if cancel {
                                gesture
                                    .transaction
                                    .rollback(&mut app)
                                    .map(|()| app::edit::EditOutcome::NoOp)
                            } else {
                                gesture.transaction.commit(&mut app)
                            };
                            *shared.piano_roll_move_state.lock().unwrap() = None;
                            shared.ui_invalidations.push(UiInvalidation::PianoRoll {
                                track,
                                change: PianoRollInvalidation::Items,
                            });
                            match finalized {
                                Ok(_) => {
                                    shared.ui_epoch.fetch_add(1, Ordering::Relaxed);
                                }
                                Err(error) => {
                                    editor.handle_host_event(HostEvent::Error(format!(
                                        "Could not finalize interrupted piano-roll gesture: {error:?}"
                                    )));
                                    ui_loop_stats.note_event(event_started.elapsed());
                                    continue;
                                }
                            }
                        }
                    }
                    if let Some(shortcut) = sequencer_history_shortcut(&editor, &raw_key) {
                        if shared.recording.load(Ordering::Relaxed) {
                            shared.recording.store(false, Ordering::Relaxed);
                            frame.recording_history_open = false;
                            app.ui.recording = false;
                        }
                        let track_count_before_replay = app.tracks.len();
                        let replay = match shortcut {
                            SequencerHistoryShortcut::Undo => app::edit::undo(&mut app),
                            SequencerHistoryShortcut::Redo => app::edit::redo(&mut app),
                        };
                        let message = match replay {
                            app::history::HistoryReplay::Applied(result) => {
                                let topology_changed =
                                    app.tracks.len() != track_count_before_replay;
                                if !topology_changed {
                                    track_names.clone_from(&app.tracks);
                                }
                                let replay_track = if topology_changed {
                                    app.ui.cursor_track
                                } else {
                                    shared.current_track.load(Ordering::Relaxed)
                                }
                                .min(app.tracks.len().saturating_sub(1));
                                shared.current_track.store(replay_track, Ordering::Relaxed);
                                *shared.bus_state.lock().unwrap() = app.buses.clone();
                                *shared.bus_node_ids.lock().unwrap() =
                                    app.graph.bus_node_ids.clone();
                                *shared.track_groups.lock().unwrap() = app.groups.clone();
                                if topology_changed {
                                    {
                                        let mut pan_ids = shared.track_pan_ids.lock().unwrap();
                                        *pan_ids = app
                                            .graph
                                            .track_node_ids
                                            .iter()
                                            .map(|ids| ids.pan_id)
                                            .collect();
                                        push_solo_mutes(shared.lg_raw, &shared.state, &pan_ids);
                                    }
                                    meters.cached_track_peak_levels = read_track_peak_levels(
                                        app.graph.lg,
                                        &shared.track_pan_ids.lock().unwrap(),
                                    );
                                    meters.cached_bus_peak_levels =
                                        read_bus_peak_levels(app.graph.lg, &app.graph.bus_node_ids);
                                    (
                                        meters.cached_modulator_phases,
                                        meters.cached_modulator_levels,
                                    ) = read_modulator_display_values(app.graph.lg, &app);
                                    meters.last_meter_poll_at = Instant::now();
                                    *shared.record_armed.lock().unwrap() =
                                        app.graph.record_armed.clone();
                                    *shared.track_groups.lock().unwrap() = app.groups.clone();
                                }
                                let rt = editor.runtime_mut();
                                if topology_changed {
                                    sync_track_topology_state(
                                        rt,
                                        &app,
                                        &shared.state,
                                        &mut track_names,
                                        replay_track,
                                        &shared.selected_steps,
                                        &shared.piano_roll_selection,
                                        &shared.accumulator_names,
                                        &shared.record_armed,
                                        &meters.cached_track_peak_levels,
                                    );
                                    sync_bus_peak_fields(rt, &meters.cached_bus_peak_levels);
                                    sync_modulator_phase_fields(
                                        rt,
                                        &meters.cached_modulator_phases,
                                    );
                                    sync_modulator_level_fields(
                                        rt,
                                        &meters.cached_modulator_levels,
                                    );
                                    rt.clear_subtree_effects_for_named_target("*sequencer*");
                                }
                                sync_bus_mixer_state(rt, &app);
                                sync_groups_bindings(rt, &app.groups);
                                rt.set_reactive(
                                    "SEQ",
                                    "track-names",
                                    build_track_names(&track_names),
                                );
                                if !app.tracks.is_empty() {
                                    rt.set_reactive(
                                        "SEQ",
                                        "instrument-panel",
                                        build_instrument_panel_value(
                                            &app,
                                            replay_track,
                                            &shared.selected_steps,
                                        ),
                                    );
                                }
                                rt.run_reactive_cycle();
                                editor.refresh_runtime_side_effects();
                                if topology_changed {
                                    refresh_visible_track_topology_layouts(&mut editor);
                                    frame.prev_track_playheads =
                                        track_playheads_snapshot(&shared.state, &app);
                                    frame.prev_track_button_states =
                                        track_button_state_snapshot(&shared.state);
                                }
                                if !app.buses.is_empty() {
                                    shared.ui_invalidations.push(UiInvalidation::BusMixer {
                                        bus: 0,
                                        change: BusMixerInvalidation::Volume,
                                    });
                                }
                                shared
                                    .ui_invalidations
                                    .push(UiInvalidation::Pattern(PatternInvalidation::AllTracks));
                                shared.fx_epoch.fetch_add(1, Ordering::Relaxed);
                                shared.ui_epoch.fetch_add(1, Ordering::Relaxed);
                                match shortcut {
                                    SequencerHistoryShortcut::Undo => {
                                        format!("Undid {}", result.label)
                                    }
                                    SequencerHistoryShortcut::Redo => {
                                        format!("Redid {}", result.label)
                                    }
                                }
                            }
                            app::history::HistoryReplay::Unavailable => match shortcut {
                                SequencerHistoryShortcut::Undo => "Nothing to undo".to_string(),
                                SequencerHistoryShortcut::Redo => "Nothing to redo".to_string(),
                            },
                            app::history::HistoryReplay::Failed(error) => match shortcut {
                                SequencerHistoryShortcut::Undo => {
                                    format!("Could not undo: {error:?}")
                                }
                                SequencerHistoryShortcut::Redo => {
                                    format!("Could not redo: {error:?}")
                                }
                            },
                        };
                        editor.show_transient_message(message);
                        editor.mark_needs_redraw();
                        ui_loop_stats.note_event(event_started.elapsed());
                        continue;
                    }
                    if handle_metal_command_shortcut_with_ui_epoch(
                        &mut editor,
                        &raw_key,
                        &shared.state,
                        &shared.current_track,
                        &shared.selected_steps,
                        &shared.step_clipboard,
                        &shared.ui_epoch,
                    ) {
                        if key_should_reveal_sequencer_track(&raw_key) {
                            let track = shared.current_track.load(Ordering::Relaxed);
                            reveal_sequencer_current_track(&mut editor, &app, track);
                        }
                        editor.mark_needs_redraw();
                        ui_loop_stats.note_event(event_started.elapsed());
                        continue;
                    }
                    let key = normalize_command_shortcuts(raw_key);
                    if key.kind == crossterm::event::KeyEventKind::Press
                        && key.code == crossterm::event::KeyCode::Esc
                    {
                        let cleared_neural_selection = {
                            let mut selection = shared.selected_neural_neurons.lock().unwrap();
                            let had_selection = !selection.is_empty();
                            selection.clear();
                            had_selection
                        };
                        if cleared_neural_selection {
                            let selection = shared.selected_neural_neurons.lock().unwrap().clone();
                            sync_selected_neural_neuron_bindings(
                                editor.runtime_mut(),
                                &shared.state,
                                &selection,
                            );
                            let track = shared.current_track.load(Ordering::Relaxed);
                            sync_fx_param_binding_fields_with_neural_selection(
                                editor.runtime_mut(),
                                &app,
                                &shared.state,
                                track,
                                &shared.selected_steps,
                                Some(&selection),
                            );
                            sync_track_plocks_for_neural_selection(
                                editor.runtime_mut(),
                                &app,
                                &shared.state,
                                track,
                                &shared.selected_steps,
                                &selection,
                            );
                            frame.prev_selected_neural_neurons = selection;
                            editor.mark_needs_redraw();
                            ui_loop_stats.note_event(event_started.elapsed());
                            continue;
                        }
                    }
                    if should_toggle_play_on_space(&editor, &key) {
                        let _ = editor.runtime_mut().eval_str("(seq-toggle-play)");
                        editor.refresh_runtime_side_effects();
                        ui_loop_stats.note_event(event_started.elapsed());
                        continue;
                    }
                    if handle_metal_soft_step_param_key(
                        &mut editor,
                        &key,
                        &mut app,
                        &shared.current_track,
                        &shared.expanded_step_projection,
                        &mut soft_step_param_edit,
                    ) {
                        ui_loop_stats.note_event(event_started.elapsed());
                        continue;
                    }
                    // Intercept keyboard for live recording when any track is armed
                    let any_armed = shared.record_armed.lock().unwrap().iter().any(|a| *a);
                    let recording_key_outcome = if (any_armed
                        || held_note_for_key(&shared.held_notes, &key))
                        && should_route_to_live_keyboard(&editor, &key, &shared.held_notes)
                    {
                        handle_recording_key(
                            &key,
                            &mut app,
                            &shared.state,
                            &shared.record_armed,
                            &shared.recording,
                            &shared.keyboard_tx,
                            &shared.keyboard_octave,
                            &shared.held_notes,
                            &shared.ui_epoch,
                        )
                    } else {
                        RecordingKeyOutcome::Ignored
                    };
                    let intercepted = recording_key_outcome.consumed();
                    if recording_key_outcome.recorded_take() {
                        // Take-retargeted notes touch neither the live
                        // pattern nor the step grid; the timeline preview
                        // updates at commit.
                        editor.mark_needs_redraw();
                    }
                    if recording_key_outcome.recorded() {
                        app.mark_recording_take_changed();
                        let ct = shared.current_track.load(Ordering::Relaxed);
                        let rt = editor.runtime_mut();
                        rt.set_reactive("SEQ", "steps", build_steps_value(&shared.state, ct));
                        sync_all_track_sequencer_state(
                            rt,
                            &shared.state,
                            &app,
                            ct,
                            &shared.selected_steps,
                        );
                        rt.run_reactive_cycle();
                        editor.refresh_runtime_side_effects();
                        editor.refresh_visible_layouts_for_buffer_named("*sequencer*");
                        editor.mark_needs_redraw();
                    }
                    // Only pass Press events to the editor (Release is only for note-off)
                    if !intercepted && key.kind == crossterm::event::KeyEventKind::Press {
                        let should_reload_custom_ui = should_reload_custom_ui_after_key(&key);
                        let previous_track = shared.current_track.load(Ordering::Relaxed);
                        editor.handle_key(key);
                        if should_reload_custom_ui {
                            reload_custom_instrument_ui(&mut editor);
                        }
                        let next_track = shared.current_track.load(Ordering::Relaxed);
                        if editor.active_buffer().name == "*sequencer*"
                            && next_track != previous_track
                        {
                            let track = shared.current_track.load(Ordering::Relaxed);
                            reveal_sequencer_current_track(&mut editor, &app, track);
                        }
                    }
                }
                BackendEvent::Terminal(Event::Mouse(mouse)) => {
                    if matches!(mouse.kind, crossterm::event::MouseEventKind::Down(_)) {
                        pointer_is_down = true;
                    }
                    let (precise_col, precise_row) = backend
                        .take_last_precise_mouse()
                        .unwrap_or((mouse.column as f32, mouse.row as f32));
                    if matches!(
                        mouse.kind,
                        crossterm::event::MouseEventKind::Drag(crossterm::event::MouseButton::Left)
                    ) {
                        pending_drag = Some((mouse, (precise_col, precise_row)));
                    } else {
                        if matches!(mouse.kind, crossterm::event::MouseEventKind::Up(_)) {
                            // The backend may deliver drag and release in the same
                            // poll batch. Preserve their order instead of letting
                            // release discard the final coalesced drag position.
                            if flush_pending_pointer_drag(
                                &mut pending_drag,
                                |pending_mouse, pending_col, pending_row| {
                                    editor.handle_tiled_mouse_precise(
                                        pending_mouse,
                                        pending_col,
                                        pending_row,
                                        0,
                                    );
                                },
                            ) {
                                backend.set_widget_cursor(editor.widget_cursor());
                            }
                            pointer_released_this_loop = true;
                            pointer_is_down = false;
                        }
                        editor.handle_tiled_mouse_precise(mouse, precise_col, precise_row, 0);
                        backend.set_widget_cursor(editor.widget_cursor());
                    }
                }
                BackendEvent::Terminal(Event::Resize(_, _)) => editor.mark_needs_redraw(),
                _ => {}
            }
            ui_loop_stats.note_event(event_started.elapsed());
        }

        // Touchpad gestures
        let gestures_started = Instant::now();
        while let Some((delta, (precise_col, precise_row))) = backend.take_pending_magnify() {
            editor.handle_tiled_touchpad_magnify(precise_col, precise_row, 0, delta);
        }
        while let Some(((delta_x, delta_y), (precise_col, precise_row))) =
            backend.take_pending_scroll()
        {
            let widget_handled =
                editor.handle_tiled_touchpad_scroll(precise_col, precise_row, 0, delta_x, delta_y);
            if widget_handled {
                continue;
            }

            // In UI mode, apply pixel deltas directly for smooth sub-cell scrolling.
            if editor.is_ui_scroll_mode() {
                let scroll_speed = 0.05; // cells per pixel-delta
                let delta_cells_y = delta_y * scroll_speed;
                let delta_cells_x = delta_x * scroll_speed;
                editor.apply_smooth_widget_scroll(delta_cells_x, delta_cells_y);
                continue;
            }

            scroll_accum_y += delta_y;
            let line_px = backend.viewport_size().1.max(1) as f32 / (rows.max(1) as f32);
            let threshold = line_px.max(20.0);
            while scroll_accum_y > threshold {
                scroll_accum_y -= threshold;
                let mouse = crossterm::event::MouseEvent {
                    kind: crossterm::event::MouseEventKind::ScrollUp,
                    column: precise_col as u16,
                    row: precise_row as u16,
                    modifiers: crossterm::event::KeyModifiers::NONE,
                };
                editor.handle_tiled_mouse_precise(mouse, precise_col, precise_row, 0);
            }
            while scroll_accum_y < -threshold {
                scroll_accum_y += threshold;
                let mouse = crossterm::event::MouseEvent {
                    kind: crossterm::event::MouseEventKind::ScrollDown,
                    column: precise_col as u16,
                    row: precise_row as u16,
                    modifiers: crossterm::event::KeyModifiers::NONE,
                };
                editor.handle_tiled_mouse_precise(mouse, precise_col, precise_row, 0);
            }
            scroll_accum_x += delta_x;
            while scroll_accum_x > threshold {
                scroll_accum_x -= threshold;
                let mouse = crossterm::event::MouseEvent {
                    kind: crossterm::event::MouseEventKind::ScrollLeft,
                    column: precise_col as u16,
                    row: precise_row as u16,
                    modifiers: crossterm::event::KeyModifiers::NONE,
                };
                editor.handle_tiled_mouse_precise(mouse, precise_col, precise_row, 0);
            }
            while scroll_accum_x < -threshold {
                scroll_accum_x += threshold;
                let mouse = crossterm::event::MouseEvent {
                    kind: crossterm::event::MouseEventKind::ScrollRight,
                    column: precise_col as u16,
                    row: precise_row as u16,
                    modifiers: crossterm::event::KeyModifiers::NONE,
                };
                editor.handle_tiled_mouse_precise(mouse, precise_col, precise_row, 0);
            }
        }

        // Flush the latest coalesced drag every loop iteration. Waiting for the
        // render boundary makes slider/knob drags feel stale and can drop the
        // final motion segment if mouse-up lands before the next frame.
        if flush_pending_pointer_drag(&mut pending_drag, |mouse, precise_col, precise_row| {
            editor.handle_tiled_mouse_precise(mouse, precise_col, precise_row, 0);
        }) {
            backend.set_widget_cursor(editor.widget_cursor());
        }
        ui_loop_stats.note_gestures(gestures_started.elapsed());

        if !app.has_pending_project_load() {
            pull_named_scratch_buffer_into_project(&editor, &mut app);
        }

        // 1b. Drain host commands (sample browser etc.)
        let host_commands_started = Instant::now();
        let drained_host_commands = editor.drain_host_commands();
        for command in drained_host_commands {
            match command {
                HostCommand::AuthoringTransactionBegin { id, label } => {
                    sessions
                        .pending_lisp_history_transactions
                        .insert(id, (label, app.history.clone(), app.history.undo_len()));
                    continue;
                }
                HostCommand::AuthoringTransactionEnd { id, success } => {
                    if let Some((label, checkpoint, checkpoint_len)) =
                        sessions.pending_lisp_history_transactions.remove(&id)
                    {
                        if success {
                            app::edit::squash_history_since(&mut app, checkpoint_len, label);
                        } else if let Err(error) =
                            app::edit::rollback_history_to(&mut app, checkpoint)
                        {
                            editor.handle_host_event(HostEvent::Error(format!(
                                "Lisp authoring rollback failed: {error:?}"
                            )));
                        }
                    }
                    continue;
                }
                HostCommand::Custom { name, payload } => {
                    let _ = current_track_for_app(&mut app, &shared.current_track);
                    match handle_macro_host_command(
                        &name,
                        &payload,
                        &mut app,
                        &shared.state,
                        shared.current_track.load(Ordering::Relaxed),
                    ) {
                        MacroHostCommandOutcome::Applied => {
                            shared.ui_epoch.fetch_add(1, Ordering::Relaxed);
                            continue;
                        }
                        MacroHostCommandOutcome::Ignored => continue,
                        MacroHostCommandOutcome::NotMacro => {}
                    }
                    dispatch_custom_host_command(
                        &name,
                        payload,
                        &mut app,
                        &mut editor,
                        &mut LoopCtx {
                            sessions: &mut sessions,
                            meters: &mut meters,
                            frame: &mut frame,
                            gesture: &mut gesture,
                            track_names: &mut track_names,
                            shared: &shared,
                        },
                    );
                }
                HostCommand::CompileInstrument { .. } | HostCommand::CompileEffect { .. } => {}
            }
        }

        let mut project_load_still_pending = false;
        if app.has_pending_project_load() {
            if live_audio_analyzer.suspend_for_project_load() {
                editor.mark_needs_redraw();
            }
            let was_pending = true;
            match app.advance_pending_project_load() {
                Ok(()) => {
                    if app.has_pending_project_load() {
                        project_load_still_pending = true;
                    } else if was_pending {
                        if let Err(error) = clear_project_script_tabs(&mut editor) {
                            editor.handle_host_event(HostEvent::Status(error));
                        }
                        push_project_scratch_to_named_buffer(&mut editor, &app);
                        eprintln!(
                            "metal_seq: project load completed tracks={} current_project={:?}",
                            app.tracks.len(),
                            app.current_project_name
                        );
                        // Register all sampler track WAVs with eseqlisp for waveform display
                        for (t, path) in app.sampler_paths.iter().enumerate() {
                            if app.is_sampler_track(t) {
                                if let Some(p) = path {
                                    register_waveform_sample(p);
                                }
                            }
                        }
                        track_names = app.tracks.clone();
                        sync_shared_track_collapsed(&shared.track_collapsed, &app);
                        // `clear_project_arrangement_state` drops the region
                        // selection App-side; the clipboard lives out here and
                        // stores pattern/take IDS rather than content, so
                        // carrying it into another project would paste
                        // whatever now happens to hold those ids.
                        *shared.arrangement_clipboard.lock().unwrap() = None;
                        let restored_track = if app.tracks.is_empty() {
                            0
                        } else {
                            app.ui.cursor_track.min(app.tracks.len() - 1)
                        };
                        shared
                            .current_track
                            .store(restored_track, Ordering::Relaxed);
                        app.ui.cursor_track = restored_track;
                        {
                            let mut pan_ids = shared.track_pan_ids.lock().unwrap();
                            *pan_ids = app
                                .graph
                                .track_node_ids
                                .iter()
                                .map(|ids| ids.pan_id)
                                .collect();
                            push_solo_mutes(shared.lg_raw, &shared.state, &pan_ids);
                        }
                        *shared.bus_node_ids.lock().unwrap() = app.graph.bus_node_ids.clone();
                        *shared.record_armed.lock().unwrap() = vec![false; track_names.len()];
                        shared.recording.store(false, Ordering::Relaxed);
                        frame.recording_history_open = false;
                        // Keep the shared bus mirror in sync with the loaded buses,
                        // else pull_shared_bus_state clobbers app.buses (length
                        // mismatch) and drops the group's backing bus from the UI.
                        *shared.bus_state.lock().unwrap() = app.buses.clone();
                        // Push loaded groups into the shared runtime store; the
                        // per-frame groups diff rebuilds the SEQ.groups reactive.
                        *shared.track_groups.lock().unwrap() = app.groups.clone();
                        {
                            let mut sel = shared.selected_tracks.lock().unwrap();
                            sel.clear();
                            if !app.tracks.is_empty() {
                                sel.insert(restored_track);
                            }
                        }

                        let ct = shared.current_track.load(Ordering::Relaxed);
                        let playhead = if app.tracks.is_empty() {
                            0
                        } else {
                            shared.state.transport.track_playheads[ct].load(Ordering::Relaxed)
                        };
                        let transport_playhead =
                            shared.state.transport.playhead.load(Ordering::Relaxed);
                        let bpm = shared.state.transport.bpm.load(Ordering::Relaxed);
                        if meters.last_cpu_ui_poll_at.elapsed() >= CPU_UI_POLL_INTERVAL {
                            meters.cached_cpu_load_bits =
                                shared.state.transport.cpu_load_pct.load(Ordering::Relaxed);
                            meters.last_cpu_ui_poll_at = Instant::now();
                        }
                        let cpu_load_pct = f32::from_bits(meters.cached_cpu_load_bits);
                        let playing = shared.state.transport.playing.load(Ordering::Relaxed);
                        let epoch = shared.state.transport.pattern_epoch.load(Ordering::Relaxed);
                        meters.cached_peak_l_level = meter_display_level(f32::from_bits(
                            shared.state.transport.peak_l.load(Ordering::Relaxed),
                        ));
                        meters.cached_peak_r_level = meter_display_level(f32::from_bits(
                            shared.state.transport.peak_r.load(Ordering::Relaxed),
                        ));
                        meters.cached_track_peak_levels = read_track_peak_levels(
                            app.graph.lg,
                            &shared.track_pan_ids.lock().unwrap(),
                        );
                        meters.cached_bus_peak_levels =
                            read_bus_peak_levels(app.graph.lg, &app.graph.bus_node_ids);
                        (
                            meters.cached_modulator_phases,
                            meters.cached_modulator_levels,
                        ) = read_modulator_display_values(app.graph.lg, &app);
                        meters.last_meter_poll_at = Instant::now();
                        let rt = editor.runtime_mut();

                        sync_pattern_state(rt, &shared.state);
                        sync_project_state(rt, &app);
                        // Rebuild bus reactive (incl. SEQ.bus-ids) and groups so the
                        // loaded group headers can resolve their backing bus index.
                        sync_bus_mixer_state(rt, &app);
                        sync_groups_bindings(rt, &app.groups);
                        rt.set_reactive("SEQ", "playing", Value::Bool(playing));
                        rt.set_reactive("SEQ", "bpm", Value::Number(bpm as f64));
                        rt.set_reactive(
                            "SEQ",
                            "transport-playhead",
                            Value::Number(transport_playhead as f64),
                        );
                        rt.set_reactive("SEQ", "cpu-load-pct", Value::Number(cpu_load_pct as f64));
                        rt.set_reactive(
                            "SEQ",
                            "master-peak-l",
                            Value::Number(meters.cached_peak_l_level),
                        );
                        rt.set_reactive(
                            "SEQ",
                            "master-peak-r",
                            Value::Number(meters.cached_peak_r_level),
                        );
                        rt.set_reactive(
                            "SEQ",
                            "master-recording",
                            Value::Bool(shared.master_recording.load(Ordering::Acquire)),
                        );
                        sync_bus_peak_fields(rt, &meters.cached_bus_peak_levels);
                        sync_modulator_phase_fields(rt, &meters.cached_modulator_phases);
                        sync_modulator_level_fields(rt, &meters.cached_modulator_levels);
                        rt.set_reactive(
                            "SEQ",
                            "num-tracks",
                            Value::Number(track_names.len() as f64),
                        );
                        set_current_track_reactive(rt, app.tracks.len(), ct);
                        rt.set_reactive("SEQ", "track-ids", build_track_ids(&app));
                        rt.set_reactive("SEQ", "track-names", build_track_names(&track_names));
                        rt.set_reactive(
                            "SEQ",
                            "record-armed",
                            build_record_armed_value(&shared.record_armed.lock().unwrap()),
                        );
                        rt.set_reactive(
                            "SEQ",
                            "selected-steps",
                            build_selection_value(&shared.selected_steps),
                        );

                        if app.tracks.is_empty() {
                            sync_playhead_fields(rt, 0, 1);
                            rt.set_reactive("SEQ", "transport-playhead", Value::Number(0.0));
                            rt.set_reactive("SEQ", "steps", Value::List(vec![]));
                            rt.set_reactive("SEQ", "velocities", Value::List(vec![]));
                            rt.set_reactive("SEQ", "durations", Value::List(vec![]));
                            rt.set_reactive("SEQ", "transposes", Value::List(vec![]));
                            rt.set_reactive("SEQ", "pans", Value::List(vec![]));
                            rt.set_reactive("SEQ", "syncs", Value::List(vec![]));
                            rt.set_reactive("SEQ", "delays", Value::List(vec![]));
                            sync_track_mixer_empty_state(rt);
                            rt.set_reactive("SEQ", "effects", Value::List(vec![]));
                            rt.set_reactive("SEQ", "midi-effects", Value::List(vec![]));
                            rt.set_reactive("SEQ", "instrument-panel", Value::List(vec![]));
                            rt.set_reactive("SEQ", "step-has-plocks", Value::List(vec![]));
                            rt.set_reactive("SEQ", "track-steps", Value::List(vec![]));
                            rt.set_reactive("SEQ", "track-num-steps", Value::List(vec![]));
                            rt.set_reactive("SEQ", "track-duration-spans", Value::List(vec![]));
                            rt.set_reactive("SEQ", "track-playheads", Value::List(vec![]));
                            rt.set_reactive("SEQ", "track-step-has-plocks", Value::List(vec![]));
                        } else {
                            sync_all_track_sequencer_state(
                                rt,
                                &shared.state,
                                &app,
                                ct,
                                &shared.selected_steps,
                            );
                            sync_playhead_fields(
                                rt,
                                playhead as usize,
                                shared.state.pattern.track_params[ct].get_num_steps(),
                            );
                            rt.set_reactive(
                                "SEQ",
                                "transport-playhead",
                                Value::Number(transport_playhead as f64),
                            );
                            rt.set_reactive("SEQ", "steps", build_steps_value(&shared.state, ct));
                            sync_step_param_lists(rt, &shared.state, ct);
                            sync_track_mixer_state(rt, &app, &shared.state);
                            sync_track_peak_fields(rt, &meters.cached_track_peak_levels);
                            sync_bus_peak_fields(rt, &meters.cached_bus_peak_levels);
                            rt.set_reactive(
                                "SEQ",
                                "effects",
                                build_effects_value(
                                    &shared.state,
                                    ct,
                                    &app.graph.effect_descriptors,
                                    &shared.selected_steps,
                                ),
                            );
                            rt.set_reactive(
                                "SEQ",
                                "midi-effects",
                                build_midi_effects_value(&shared.state, ct, &shared.selected_steps),
                            );
                            rt.set_reactive(
                                "SEQ",
                                "instrument-panel",
                                build_instrument_panel_value(&app, ct, &shared.selected_steps),
                            );
                            *shared.accumulator_names.lock().unwrap() =
                                build_accumulator_names(&app);
                            let selected_neural_snapshot =
                                shared.selected_neural_neurons.lock().unwrap().clone();
                            sync_track_params_with_neural_selection(
                                rt,
                                &app,
                                &shared.state,
                                ct,
                                &shared.selected_steps,
                                Some(&selected_neural_snapshot),
                            );
                            sync_fx_param_binding_fields_with_neural_selection(
                                rt,
                                &app,
                                &shared.state,
                                ct,
                                &shared.selected_steps,
                                Some(&selected_neural_snapshot),
                            );
                            rt.set_reactive(
                                "SEQ",
                                "step-has-plocks",
                                build_step_has_plocks(
                                    &shared.state,
                                    ct,
                                    &app.graph.effect_descriptors,
                                ),
                            );
                            sync_sidebar_browser(rt, &app, ct);
                        }

                        rt.clear_subtree_effects_for_named_target("*sequencer*");
                        rt.run_reactive_cycle();
                        editor.refresh_runtime_side_effects();
                        editor.refresh_visible_layouts_for_buffer_named("*sequencer*");
                        if let Err(error) =
                            evaluate_project_scratch_on_ui_runtime(&mut editor, &app)
                        {
                            editor.handle_host_event(HostEvent::Status(format!(
                                "Scratch UI eval error: {error}"
                            )));
                        }
                        shared.ui_invalidations.clear();
                        shared.expanded_step_projection.clear();

                        frame.prev_current_track = ct;
                        frame.prev_playhead = playhead;
                        frame.prev_transport_playhead = transport_playhead;
                        frame.prev_bpm = bpm;
                        frame.prev_playing = playing;
                        frame.prev_pattern_epoch = epoch;
                        frame.prev_cpu_load_bits = meters.cached_cpu_load_bits;
                        frame.prev_peak_l_level = meters.cached_peak_l_level;
                        frame.prev_peak_r_level = meters.cached_peak_r_level;
                        frame.prev_master_recording =
                            shared.master_recording.load(Ordering::Acquire);
                        frame.prev_track_peak_levels = meters.cached_track_peak_levels.clone();
                        frame.prev_modulator_phases = meters.cached_modulator_phases.clone();
                        frame.prev_modulator_levels = meters.cached_modulator_levels.clone();
                        frame.prev_bus_playheads = bus_playhead_snapshot(&app);
                        frame.prev_track_playheads = track_playheads_snapshot(&shared.state, &app);
                        frame.prev_track_button_states = track_button_state_snapshot(&shared.state);
                        frame.prev_ui_epoch = shared.ui_epoch.load(Ordering::Relaxed);

                        if let Some((status, _)) = app.editor.status_message.take() {
                            eprintln!("metal_seq: project load status={status}");
                            editor.handle_host_event(HostEvent::Status(status));
                        }
                    }
                }
                Err(error) => {
                    eprintln!("metal_seq: project load advance failed error={error}");
                    editor.handle_host_event(HostEvent::Status(format!(
                        "Error loading project: {error}"
                    )));
                }
            }
        }
        if pointer_released_this_loop && gesture.rack_control_snapshot_dirty {
            shared.state.publish_scheduler_snapshot();
            gesture.rack_control_snapshot_dirty = false;
        }
        if pointer_released_this_loop {
            app::edit::finish_active_gesture(&mut app);
        } else if !pointer_is_down {
            app::edit::finish_active_gesture_if_idle(&mut app);
        }
        ui_loop_stats.note_host_commands(host_commands_started.elapsed());

        if let Some(completed_load) =
            sessions
                .pending_saved_instrument_load
                .as_ref()
                .and_then(|pending| match pending.receiver.try_recv() {
                    Ok(result) => Some(result),
                    Err(std::sync::mpsc::TryRecvError::Empty) => None,
                    Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                        Some(Err("Instrument load compile thread crashed".to_string()))
                    }
                })
        {
            let pending = sessions
                .pending_saved_instrument_load
                .take()
                .expect("completed saved instrument load must have pending state");
            let _ = editor
                .runtime_mut()
                .eval_str("(set! sbrowser-loading-instrument-name \"\")");
            match completed_load {
                Ok(result) => match apply_compiled_saved_instrument(
                    &mut app,
                    pending.target,
                    &pending.name,
                    &pending.source,
                    pending.run_mode,
                    result,
                ) {
                    Ok(SavedInstrumentLoadApply::Added { track, group_id }) => {
                        finish_added_instrument_track(
                            track,
                            AddTrackInstrumentCtx {
                                app: &mut app,
                                editor: &mut editor,
                                state: &shared.state,
                                current_track: &shared.current_track,
                                track_names: &mut track_names,
                                track_pan_ids: &shared.track_pan_ids,
                                record_armed: &shared.record_armed,
                                selected_steps: &shared.selected_steps,
                                accumulator_names: &shared.accumulator_names,
                                cached_track_peak_levels: &meters.cached_track_peak_levels,
                                group_id,
                                track_groups: &shared.track_groups,
                                ui_epoch: &shared.ui_epoch,
                                lg_raw: shared.lg_raw,
                            },
                        );
                        editor.mark_needs_redraw();
                    }
                    Ok(SavedInstrumentLoadApply::Swapped { summary }) => {
                        finish_swapped_instrument_track(
                            &pending.name,
                            summary,
                            SwapTrackInstrumentCtx {
                                app: &mut app,
                                editor: &mut editor,
                                state: &shared.state,
                                current_track: &shared.current_track,
                                track_names: &mut track_names,
                                selected_steps: &shared.selected_steps,
                                fx_epoch: &shared.fx_epoch,
                                ui_epoch: &shared.ui_epoch,
                            },
                        );
                        editor.mark_needs_redraw();
                    }
                    Err(error) => {
                        let action = match pending.target {
                            SavedInstrumentLoadTarget::AddTrack { .. } => "adding instrument track",
                            SavedInstrumentLoadTarget::SwapTrack { .. } => "swapping instrument",
                        };
                        editor.handle_host_event(HostEvent::Status(format!(
                            "Error {action}: {error}"
                        )));
                        editor.mark_needs_redraw();
                    }
                },
                Err(error) => {
                    let action = match pending.target {
                        SavedInstrumentLoadTarget::AddTrack { .. } => "loading instrument",
                        SavedInstrumentLoadTarget::SwapTrack { .. } => {
                            "loading instrument for swap"
                        }
                    };
                    editor.handle_host_event(HostEvent::Status(format!("Error {action}: {error}")));
                    editor.mark_needs_redraw();
                }
            }
        }

        if let Some(completed_cancel_restore) = sessions
            .pending_instrument_cancel_restore
            .as_ref()
            .and_then(|pending| match pending.receiver.try_recv() {
                Ok(result) => Some(result),
                Err(std::sync::mpsc::TryRecvError::Empty) => None,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    Some(Err("Instrument restore compile thread crashed".to_string()))
                }
            })
        {
            let pending = sessions
                .pending_instrument_cancel_restore
                .take()
                .expect("completed cancel restore must have pending state");
            let session = pending.session;
            match completed_cancel_restore {
                Ok(result) => match app.apply_compiled_instrument_engine(
                    session.engine_id,
                    &session.name,
                    &pending.persisted_source,
                    result,
                ) {
                    Ok(()) => {
                        // A fork-cancel discards its draft only now that the
                        // original is audible again; a failed restore below
                        // hands the draft session back intact.
                        if let Some(draft_dir) = pending.fork_draft_dir.as_deref() {
                            let _ = std::fs::remove_dir_all(draft_dir);
                        }
                        if let Some(buf_name) = sessions.editor_buffer_name.take() {
                            if let Err(error) = editor
                                .runtime_mut()
                                .eval_str(restore_instrument_patcher_layout_source())
                            {
                                editor.handle_host_event(HostEvent::Error(format!(
                                    "Failed to restore main editor layout: {error:?}"
                                )));
                            }
                            editor.refresh_runtime_side_effects();
                            editor.remove_buffer_by_name(&buf_name);
                        }
                        sessions.editor_mode = None;
                        let rt = editor.runtime_mut();
                        rt.set_reactive("SEQ", "editor-active", Value::Bool(false));
                        rt.set_reactive("SEQ", "editor-canceling", Value::Bool(false));
                        rt.set_reactive("SEQ", "editor-mode", Value::String(String::new()));
                        rt.set_reactive("SEQ", "editor-error", Value::String(String::new()));
                        rt.set_reactive("SEQ", "editor-buffer-name", Value::String(String::new()));
                        rt.set_reactive(
                            "SEQ",
                            "instrument-panel",
                            build_instrument_panel_value(
                                &app,
                                shared.current_track.load(Ordering::Relaxed),
                                &shared.selected_steps,
                            ),
                        );
                        rt.run_reactive_cycle();
                        editor.refresh_runtime_side_effects();
                        shared.ui_epoch.fetch_add(1, Ordering::Relaxed);
                        editor.handle_host_event(HostEvent::Status("Editor cancelled".to_string()));
                        editor.mark_needs_redraw();
                    }
                    Err(error) => {
                        sessions.instrument_edit_session = Some(session);
                        let rt = editor.runtime_mut();
                        rt.set_reactive("SEQ", "editor-canceling", Value::Bool(false));
                        rt.set_reactive(
                            "SEQ",
                            "editor-error",
                            Value::String(format!("Failed to restore instrument: {error}")),
                        );
                        rt.run_reactive_cycle();
                        editor.refresh_runtime_side_effects();
                        editor.mark_needs_redraw();
                    }
                },
                Err(error) => {
                    sessions.instrument_edit_session = Some(session);
                    let rt = editor.runtime_mut();
                    rt.set_reactive("SEQ", "editor-canceling", Value::Bool(false));
                    rt.set_reactive(
                        "SEQ",
                        "editor-error",
                        Value::String(format!("Failed to restore instrument: {error}")),
                    );
                    rt.run_reactive_cycle();
                    editor.refresh_runtime_side_effects();
                    editor.mark_needs_redraw();
                }
            }
        }

        if let Some(completed_cancel_restore) = sessions
            .pending_effect_cancel_restore
            .as_ref()
            .and_then(|pending| match pending.receiver.try_recv() {
                Ok(result) => Some(result),
                Err(std::sync::mpsc::TryRecvError::Empty) => None,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    Some(Err("Effect restore compile thread crashed".to_string()))
                }
            })
        {
            let pending = sessions
                .pending_effect_cancel_restore
                .take()
                .expect("completed effect cancel restore must have pending state");
            let session = pending.session;
            match completed_cancel_restore {
                Ok(result) => {
                    match apply_compiled_effect_edit_session(
                        &mut app,
                        &session,
                        &session.name,
                        result,
                    ) {
                        Ok(()) => {
                            // See the instrument restore above: the forked
                            // draft dir outlives a failed restore.
                            if let Some(draft_dir) = pending.fork_draft_dir.as_deref() {
                                let _ = std::fs::remove_dir_all(draft_dir);
                            }
                            if let Some(buf_name) = sessions.editor_buffer_name.take() {
                                if let Err(error) = editor
                                    .runtime_mut()
                                    .eval_str(restore_instrument_patcher_layout_source())
                                {
                                    editor.handle_host_event(HostEvent::Error(format!(
                                        "Failed to restore main editor layout: {error:?}"
                                    )));
                                }
                                editor.refresh_runtime_side_effects();
                                editor.remove_buffer_by_name(&buf_name);
                            }
                            sessions.editor_mode = None;
                            let rt = editor.runtime_mut();
                            rt.set_reactive("SEQ", "editor-active", Value::Bool(false));
                            rt.set_reactive("SEQ", "editor-canceling", Value::Bool(false));
                            rt.set_reactive("SEQ", "editor-mode", Value::String(String::new()));
                            rt.set_reactive("SEQ", "editor-error", Value::String(String::new()));
                            rt.set_reactive(
                                "SEQ",
                                "editor-buffer-name",
                                Value::String(String::new()),
                            );
                            match session.target {
                                EffectEditTarget::Track { track, .. } => {
                                    rt.set_reactive(
                                        "SEQ",
                                        "effects",
                                        build_effects_value(
                                            &shared.state,
                                            track,
                                            &app.graph.effect_descriptors,
                                            &shared.selected_steps,
                                        ),
                                    );
                                }
                                EffectEditTarget::Bus { .. } => {
                                    *shared.bus_state.lock().unwrap() = app.buses.clone();
                                    sync_bus_mixer_state(rt, &app);
                                    rt.set_reactive(
                                        "SEQ",
                                        "bus-effects",
                                        build_bus_effects_value_for_selection(
                                            &app,
                                            Some(&shared.selected_steps),
                                        ),
                                    );
                                }
                            }
                            rt.run_reactive_cycle();
                            editor.refresh_runtime_side_effects();
                            editor.refresh_visible_layouts_for_buffer_named("*fx*");
                            shared.fx_epoch.fetch_add(1, Ordering::Relaxed);
                            shared.ui_epoch.fetch_add(1, Ordering::Relaxed);
                            editor.handle_host_event(HostEvent::Status(
                                "Editor cancelled".to_string(),
                            ));
                            editor.mark_needs_redraw();
                        }
                        Err(error) => {
                            sessions.effect_edit_session = Some(session);
                            let rt = editor.runtime_mut();
                            rt.set_reactive("SEQ", "editor-canceling", Value::Bool(false));
                            rt.set_reactive(
                                "SEQ",
                                "editor-error",
                                Value::String(format!("Failed to restore effect: {error}")),
                            );
                            rt.run_reactive_cycle();
                            editor.refresh_runtime_side_effects();
                            editor.mark_needs_redraw();
                        }
                    }
                }
                Err(error) => {
                    sessions.effect_edit_session = Some(session);
                    let rt = editor.runtime_mut();
                    rt.set_reactive("SEQ", "editor-canceling", Value::Bool(false));
                    rt.set_reactive(
                        "SEQ",
                        "editor-error",
                        Value::String(format!("Failed to restore effect: {error}")),
                    );
                    rt.run_reactive_cycle();
                    editor.refresh_runtime_side_effects();
                    editor.mark_needs_redraw();
                }
            }
        }

        if let Some(completed_preview) =
            sessions
                .pending_instrument_preview
                .as_ref()
                .and_then(|pending| match pending.receiver.try_recv() {
                    Ok(result) => Some(Ok((
                        pending.generation,
                        pending.source.clone(),
                        pending.layout.clone(),
                        result,
                    ))),
                    Err(std::sync::mpsc::TryRecvError::Empty) => None,
                    Err(std::sync::mpsc::TryRecvError::Disconnected) => Some(Err(())),
                })
        {
            let _ = sessions.pending_instrument_preview.take();
            let mut replan_after_preview = false;
            match completed_preview {
                Ok((generation, source, layout, compile_result)) => {
                    if let Some(session) = sessions.instrument_edit_session.as_mut() {
                        if session.preview_generation == generation {
                            match compile_result {
                                Ok(result) => match app.apply_compiled_instrument_engine(
                                    session.engine_id,
                                    &session.name,
                                    &source,
                                    result,
                                ) {
                                    Ok(()) => {
                                        session.last_valid_source = source;
                                        session.last_valid_layout = layout;
                                        session.visible_revision_valid = true;
                                        replan_after_preview = session.learn_target_path.is_some();
                                        let rt = editor.runtime_mut();
                                        rt.set_reactive(
                                            "SEQ",
                                            "editor-error",
                                            Value::String(String::new()),
                                        );
                                        rt.set_reactive(
                                            "SEQ",
                                            "instrument-panel",
                                            build_instrument_panel_value(
                                                &app,
                                                shared.current_track.load(Ordering::Relaxed),
                                                &shared.selected_steps,
                                            ),
                                        );
                                        rt.run_reactive_cycle();
                                        editor.refresh_runtime_side_effects();
                                        shared.ui_epoch.fetch_add(1, Ordering::Relaxed);
                                        editor.handle_host_event(HostEvent::Status(format!(
                                            "Previewed instrument '{}'",
                                            session.name
                                        )));
                                    }
                                    Err(error) => {
                                        session.visible_revision_valid = false;
                                        let rt = editor.runtime_mut();
                                        rt.set_reactive(
                                            "SEQ",
                                            "editor-error",
                                            Value::String(error),
                                        );
                                        rt.run_reactive_cycle();
                                        editor.refresh_runtime_side_effects();
                                    }
                                },
                                Err(error) => {
                                    session.visible_revision_valid = false;
                                    let rt = editor.runtime_mut();
                                    rt.set_reactive("SEQ", "editor-error", Value::String(error));
                                    rt.run_reactive_cycle();
                                    editor.refresh_runtime_side_effects();
                                }
                            }
                        }
                    }
                }
                Err(()) => {
                    if let Some(session) = sessions.instrument_edit_session.as_mut() {
                        session.visible_revision_valid = false;
                        let rt = editor.runtime_mut();
                        rt.set_reactive(
                            "SEQ",
                            "editor-error",
                            Value::String("Instrument preview compile thread crashed".to_string()),
                        );
                        rt.run_reactive_cycle();
                        editor.refresh_runtime_side_effects();
                    }
                }
            }
            if replan_after_preview {
                if let Some(session) = sessions.instrument_edit_session.as_ref() {
                    match launch_learn_job(
                        &app,
                        session,
                        LearnLaunchKind::Plan,
                        sequencer::learn_job::LearnTrainingConfig::default(),
                        None,
                        None,
                    ) {
                        Ok(job) => {
                            replace_learn_job(&mut sessions.pending_learn_job, job);
                            editor.runtime_mut().set_reactive(
                                "SEQ",
                                "learn-phase",
                                Value::String("planning".to_string()),
                            );
                            editor.runtime_mut().run_reactive_cycle();
                            editor.refresh_runtime_side_effects();
                        }
                        Err(error) => {
                            let rt = editor.runtime_mut();
                            rt.set_reactive(
                                "SEQ",
                                "learn-phase",
                                Value::String("error".to_string()),
                            );
                            rt.set_reactive("SEQ", "learn-error", Value::String(error));
                            rt.run_reactive_cycle();
                            editor.refresh_runtime_side_effects();
                        }
                    }
                }
            }
        }

        if let Some(completed_preview) =
            sessions
                .pending_effect_preview
                .as_ref()
                .and_then(|pending| match pending.receiver.try_recv() {
                    Ok(result) => Some(Ok((
                        pending.generation,
                        pending.source.clone(),
                        pending.layout.clone(),
                        result,
                    ))),
                    Err(std::sync::mpsc::TryRecvError::Empty) => None,
                    Err(std::sync::mpsc::TryRecvError::Disconnected) => Some(Err(())),
                })
        {
            let _ = sessions.pending_effect_preview.take();
            match completed_preview {
                Ok((generation, source, layout, compile_result)) => {
                    if let Some(session) = sessions.effect_edit_session.as_mut() {
                        if session.preview_generation == generation {
                            match compile_result {
                                Ok(result) => {
                                    let name = session.name.clone();
                                    match apply_compiled_effect_edit_session(
                                        &mut app, session, &name, result,
                                    ) {
                                        Ok(()) => {
                                            session.last_valid_source = source;
                                            session.last_valid_layout = layout;
                                            session.visible_revision_valid = true;
                                            let rt = editor.runtime_mut();
                                            rt.set_reactive(
                                                "SEQ",
                                                "editor-error",
                                                Value::String(String::new()),
                                            );
                                            match session.target {
                                                EffectEditTarget::Track { track, .. } => {
                                                    rt.set_reactive(
                                                        "SEQ",
                                                        "effects",
                                                        build_effects_value(
                                                            &shared.state,
                                                            track,
                                                            &app.graph.effect_descriptors,
                                                            &shared.selected_steps,
                                                        ),
                                                    );
                                                }
                                                EffectEditTarget::Bus { .. } => {
                                                    *shared.bus_state.lock().unwrap() =
                                                        app.buses.clone();
                                                    sync_bus_mixer_state(rt, &app);
                                                    rt.set_reactive(
                                                        "SEQ",
                                                        "bus-effects",
                                                        build_bus_effects_value_for_selection(
                                                            &app,
                                                            Some(&shared.selected_steps),
                                                        ),
                                                    );
                                                }
                                            }
                                            rt.run_reactive_cycle();
                                            editor.refresh_runtime_side_effects();
                                            editor.refresh_visible_layouts_for_buffer_named("*fx*");
                                            shared.fx_epoch.fetch_add(1, Ordering::Relaxed);
                                            shared.ui_epoch.fetch_add(1, Ordering::Relaxed);
                                            editor.handle_host_event(HostEvent::Status(format!(
                                                "Previewed effect '{}'",
                                                session.name
                                            )));
                                        }
                                        Err(error) => {
                                            session.visible_revision_valid = false;
                                            let rt = editor.runtime_mut();
                                            rt.set_reactive(
                                                "SEQ",
                                                "editor-error",
                                                Value::String(error),
                                            );
                                            rt.run_reactive_cycle();
                                            editor.refresh_runtime_side_effects();
                                        }
                                    }
                                }
                                Err(error) => {
                                    session.visible_revision_valid = false;
                                    let rt = editor.runtime_mut();
                                    rt.set_reactive("SEQ", "editor-error", Value::String(error));
                                    rt.run_reactive_cycle();
                                    editor.refresh_runtime_side_effects();
                                }
                            }
                        }
                    }
                }
                Err(()) => {
                    if let Some(session) = sessions.effect_edit_session.as_mut() {
                        session.visible_revision_valid = false;
                        let rt = editor.runtime_mut();
                        rt.set_reactive(
                            "SEQ",
                            "editor-error",
                            Value::String("Effect preview compile thread crashed".to_string()),
                        );
                        rt.run_reactive_cycle();
                        editor.refresh_runtime_side_effects();
                    }
                }
            }
        }
        poll_learn_job(&mut app, &mut sessions, &mut editor);
        let mut completed_agentic = Vec::new();
        for (key, pending) in &sessions.pending_agentic_bubbles {
            match pending.receiver.try_recv() {
                Ok(result) => completed_agentic.push((key.clone(), result)),
                Err(std::sync::mpsc::TryRecvError::Empty) => {}
                Err(std::sync::mpsc::TryRecvError::Disconnected) => completed_agentic
                    .push((key.clone(), Err("request worker disconnected".to_string()))),
            }
        }
        for (key, result) in completed_agentic {
            let Some(pending) = sessions.pending_agentic_bubbles.remove(&key) else {
                continue;
            };
            match result {
                Ok(output) => match output {
                    sequencer::agent::agentic_bubble::AgenticBubbleOutput::Macro {
                        macro_name,
                        source,
                    } => match eseqlisp::widget_render::patcher::resolve_agentic_bubble(
                        &pending.path,
                        pending.intent,
                        &pending.bubble_id,
                        pending.generation,
                        &macro_name,
                        &source,
                    ) {
                        Ok(()) => {
                            eprintln!(
                                "[agentic-bubble] host materialized path={} bubble={} generation={} macro={}",
                                pending.path.display(),
                                pending.bubble_id,
                                pending.generation,
                                macro_name
                            );
                            editor.refresh_runtime_side_effects();
                            editor.mark_needs_redraw();
                            editor.handle_host_event(HostEvent::Status(format!(
                                "Generated macro '{}'",
                                macro_name
                            )));
                        }
                        Err(error) => {
                            eprintln!(
                                "[agentic-bubble] host materialize failed path={} bubble={} generation={} error={}",
                                pending.path.display(),
                                pending.bubble_id,
                                pending.generation,
                                error
                            );
                            eseqlisp::widget_render::patcher::fail_agentic_bubble(
                                &pending.path,
                                &pending.bubble_id,
                                pending.generation,
                                "materialize failed",
                                error.clone(),
                            );
                            editor.mark_needs_redraw();
                            editor.handle_host_event(HostEvent::Status(format!(
                                "Agentic bubble failed: {error}"
                            )));
                        }
                    },
                    sequencer::agent::agentic_bubble::AgenticBubbleOutput::MacroEdit { source } => {
                        let macro_name = extract_macro_name_from_defmacro(&source)
                            .unwrap_or_else(|| "macro".to_string());
                        match eseqlisp::widget_render::patcher::resolve_agentic_bubble_macro_edit(
                            &pending.path,
                            pending.intent,
                            &pending.bubble_id,
                            pending.generation,
                            &macro_name,
                            &source,
                        ) {
                            Ok(()) => {
                                // The edit landed in dsp.lisp without touching
                                // interaction state, so no writeback payload
                                // (and therefore no compile) is coming from the
                                // patcher — kick one here or the new macro is
                                // visible but silent until an unrelated edit.
                                match std::fs::read_to_string(&pending.path) {
                                    Ok(source) => match pending.intent {
                                        eseqlisp::widget_render::patcher::PatcherIntent::Instrument => {
                                            if let Some(session) =
                                                sessions.instrument_edit_session.as_mut()
                                            {
                                                edit_sessions::queue_instrument_preview_compile(
                                                    session,
                                                    &mut sessions.pending_instrument_preview,
                                                    source,
                                                    app.graph.sample_rate,
                                                );
                                            }
                                        }
                                        eseqlisp::widget_render::patcher::PatcherIntent::Effect => {
                                            if let Some(session) =
                                                sessions.effect_edit_session.as_mut()
                                            {
                                                edit_sessions::queue_effect_preview_compile(
                                                    session,
                                                    &mut sessions.pending_effect_preview,
                                                    source,
                                                    app.graph.sample_rate,
                                                );
                                            }
                                        }
                                    },
                                    Err(error) => eprintln!(
                                        "[agentic-bubble] macro edit recompile skipped path={} error={error}",
                                        pending.path.display()
                                    ),
                                }
                                editor.refresh_runtime_side_effects();
                                editor.mark_needs_redraw();
                                editor.handle_host_event(HostEvent::Status(format!(
                                    "Updated macro '{}'",
                                    macro_name
                                )));
                            }
                            Err(error) => {
                                eseqlisp::widget_render::patcher::fail_agentic_bubble(
                                    &pending.path,
                                    &pending.bubble_id,
                                    pending.generation,
                                    "materialize failed",
                                    error.clone(),
                                );
                                editor.mark_needs_redraw();
                                editor.handle_host_event(HostEvent::Status(format!(
                                    "Agentic bubble failed: {error}"
                                )));
                            }
                        }
                    }
                    sequencer::agent::agentic_bubble::AgenticBubbleOutput::Connections { ops } => {
                        match eseqlisp::widget_render::patcher::resolve_agentic_bubble_connections(
                            &pending.path,
                            pending.intent,
                            &pending.bubble_id,
                            pending.generation,
                            &ops,
                        ) {
                            Ok(report) => {
                                // The plan bypassed the widget's own event
                                // handling, so nothing has told the patch to
                                // recompile. Fire the same semantic change a
                                // mouse-drawn cable does, or the cables appear
                                // and the patch stays silent.
                                editor.notify_patcher_semantic_change(&pending.path);
                                // The applied cables and inlined values are text
                                // the patcher has never measured, and it only
                                // wraps text whose glyph widths a measure pass
                                // cached.
                                editor.runtime_mut().invalidate_layout_deferred();
                                editor.refresh_runtime_side_effects();
                                editor.mark_needs_redraw();
                                let mut status = report.applied.join("; ");
                                if !report.skipped.is_empty() {
                                    status.push_str(&format!(
                                        " (skipped: {})",
                                        report.skipped.join("; ")
                                    ));
                                }
                                editor.handle_host_event(HostEvent::Status(status));
                            }
                            Err(error) => {
                                eseqlisp::widget_render::patcher::fail_agentic_bubble(
                                    &pending.path,
                                    &pending.bubble_id,
                                    pending.generation,
                                    "connection plan rejected",
                                    error.clone(),
                                );
                                editor.mark_needs_redraw();
                                editor.handle_host_event(HostEvent::Status(format!(
                                    "Agentic bubble failed: {error}"
                                )));
                            }
                        }
                    }
                    sequencer::agent::agentic_bubble::AgenticBubbleOutput::Answer { text } => {
                        eseqlisp::widget_render::patcher::resolve_agentic_bubble_answer(
                            &pending.path,
                            &pending.bubble_id,
                            pending.generation,
                            text,
                        );
                        // The answer is text the patcher has never measured, and
                        // it only wraps text whose glyph widths are cached by a
                        // measure pass. Without a relayout the bubble draws
                        // nothing until some later interaction forces one.
                        editor.runtime_mut().invalidate_layout_deferred();
                        editor.refresh_runtime_side_effects();
                        editor.mark_needs_redraw();
                        editor.handle_host_event(HostEvent::Status(
                            "Agentic bubble answered".to_string(),
                        ));
                    }
                },
                Err(error) => {
                    eprintln!(
                        "[agentic-bubble] host generation failed path={} bubble={} generation={} error={}",
                        pending.path.display(),
                        pending.bubble_id,
                        pending.generation,
                        error
                    );
                    eseqlisp::widget_render::patcher::fail_agentic_bubble(
                        &pending.path,
                        &pending.bubble_id,
                        pending.generation,
                        "generation failed",
                        error.clone(),
                    );
                    editor.mark_needs_redraw();
                    editor.handle_host_event(HostEvent::Status(format!(
                        "Agentic bubble failed: {error}"
                    )));
                }
            }
        }
        if project_load_still_pending {
            continue;
        }

        match reactive_tick_and_render(
            &mut app,
            &mut editor,
            &mut backend,
            &mut LoopCtx {
                sessions: &mut sessions,
                meters: &mut meters,
                frame: &mut frame,
                gesture: &mut gesture,
                track_names: &mut track_names,
                shared: &shared,
            },
            TickInputs {
                cols,
                rows,
                viewport_size,
                stub_animation_active,
                frame_interval,
                sdf_animation_active,
                playing_now,
            },
            &mut last_render_at,
            &mut stub_animation_cache,
            &mut ui_loop_stats,
        )? {
            TickFlow::Quit => break,
            TickFlow::Continue => {}
        }
    }

    let _ = backend.teardown();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};

    #[test]
    fn pending_pointer_drag_is_dispatched_before_release_can_clear_it() {
        let drag = MouseEvent {
            kind: MouseEventKind::Drag(MouseButton::Left),
            column: 17,
            row: 4,
            modifiers: KeyModifiers::NONE,
        };
        let mut pending = Some((drag, (17.75, 4.25)));
        let mut dispatched = Vec::new();

        let flushed = flush_pending_pointer_drag(&mut pending, |mouse, col, row| {
            dispatched.push((mouse.kind, col, row));
        });

        assert!(flushed);
        assert!(pending.is_none());
        assert_eq!(
            dispatched,
            vec![(MouseEventKind::Drag(MouseButton::Left), 17.75, 4.25)]
        );
    }
}
