#![allow(
    dead_code,
    clippy::inspect_for_each,
    clippy::manual_clamp,
    clippy::needless_range_loop,
    clippy::too_many_arguments,
    clippy::type_complexity,
    clippy::useless_conversion,
    clippy::useless_format
)]

mod browser;
mod capture;
mod constants;
mod custom_ui;
mod editor_setup;
mod host_commands;
mod input;
mod lisp_hot_reload;
mod live_audio_analyzer;
mod natives;
mod piano_roll;
mod profile;
mod sample_import_ui;
mod sampler_monitor;
mod state_values;
mod ui_invalidation;
mod values;

use browser::*;
use constants::*;
use custom_ui::*;
use editor_setup::*;
use host_commands::*;
use input::*;
use lisp_hot_reload::*;
use live_audio_analyzer::*;
use natives::*;
use piano_roll::*;
use profile::*;
use sample_import_ui::*;
use sampler_monitor::*;
use state_values::*;
use ui_invalidation::*;
use values::*;

use std::cell::RefCell;
use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crossterm::event::Event;

use eseqlisp::backend::{Backend, BackendEvent};
use eseqlisp::editor::ViewMode;
use eseqlisp::parser::{ASTParser, Expression, Parser};
use eseqlisp::vm::Value;
use eseqlisp::{BufferMode, Editor, HostCommand, HostEvent, Runtime};

use sequencer::agent::actions::{
    AgentInstrumentParamSchema, AgentInstrumentPresetSchema, AgentSessionContext,
};
use sequencer::effects::{ParamKind, ParamScaling};
use sequencer::engine;
use sequencer::sequencer::{
    CustomInstrumentRunMode, InstrumentSlotResetSummary, InstrumentType, KeyboardTrigger,
    MidiFxPosition, PatternId, RackSlotParam, SequencerState, StepParam, SwingResolution, Timebase,
    TrackId, TrackOutput, TrackSendSnapshot, MAX_STEPS, SYNC_RESOLUTIONS,
};
use sequencer::app;
use std::sync::atomic::AtomicBool;

mod agent_finalize;
mod edit_sessions;
mod history_commands;
mod loop_ctx;
mod reactive_sync;

use agent_finalize::*;
use edit_sessions::*;
use history_commands::*;
use loop_ctx::*;
use reactive_sync::*;

#[cfg(test)]
mod tests;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let capture_args = capture::CaptureArgs::parse_env()
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidInput, error))?;
    sequencer::paths::enter_sequencer_dir()?;
    sequencer::crash::install()?;

    if let Some(args) = capture_args {
        return capture::run(args);
    }

    // 1. Init audio engine
    let eng = engine::init_engine()?;
    let lg_ptr = eng.lg_ptr;
    let state = eng.state.clone();
    let stream = eng._stream;

    // 2. Create App. Start intentionally empty so the first action is choosing
    // a sound instead of editing a canned pattern.
    let master_recorder = eng.master_recorder.clone();
    let mut app = app::App::new(
        eng.state.clone(),
        eng.lg_ptr,
        eng.sample_rate,
        eng.buses,
        eng.master_recorder,
        eng.keyboard_tx,
    );

    let mut track_names: Vec<String> = Vec::new();

    // Collect node IDs for param pushing to audiograph
    let track_pan_ids: Arc<Mutex<Vec<i32>>> = Arc::new(Mutex::new(
        app.graph.track_node_ids.iter().map(|n| n.pan_id).collect(),
    ));
    let track_collapsed: Arc<Mutex<Vec<bool>>> = Arc::new(Mutex::new(app.track_collapsed.clone()));
    let bus_state: Arc<Mutex<Vec<app::BusChannelState>>> = Arc::new(Mutex::new(app.buses.clone()));
    let bus_node_ids: Arc<Mutex<Vec<app::BusNodeIds>>> =
        Arc::new(Mutex::new(app.graph.bus_node_ids.clone()));
    let lg_raw = lg_ptr.0;

    // Shared current track index
    let current_track = Arc::new(AtomicUsize::new(0));
    // Multi-select set for mixer group operations (includes current_track when non-empty)
    let selected_tracks: Arc<Mutex<HashSet<usize>>> = Arc::new(Mutex::new(HashSet::new()));
    // Shared in-memory track groups (mirror of app.groups, mutated by natives)
    let track_groups: Arc<Mutex<Vec<sequencer::project::ProjectTrackGroup>>> =
        Arc::new(Mutex::new(app.groups.clone()));
    // Selected steps for p-locking
    let selected_steps: Arc<Mutex<HashSet<usize>>> = Arc::new(Mutex::new(HashSet::new()));
    let selected_neural_neurons: sequencer::lisp_host::SharedSelectedNeuralNeurons =
        Arc::new(Mutex::new(BTreeSet::new()));
    let piano_roll_selection: Arc<Mutex<HashSet<u64>>> = Arc::new(Mutex::new(HashSet::new()));
    let piano_roll_move_state: Arc<Mutex<Option<PianoRollMoveState>>> = Arc::new(Mutex::new(None));
    let step_clipboard: Arc<
        Mutex<Option<(usize, Vec<(usize, sequencer::sequencer::StepSnapshot)>)>>,
    > = Arc::new(Mutex::new(None));
    // UI-only counter for changes that shouldn't affect pattern_epoch (e.g. volume, selection)
    let ui_epoch = Arc::new(AtomicUsize::new(0));
    // FX/instrument panel refresh counter for changes that affect *fx* but
    // should not force *fx* to rerun on unrelated step-grid edits.
    let fx_epoch = Arc::new(AtomicUsize::new(0));
    let ui_invalidations = Arc::new(UiInvalidationQueue::new());
    let expanded_step_projection = Arc::new(ExpandedStepProjectionRegistry::new());
    let active_delete_target: Arc<Mutex<Option<ActiveDeleteTarget>>> = Arc::new(Mutex::new(None));
    let active_delete_target_version = Arc::new(AtomicUsize::new(0));
    // When set, pagination stays on the user-selected page until the cooldown expires.
    let auto_follow_override_until: Arc<Mutex<Option<Instant>>> = Arc::new(Mutex::new(None));

    // Recording state shared between native functions and event loop
    let recording = Arc::new(AtomicBool::new(false));
    let master_recording = Arc::new(AtomicBool::new(false));
    let record_armed: Arc<Mutex<Vec<bool>>> = Arc::new(Mutex::new(vec![false; track_names.len()]));
    // Keyboard trigger sender for live playing when armed
    let keyboard_tx = app.graph.keyboard_tx.clone();
    // Keyboard octave offset for live playing
    let keyboard_octave = Arc::new(std::sync::atomic::AtomicI32::new(0));
    // Held keys for recording: (key_char, transpose, step_at_press, press_instant)
    let held_notes: Arc<Mutex<Vec<HeldKeyboardNote>>> = Arc::new(Mutex::new(Vec::new()));

    let RuntimeInit {
        runtime,
        accumulator_names,
        midi_fx_names: _,
        sample_browser,
        piano_roll_clipboard,
        selected_drum_lane_steps,
    } = init_runtime(
        &app,
        state.clone(),
        &track_names,
        track_pan_ids.clone(),
        track_collapsed.clone(),
        bus_state.clone(),
        bus_node_ids.clone(),
        current_track.clone(),
        selected_tracks.clone(),
        track_groups.clone(),
        selected_steps.clone(),
        piano_roll_selection.clone(),
        piano_roll_move_state.clone(),
        recording.clone(),
        master_recording.clone(),
        master_recorder.clone(),
        record_armed.clone(),
        ui_epoch.clone(),
        fx_epoch.clone(),
        ui_invalidations.clone(),
        expanded_step_projection.clone(),
        selected_neural_neurons.clone(),
        active_delete_target.clone(),
        active_delete_target_version.clone(),
        auto_follow_override_until.clone(),
        lg_raw,
    );

    let (mut editor, mut backend) = create_editor_and_backend(runtime, &app)?;

    // 5. Metal event loop
    let idle_frame_interval = Duration::from_secs_f64(1.0 / 30.0);
    let animation_frame_interval = Duration::from_secs_f64(1.0 / 60.0);
    let mut last_render_at = Instant::now() - idle_frame_interval;
    let mut stub_animation_cache = StubAnimationRenderCache::new();
    let mut pending_drag: Option<(Event, (f32, f32))> = None;
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
        pending_lisp_history_transactions: HashMap::new(),
    };
    let mut frame = FrameDiffState {
        prev_editor_macro_action: (String::new(), String::new()),
        prev_playing: false,
        prev_bpm: 0,
        prev_playhead: u32::MAX,
        prev_transport_playhead: u32::MAX,
        prev_pattern_epoch: 0,
        prev_current_track: usize::MAX,
        prev_cpu_load_bits: u32::MAX,
        prev_peak_l_level: -1.0f64,
        prev_peak_r_level: -1.0f64,
        prev_recording: false,
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
        prev_track_button_states: track_button_state_snapshot(&state),
        prev_current_track_playhead_visible: false,
        prev_ui_epoch: 0,
        prev_fx_epoch: 0,
        prev_instrument_active_notes: Vec::new(),
        prev_active_buffer_name: editor.active_buffer().name.clone(),
        prev_selected_neural_neurons: selected_neural_neurons.lock().unwrap().clone(),
        prev_agent_generation_watermark: agent_generation_watermark(&app),
        prev_sampler_analysis_key: None,
        prev_auto_follow: true,
        prev_queued_transport_scene: None,
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
        last_voice_count_log_at: Instant::now() - VOICE_COUNT_LOG_INTERVAL,
    };
    let mut live_audio_analyzer = LiveAudioAnalyzerManager::new(app.graph.lg);
    let log_voice_counts = std::env::var_os("TINYSEQ_LOG_VOICE_COUNTS").is_some();
    if log_voice_counts {
        sequencer::voice_modulator::set_process_stats_enabled(true);
    }

    // Cheaply clonable mirrors of the handles init_runtime captured, bundled
    // for the extracted host-command dispatcher.
    let shared = SharedHandles {
        state: state.clone(),
        lg_raw,
        current_track: current_track.clone(),
        selected_tracks: selected_tracks.clone(),
        selected_steps: selected_steps.clone(),
        selected_neural_neurons: selected_neural_neurons.clone(),
        piano_roll_selection: piano_roll_selection.clone(),
        piano_roll_move_state: piano_roll_move_state.clone(),
        step_clipboard: step_clipboard.clone(),
        ui_epoch: ui_epoch.clone(),
        fx_epoch: fx_epoch.clone(),
        ui_invalidations: ui_invalidations.clone(),
        expanded_step_projection: expanded_step_projection.clone(),
        active_delete_target: active_delete_target.clone(),
        active_delete_target_version: active_delete_target_version.clone(),
        auto_follow_override_until: auto_follow_override_until.clone(),
        track_pan_ids: track_pan_ids.clone(),
        track_collapsed: track_collapsed.clone(),
        bus_state: bus_state.clone(),
        bus_node_ids: bus_node_ids.clone(),
        track_groups: track_groups.clone(),
        record_armed: record_armed.clone(),
        keyboard_tx: keyboard_tx.clone(),
        accumulator_names: accumulator_names.clone(),
        piano_roll_clipboard: piano_roll_clipboard.clone(),
        selected_drum_lane_steps: selected_drum_lane_steps.clone(),
    };

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
        app.graph_controller().reap_due_rack_teardowns();
        let queued_transport_scene = state
            .quantized_launches()
            .pending_target(sequencer::quantized_launch::QuantizedLaunchOwner::Transport)
            .map(|target| match target {
                sequencer::quantized_launch::PatternLaunchTarget::Scene { scene }
                | sequencer::quantized_launch::PatternLaunchTarget::SceneTracks { scene, .. } => {
                    scene
                }
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
        let sample_browser_ready = { sample_browser.borrow_mut().poll_ready() };
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
                ui_epoch.fetch_add(1, Ordering::Relaxed);
            }
        }
        if backend.poll_editable_shader_overrides() {
            editor.mark_needs_redraw();
        }
        release_due_key_lock_auditions(
            &mut sessions.pending_key_lock_auditions,
            &keyboard_tx,
            Instant::now(),
        );
        pull_shared_bus_state(&mut app, &bus_state);
        let recording_now = recording.load(Ordering::Relaxed);
        if recording_now != frame.prev_recording {
            let result = if recording_now {
                app.begin_recording_take_history().map(|_| None)
            } else {
                app.finish_recording_take_history()
            };
            if let Err(error) = result {
                recording.store(false, Ordering::Relaxed);
                editor.handle_host_event(HostEvent::Error(format!(
                    "Recording history failed: {error}"
                )));
                frame.prev_recording = false;
            } else {
                frame.prev_recording = recording_now;
            }
        }
        if !app.has_pending_project_load() {
            pull_named_scratch_buffer_into_project(&editor, &mut app);
        }
        editor.update_timers();
        let active_buffer_name = editor.active_buffer().name.clone();
        if active_buffer_name != frame.prev_active_buffer_name {
            frame.prev_active_buffer_name = active_buffer_name;
            let mut guard = active_delete_target.lock().unwrap();
            let should_clear = should_clear_active_delete_target_for_buffer(
                guard.as_ref(),
                &frame.prev_active_buffer_name,
            );
            if should_clear {
                guard.take();
                drop(guard);
                active_delete_target_version.fetch_add(1, Ordering::Relaxed);
                ui_epoch.fetch_add(1, Ordering::Relaxed);
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
        if log_voice_counts && meters.last_voice_count_log_at.elapsed() >= VOICE_COUNT_LOG_INTERVAL {
            log_active_voice_counts(&state, &track_names);
            meters.last_voice_count_log_at = Instant::now();
        }

        let viewport_size = (cols, rows);
        let stub_animation_active = stub_animation_cache.is_active(
            viewport_size,
            backend.agent_instrument_stub_animation_visible(),
        );
        let widget_animation_active = editor.visible_widgets_animating();
        let frame_interval = if stub_animation_active || widget_animation_active {
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
        let playing_now = state.transport.playing.load(Ordering::Relaxed);
        let timeout = if playing_now {
            frame_interval
                .saturating_sub(last_render_at.elapsed())
                .max(Duration::from_millis(8))
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
                                Ok(true) => editor.show_transient_message("Parameter edit canceled"),
                                Ok(false) => {}
                                Err(error) => editor.handle_host_event(HostEvent::Error(format!(
                                    "Could not cancel parameter edit: {error:?}"
                                ))),
                            }
                            pending_drag = None;
                            pointer_is_down = false;
                            ui_epoch.fetch_add(1, Ordering::Relaxed);
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
                            *piano_roll_move_state.lock().unwrap() = None;
                            ui_invalidations.push(UiInvalidation::PianoRoll {
                                track,
                                change: PianoRollInvalidation::Items,
                            });
                            match finalized {
                                Ok(_) => {
                                    ui_epoch.fetch_add(1, Ordering::Relaxed);
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
                        if recording.load(Ordering::Relaxed) {
                            recording.store(false, Ordering::Relaxed);
                            frame.prev_recording = false;
                            app.ui.recording = false;
                        }
                        let track_count_before_replay = app.tracks.len();
                        let replay = match shortcut {
                            SequencerHistoryShortcut::Undo => app::edit::undo(&mut app),
                            SequencerHistoryShortcut::Redo => app::edit::redo(&mut app),
                        };
                        let message = match replay {
                            app::history::HistoryReplay::Applied(result) => {
                                let topology_changed = app.tracks.len() != track_count_before_replay;
                                if !topology_changed {
                                    track_names.clone_from(&app.tracks);
                                }
                                let replay_track = if topology_changed {
                                    app.ui.cursor_track
                                } else {
                                    current_track.load(Ordering::Relaxed)
                                }.min(app.tracks.len().saturating_sub(1));
                                current_track.store(replay_track, Ordering::Relaxed);
                                *bus_state.lock().unwrap() = app.buses.clone();
                                *bus_node_ids.lock().unwrap() = app.graph.bus_node_ids.clone();
                                *track_groups.lock().unwrap() = app.groups.clone();
                                if topology_changed {
                                    {
                                        let mut pan_ids = track_pan_ids.lock().unwrap();
                                        *pan_ids = app.graph.track_node_ids.iter()
                                            .map(|ids| ids.pan_id)
                                            .collect();
                                        push_solo_mutes(lg_raw, &state, &pan_ids);
                                    }
                                    meters.cached_track_peak_levels = read_track_peak_levels(
                                        app.graph.lg,
                                        &track_pan_ids.lock().unwrap(),
                                    );
                                    meters.cached_bus_peak_levels =
                                        read_bus_peak_levels(app.graph.lg, &app.graph.bus_node_ids);
                                    (meters.cached_modulator_phases, meters.cached_modulator_levels) =
                                        read_modulator_display_values(app.graph.lg, &app);
                                    meters.last_meter_poll_at = Instant::now();
                                    *record_armed.lock().unwrap() = app.graph.record_armed.clone();
                                    *track_groups.lock().unwrap() = app.groups.clone();
                                }
                                let rt = editor.runtime_mut();
                                if topology_changed {
                                    sync_track_topology_state(
                                        rt,
                                        &app,
                                        &state,
                                        &mut track_names,
                                        replay_track,
                                        &selected_steps,
                                        &piano_roll_selection,
                                        &accumulator_names,
                                        &record_armed,
                                        &meters.cached_track_peak_levels,
                                    );
                                    sync_bus_peak_fields(rt, &meters.cached_bus_peak_levels);
                                    sync_modulator_phase_fields(rt, &meters.cached_modulator_phases);
                                    sync_modulator_level_fields(rt, &meters.cached_modulator_levels);
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
                                            &selected_steps,
                                        ),
                                    );
                                }
                                rt.run_reactive_cycle();
                                editor.refresh_runtime_side_effects();
                                if topology_changed {
                                    refresh_visible_track_topology_layouts(&mut editor);
                                    frame.prev_track_playheads = track_playheads_snapshot(&state, &app);
                                    frame.prev_track_button_states = track_button_state_snapshot(&state);
                                }
                                if !app.buses.is_empty() {
                                    ui_invalidations.push(UiInvalidation::BusMixer {
                                        bus: 0,
                                        change: BusMixerInvalidation::Volume,
                                    });
                                }
                                ui_invalidations.push(UiInvalidation::Pattern(
                                    PatternInvalidation::AllTracks,
                                ));
                                fx_epoch.fetch_add(1, Ordering::Relaxed);
                                ui_epoch.fetch_add(1, Ordering::Relaxed);
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
                        &state,
                        &current_track,
                        &selected_steps,
                        &step_clipboard,
                        &ui_epoch,
                    ) {
                        if key_should_reveal_sequencer_track(&raw_key) {
                            let track = current_track.load(Ordering::Relaxed);
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
                            let mut selection = selected_neural_neurons.lock().unwrap();
                            let had_selection = !selection.is_empty();
                            selection.clear();
                            had_selection
                        };
                        if cleared_neural_selection {
                            let selection = selected_neural_neurons.lock().unwrap().clone();
                            sync_selected_neural_neuron_bindings(
                                editor.runtime_mut(),
                                &state,
                                &selection,
                            );
                            let track = current_track.load(Ordering::Relaxed);
                            sync_fx_param_binding_fields_with_neural_selection(
                                editor.runtime_mut(),
                                &app,
                                &state,
                                track,
                                &selected_steps,
                                Some(&selection),
                            );
                            sync_track_plocks_for_neural_selection(
                                editor.runtime_mut(),
                                &app,
                                &state,
                                track,
                                &selected_steps,
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
                        &current_track,
                        &expanded_step_projection,
                        &mut soft_step_param_edit,
                    ) {
                        ui_loop_stats.note_event(event_started.elapsed());
                        continue;
                    }
                    // Intercept keyboard for live recording when any track is armed
                    let any_armed = record_armed.lock().unwrap().iter().any(|a| *a);
                    let recording_key_outcome = if (any_armed
                        || held_note_for_key(&held_notes, &key))
                        && should_route_to_live_keyboard(&editor, &key, &held_notes)
                    {
                        handle_recording_key(
                            &key,
                            &state,
                            &record_armed,
                            &recording,
                            &keyboard_tx,
                            &keyboard_octave,
                            &held_notes,
                            &ui_epoch,
                        )
                    } else {
                        RecordingKeyOutcome::Ignored
                    };
                    let intercepted = recording_key_outcome.consumed();
                    if recording_key_outcome.recorded() {
                        app.mark_recording_take_changed();
                        let ct = current_track.load(Ordering::Relaxed);
                        let rt = editor.runtime_mut();
                        rt.set_reactive("SEQ", "steps", build_steps_value(&state, ct));
                        sync_all_track_sequencer_state(rt, &state, &app, ct, &selected_steps);
                        rt.run_reactive_cycle();
                        editor.refresh_runtime_side_effects();
                        editor.refresh_visible_layouts_for_buffer_named("*sequencer*");
                        editor.mark_needs_redraw();
                    }
                    // Only pass Press events to the editor (Release is only for note-off)
                    if !intercepted && key.kind == crossterm::event::KeyEventKind::Press {
                        let should_reload_custom_ui = should_reload_custom_ui_after_key(&key);
                        let previous_track = current_track.load(Ordering::Relaxed);
                        editor.handle_key(key);
                        if should_reload_custom_ui {
                            reload_custom_instrument_ui(&mut editor);
                        }
                        let next_track = current_track.load(Ordering::Relaxed);
                        if editor.active_buffer().name == "*sequencer*"
                            && next_track != previous_track
                        {
                            let track = current_track.load(Ordering::Relaxed);
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
                        pending_drag = Some((Event::Mouse(mouse), (precise_col, precise_row)));
                    } else {
                        if matches!(mouse.kind, crossterm::event::MouseEventKind::Up(_)) {
                            pending_drag = None;
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
        if let Some((Event::Mouse(mouse), (precise_col, precise_row))) = pending_drag.take() {
            editor.handle_tiled_mouse_precise(mouse, precise_col, precise_row, 0);
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
                    sessions.pending_lisp_history_transactions.insert(
                        id,
                        (label, app.history.clone(), app.history.undo_len()),
                    );
                    continue;
                }
                HostCommand::AuthoringTransactionEnd { id, success } => {
                    if let Some((label, checkpoint, checkpoint_len)) =
                        sessions.pending_lisp_history_transactions.remove(&id)
                    {
                        if success {
                            app::edit::squash_history_since(
                                &mut app,
                                checkpoint_len,
                                label,
                            );
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
                let _ = current_track_for_app(&mut app, &current_track);
                match handle_macro_host_command(
                    &name,
                    &payload,
                    &mut app,
                    &state,
                    current_track.load(Ordering::Relaxed),
                ) {
                    MacroHostCommandOutcome::Applied => {
                        ui_epoch.fetch_add(1, Ordering::Relaxed);
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
                        sync_shared_track_collapsed(&track_collapsed, &app);
                        let restored_track = if app.tracks.is_empty() {
                            0
                        } else {
                            app.ui.cursor_track.min(app.tracks.len() - 1)
                        };
                        current_track.store(restored_track, Ordering::Relaxed);
                        app.ui.cursor_track = restored_track;
                        {
                            let mut pan_ids = track_pan_ids.lock().unwrap();
                            *pan_ids = app
                                .graph
                                .track_node_ids
                                .iter()
                                .map(|ids| ids.pan_id)
                                .collect();
                            push_solo_mutes(lg_raw, &state, &pan_ids);
                        }
                        *bus_node_ids.lock().unwrap() = app.graph.bus_node_ids.clone();
                        *record_armed.lock().unwrap() = vec![false; track_names.len()];
                        recording.store(false, Ordering::Relaxed);
                        frame.prev_recording = false;
                        // Keep the shared bus mirror in sync with the loaded buses,
                        // else pull_shared_bus_state clobbers app.buses (length
                        // mismatch) and drops the group's backing bus from the UI.
                        *bus_state.lock().unwrap() = app.buses.clone();
                        // Push loaded groups into the shared runtime store; the
                        // per-frame groups diff rebuilds the SEQ.groups reactive.
                        *track_groups.lock().unwrap() = app.groups.clone();
                        {
                            let mut sel = selected_tracks.lock().unwrap();
                            sel.clear();
                            if !app.tracks.is_empty() {
                                sel.insert(restored_track);
                            }
                        }

                        let ct = current_track.load(Ordering::Relaxed);
                        let playhead = if app.tracks.is_empty() {
                            0
                        } else {
                            state.transport.track_playheads[ct].load(Ordering::Relaxed)
                        };
                        let transport_playhead = state.transport.playhead.load(Ordering::Relaxed);
                        let bpm = state.transport.bpm.load(Ordering::Relaxed);
                        if meters.last_cpu_ui_poll_at.elapsed() >= CPU_UI_POLL_INTERVAL {
                            meters.cached_cpu_load_bits =
                                state.transport.cpu_load_pct.load(Ordering::Relaxed);
                            meters.last_cpu_ui_poll_at = Instant::now();
                        }
                        let cpu_load_pct = f32::from_bits(meters.cached_cpu_load_bits);
                        let playing = state.transport.playing.load(Ordering::Relaxed);
                        let epoch = state.transport.pattern_epoch.load(Ordering::Relaxed);
                        meters.cached_peak_l_level = meter_display_level(f32::from_bits(
                            state.transport.peak_l.load(Ordering::Relaxed),
                        ));
                        meters.cached_peak_r_level = meter_display_level(f32::from_bits(
                            state.transport.peak_r.load(Ordering::Relaxed),
                        ));
                        meters.cached_track_peak_levels =
                            read_track_peak_levels(app.graph.lg, &track_pan_ids.lock().unwrap());
                        meters.cached_bus_peak_levels =
                            read_bus_peak_levels(app.graph.lg, &app.graph.bus_node_ids);
                        (meters.cached_modulator_phases, meters.cached_modulator_levels) =
                            read_modulator_display_values(app.graph.lg, &app);
                        meters.last_meter_poll_at = Instant::now();
                        let rt = editor.runtime_mut();

                        sync_pattern_state(rt, &state);
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
                        rt.set_reactive("SEQ", "master-peak-l", Value::Number(meters.cached_peak_l_level));
                        rt.set_reactive("SEQ", "master-peak-r", Value::Number(meters.cached_peak_r_level));
                        rt.set_reactive(
                            "SEQ",
                            "master-recording",
                            Value::Bool(master_recording.load(Ordering::Acquire)),
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
                            build_record_armed_value(&record_armed.lock().unwrap()),
                        );
                        rt.set_reactive(
                            "SEQ",
                            "selected-steps",
                            build_selection_value(&selected_steps),
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
                            sync_all_track_sequencer_state(rt, &state, &app, ct, &selected_steps);
                            sync_playhead_fields(
                                rt,
                                playhead as usize,
                                state.pattern.track_params[ct].get_num_steps(),
                            );
                            rt.set_reactive(
                                "SEQ",
                                "transport-playhead",
                                Value::Number(transport_playhead as f64),
                            );
                            rt.set_reactive("SEQ", "steps", build_steps_value(&state, ct));
                            sync_step_param_lists(rt, &state, ct);
                            sync_track_mixer_state(rt, &app, &state);
                            sync_track_peak_fields(rt, &meters.cached_track_peak_levels);
                            sync_bus_peak_fields(rt, &meters.cached_bus_peak_levels);
                            rt.set_reactive(
                                "SEQ",
                                "effects",
                                build_effects_value(
                                    &state,
                                    ct,
                                    &app.graph.effect_descriptors,
                                    &selected_steps,
                                ),
                            );
                            rt.set_reactive(
                                "SEQ",
                                "midi-effects",
                                build_midi_effects_value(&state, ct, &selected_steps),
                            );
                            rt.set_reactive(
                                "SEQ",
                                "instrument-panel",
                                build_instrument_panel_value(&app, ct, &selected_steps),
                            );
                            *accumulator_names.lock().unwrap() = build_accumulator_names(&app);
                            let selected_neural_snapshot =
                                selected_neural_neurons.lock().unwrap().clone();
                            sync_track_params_with_neural_selection(
                                rt,
                                &app,
                                &state,
                                ct,
                                &selected_steps,
                                Some(&selected_neural_snapshot),
                            );
                            sync_fx_param_binding_fields_with_neural_selection(
                                rt,
                                &app,
                                &state,
                                ct,
                                &selected_steps,
                                Some(&selected_neural_snapshot),
                            );
                            rt.set_reactive(
                                "SEQ",
                                "step-has-plocks",
                                build_step_has_plocks(&state, ct, &app.graph.effect_descriptors),
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
                        ui_invalidations.clear();
                        expanded_step_projection.clear();

                        frame.prev_current_track = ct;
                        frame.prev_playhead = playhead;
                        frame.prev_transport_playhead = transport_playhead;
                        frame.prev_bpm = bpm;
                        frame.prev_playing = playing;
                        frame.prev_pattern_epoch = epoch;
                        frame.prev_cpu_load_bits = meters.cached_cpu_load_bits;
                        frame.prev_peak_l_level = meters.cached_peak_l_level;
                        frame.prev_peak_r_level = meters.cached_peak_r_level;
                        frame.prev_master_recording = master_recording.load(Ordering::Acquire);
                        frame.prev_track_peak_levels = meters.cached_track_peak_levels.clone();
                        frame.prev_modulator_phases = meters.cached_modulator_phases.clone();
                        frame.prev_modulator_levels = meters.cached_modulator_levels.clone();
                        frame.prev_bus_playheads = bus_playhead_snapshot(&app);
                        frame.prev_track_playheads = track_playheads_snapshot(&state, &app);
                        frame.prev_track_button_states = track_button_state_snapshot(&state);
                        frame.prev_ui_epoch = ui_epoch.load(Ordering::Relaxed);

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
            state.publish_scheduler_snapshot();
            gesture.rack_control_snapshot_dirty = false;
        }
        if pointer_released_this_loop {
            app::edit::finish_active_gesture(&mut app);
        } else if !pointer_is_down {
            app::edit::finish_active_gesture_if_idle(&mut app);
        }
        ui_loop_stats.note_host_commands(host_commands_started.elapsed());

        if let Some(completed_load) = sessions.pending_saved_instrument_load.as_ref().and_then(|pending| {
            match pending.receiver.try_recv() {
                Ok(result) => Some(result),
                Err(std::sync::mpsc::TryRecvError::Empty) => None,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    Some(Err("Instrument load compile thread crashed".to_string()))
                }
            }
        }) {
            let pending = sessions.pending_saved_instrument_load
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
                                state: &state,
                                current_track: &current_track,
                                track_names: &mut track_names,
                                track_pan_ids: &track_pan_ids,
                                record_armed: &record_armed,
                                selected_steps: &selected_steps,
                                accumulator_names: &accumulator_names,
                                cached_track_peak_levels: &meters.cached_track_peak_levels,
                                group_id,
                                track_groups: &track_groups,
                                ui_epoch: &ui_epoch,
                                lg_raw,
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
                                state: &state,
                                current_track: &current_track,
                                track_names: &mut track_names,
                                selected_steps: &selected_steps,
                                fx_epoch: &fx_epoch,
                                ui_epoch: &ui_epoch,
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

        if let Some(completed_cancel_restore) =
            sessions.pending_instrument_cancel_restore
                .as_ref()
                .and_then(|pending| match pending.receiver.try_recv() {
                    Ok(result) => Some(result),
                    Err(std::sync::mpsc::TryRecvError::Empty) => None,
                    Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                        Some(Err("Instrument restore compile thread crashed".to_string()))
                    }
                })
        {
            let pending = sessions.pending_instrument_cancel_restore
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
                                current_track.load(Ordering::Relaxed),
                                &selected_steps,
                            ),
                        );
                        rt.run_reactive_cycle();
                        editor.refresh_runtime_side_effects();
                        ui_epoch.fetch_add(1, Ordering::Relaxed);
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

        if let Some(completed_cancel_restore) =
            sessions.pending_effect_cancel_restore.as_ref().and_then(|pending| {
                match pending.receiver.try_recv() {
                    Ok(result) => Some(result),
                    Err(std::sync::mpsc::TryRecvError::Empty) => None,
                    Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                        Some(Err("Effect restore compile thread crashed".to_string()))
                    }
                }
            })
        {
            let pending = sessions.pending_effect_cancel_restore
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
                                            &state,
                                            track,
                                            &app.graph.effect_descriptors,
                                            &selected_steps,
                                        ),
                                    );
                                }
                                EffectEditTarget::Bus { .. } => {
                                    *bus_state.lock().unwrap() = app.buses.clone();
                                    sync_bus_mixer_state(rt, &app);
                                    rt.set_reactive(
                                        "SEQ",
                                        "bus-effects",
                                        build_bus_effects_value_for_selection(
                                            &app,
                                            Some(&selected_steps),
                                        ),
                                    );
                                }
                            }
                            rt.run_reactive_cycle();
                            editor.refresh_runtime_side_effects();
                            editor.refresh_visible_layouts_for_buffer_named("*fx*");
                            fx_epoch.fetch_add(1, Ordering::Relaxed);
                            ui_epoch.fetch_add(1, Ordering::Relaxed);
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

        if let Some(completed_preview) = sessions.pending_instrument_preview.as_ref().and_then(|pending| {
            match pending.receiver.try_recv() {
                Ok(result) => Some(Ok((
                    pending.generation,
                    pending.source.clone(),
                    pending.layout.clone(),
                    result,
                ))),
                Err(std::sync::mpsc::TryRecvError::Empty) => None,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => Some(Err(())),
            }
        }) {
            let _ = sessions.pending_instrument_preview.take();
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
                                                current_track.load(Ordering::Relaxed),
                                                &selected_steps,
                                            ),
                                        );
                                        rt.run_reactive_cycle();
                                        editor.refresh_runtime_side_effects();
                                        ui_epoch.fetch_add(1, Ordering::Relaxed);
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
        }

        if let Some(completed_preview) =
            sessions.pending_effect_preview
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
                                                            &state,
                                                            track,
                                                            &app.graph.effect_descriptors,
                                                            &selected_steps,
                                                        ),
                                                    );
                                                }
                                                EffectEditTarget::Bus { .. } => {
                                                    *bus_state.lock().unwrap() = app.buses.clone();
                                                    sync_bus_mixer_state(rt, &app);
                                                    rt.set_reactive(
                                                        "SEQ",
                                                        "bus-effects",
                                                        build_bus_effects_value_for_selection(
                                                            &app,
                                                            Some(&selected_steps),
                                                        ),
                                                    );
                                                }
                                            }
                                            rt.run_reactive_cycle();
                                            editor.refresh_runtime_side_effects();
                                            editor.refresh_visible_layouts_for_buffer_named("*fx*");
                                            fx_epoch.fetch_add(1, Ordering::Relaxed);
                                            ui_epoch.fetch_add(1, Ordering::Relaxed);
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
                    sequencer::agent::agentic_bubble::AgenticBubbleOutput::Answer { text } => {
                        eseqlisp::widget_render::patcher::resolve_agentic_bubble_answer(
                            &pending.path,
                            &pending.bubble_id,
                            pending.generation,
                            text,
                        );
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

        poll_pending_compile_status(
            &mut app,
            &mut editor,
            &state,
            &current_track,
            &selected_steps,
            &fx_epoch,
            &ui_epoch,
        );

        // 2. Sync reactive state AFTER events
        let ct = current_track_for_app(&mut app, &current_track).unwrap_or(0);
        sync_watched_sampler_voices(
            &app,
            ct,
            &mut frame.watched_sampler_voice_track,
            &mut frame.watched_sampler_voice_ids,
        );
        let reactive_sync_started = Instant::now();
        {
            let playing = state.transport.playing.load(Ordering::Relaxed);
            let bpm = state.transport.bpm.load(Ordering::Relaxed);
            if meters.last_cpu_ui_poll_at.elapsed() >= CPU_UI_POLL_INTERVAL {
                meters.cached_cpu_load_bits = state.transport.cpu_load_pct.load(Ordering::Relaxed);
                meters.last_cpu_ui_poll_at = Instant::now();
            }
            let cpu_load_bits = meters.cached_cpu_load_bits;
            let transport_playhead = state.transport.playhead.load(Ordering::Relaxed);
            let playhead = state.transport.track_playheads[ct].load(Ordering::Relaxed);
            let bus_playheads = bus_playhead_snapshot(&app);
            let epoch = state.transport.pattern_epoch.load(Ordering::Relaxed);
            let metal_visible = editor_has_visible_buffer(&editor, "*metal*");
            let mixer_visible = editor_has_visible_buffer(&editor, "*mixer*");
            let sequencer_visible = editor_has_visible_buffer(&editor, "*sequencer*");
            let fx_visible = editor_has_visible_buffer(&editor, "*fx*");
            let step_visible = editor_has_visible_buffer(&editor, "*step*");
            let transport_visible = editor_has_visible_buffer(&editor, "*transport*");
            let master_meter_visible = transport_visible || mixer_visible;
            let track_meter_visible =
                track_meter_bindings_visible(mixer_visible, sequencer_visible);
            let current_track_playhead_visible = editor_has_visible_buffer(&editor, "*metal*")
                || editor_has_visible_buffer(&editor, "*piano-roll*");
            let current_track_playhead_changed = playhead != frame.prev_playhead;
            if meters.last_meter_poll_at.elapsed() >= METER_POLL_INTERVAL {
                meters.cached_peak_l_level = meter_display_level(f32::from_bits(
                    state.transport.peak_l.load(Ordering::Relaxed),
                ));
                meters.cached_peak_r_level = meter_display_level(f32::from_bits(
                    state.transport.peak_r.load(Ordering::Relaxed),
                ));
                meters.cached_track_peak_levels =
                    read_track_peak_levels(app.graph.lg, &track_pan_ids.lock().unwrap());
                meters.cached_rack_slot_peak_levels = read_rack_slot_peak_levels(app.graph.lg, &app);
                meters.cached_bus_peak_levels =
                    read_bus_peak_levels(app.graph.lg, &app.graph.bus_node_ids);
                (meters.cached_modulator_phases, meters.cached_modulator_levels) =
                    read_modulator_display_values(app.graph.lg, &app);
                meters.last_meter_poll_at = Instant::now();
            }
            let mut needs_reactive_cycle = false;
            let mut refresh_visible_step_after_cycle = false;
            let selected_neural_snapshot = selected_neural_neurons.lock().unwrap().clone();
            if fx_visible {
                let active_notes = state.active_notes(ct);
                if active_notes != frame.prev_instrument_active_notes {
                    needs_reactive_cycle |= editor
                        .runtime_mut()
                        .set_reactive(
                            "SEQ",
                            "instrument-active-notes",
                            build_active_notes_value(&active_notes),
                        )
                        .effects_dirty;
                    frame.prev_instrument_active_notes = active_notes;
                }
            }
            if selected_neural_snapshot != frame.prev_selected_neural_neurons {
                needs_reactive_cycle |= sync_selected_neural_neuron_bindings(
                    editor.runtime_mut(),
                    &state,
                    &selected_neural_snapshot,
                );
                needs_reactive_cycle |= sync_fx_param_binding_fields_with_neural_selection(
                    editor.runtime_mut(),
                    &app,
                    &state,
                    ct,
                    &selected_steps,
                    Some(&selected_neural_snapshot),
                );
                needs_reactive_cycle |= sync_track_plocks_for_neural_selection(
                    editor.runtime_mut(),
                    &app,
                    &state,
                    ct,
                    &selected_steps,
                    &selected_neural_snapshot,
                );
                frame.prev_selected_neural_neurons = selected_neural_snapshot.clone();
            }
            // Track switch — rebuild everything
            if ct != frame.prev_current_track && !app.tracks.is_empty() {
                editor.reset_widget_scroll_for_buffer_named("*metal*");
                editor.reset_widget_scroll_for_buffer_named("*fx*");
                gesture.preview_plock_variant = None;
                let cleared_step_selection = {
                    let mut selection = selected_steps.lock().unwrap();
                    let had_selection = !selection.is_empty();
                    selection.clear();
                    had_selection
                };
                let cleared_piano_selection = {
                    let mut selection = piano_roll_selection.lock().unwrap();
                    let had_selection = !selection.is_empty();
                    selection.clear();
                    had_selection
                };
                if cleared_step_selection || cleared_piano_selection {
                    fx_epoch.fetch_add(1, Ordering::Relaxed);
                }
                let _ = editor.runtime_mut().eval_str("(set! selected-bus -1)");
                reset_sampler_waveform_view(&mut editor);
                let rt = editor.runtime_mut();
                sync_shared_track_collapsed(&track_collapsed, &app);
                sync_track_name_state(rt, &mut track_names, &app);
                sync_pattern_state(rt, &state);
                set_current_track_reactive(rt, app.tracks.len(), ct);
                if current_track_playhead_visible {
                    sync_playhead_fields(
                        rt,
                        playhead as usize,
                        state.pattern.track_params[ct].get_num_steps(),
                    );
                }
                if transport_visible {
                    rt.set_reactive(
                        "SEQ",
                        "transport-playhead",
                        Value::Number(transport_playhead as f64),
                    );
                }
                rt.set_reactive("SEQ", "steps", build_steps_value(&state, ct));
                sync_piano_roll_state(rt, &state, ct, &piano_roll_selection);
                sync_step_param_lists(rt, &state, ct);
                sync_track_mixer_state(rt, &app, &state);
                sync_bus_mixer_state(rt, &app);
                sync_track_peak_fields(rt, &meters.cached_track_peak_levels);
                sync_bus_peak_fields(rt, &meters.cached_bus_peak_levels);
                sync_modulator_phase_fields(rt, &meters.cached_modulator_phases);
                sync_modulator_level_fields(rt, &meters.cached_modulator_levels);
                rt.set_reactive(
                    "SEQ",
                    "effects",
                    build_effects_value(&state, ct, &app.graph.effect_descriptors, &selected_steps),
                );
                rt.set_reactive(
                    "SEQ",
                    "midi-effects",
                    build_midi_effects_value(&state, ct, &selected_steps),
                );
                rt.set_reactive(
                    "SEQ",
                    "instrument-panel",
                    build_instrument_panel_value(&app, ct, &selected_steps),
                );
                *accumulator_names.lock().unwrap() = build_accumulator_names(&app);
                sync_track_params_with_neural_selection(
                    rt,
                    &app,
                    &state,
                    ct,
                    &selected_steps,
                    Some(&selected_neural_snapshot),
                );
                sync_fx_param_binding_fields_with_neural_selection(
                    rt,
                    &app,
                    &state,
                    ct,
                    &selected_steps,
                    Some(&selected_neural_snapshot),
                );
                rt.set_reactive(
                    "SEQ",
                    "step-has-plocks",
                    build_step_has_plocks(&state, ct, &app.graph.effect_descriptors),
                );
                sync_sidebar_browser(rt, &app, ct);
                frame.prev_current_track = ct;
                frame.prev_playhead = playhead;
                frame.prev_transport_playhead = transport_playhead;
                frame.prev_pattern_epoch = epoch;
                needs_reactive_cycle = true;
            }

            // Track-groups reconcile: pull native-mutated groups (collapse toggle,
            // group create) into app.groups and rebuild the SEQ.groups reactive.
            {
                let groups_snapshot = track_groups.lock().unwrap().clone();
                if groups_snapshot != frame.prev_groups {
                    app.groups = groups_snapshot.clone();
                    let rt = editor.runtime_mut();
                    sync_groups_bindings(rt, &app.groups);
                    frame.prev_groups = groups_snapshot;
                    needs_reactive_cycle = true;
                }
            }

            // Multi-select highlight reconcile. Runs after the track-switch block
            // so it overrides the single-select bindings written there.
            {
                let selected_snapshot = selected_tracks.lock().unwrap().clone();
                if selected_snapshot != frame.prev_selected_tracks {
                    let rt = editor.runtime_mut();
                    sync_selected_tracks_bindings(rt, app.tracks.len(), ct, &selected_snapshot);
                    frame.prev_selected_tracks = selected_snapshot;
                    needs_reactive_cycle = true;
                }
            }

            if playing != frame.prev_playing {
                let rt = editor.runtime_mut();
                rt.set_reactive("SEQ", "playing", Value::Bool(playing));
                if sequencer_visible {
                    if playing {
                        sync_all_track_playhead_fields(rt, &state, &app);
                    } else {
                        clear_all_track_playhead_fields(rt, &app);
                    }
                }
                frame.prev_playing = playing;
                needs_reactive_cycle = true;
                if (fx_visible || step_visible) && !app.tracks.is_empty() {
                    let rt = editor.runtime_mut();
                    sync_track_params_with_neural_selection(
                        rt,
                        &app,
                        &state,
                        ct,
                        &selected_steps,
                        Some(&selected_neural_snapshot),
                    );
                    if gesture.preview_plock_variant.as_ref().is_some_and(|(track, _)| {
                        *track != ct || !selected_steps.lock().unwrap().is_empty()
                    }) {
                        gesture.preview_plock_variant = None;
                    }
                    let preview_dirty = sync_track_plock_variant_preview(
                        rt,
                        &app,
                        &state,
                        ct,
                        &selected_steps,
                        gesture.preview_plock_variant.as_ref(),
                    );
                    needs_reactive_cycle |= preview_dirty;
                    refresh_visible_step_after_cycle |= preview_dirty;
                    if fx_visible {
                        sync_fx_param_binding_fields_with_neural_selection(
                            rt,
                            &app,
                            &state,
                            ct,
                            &selected_steps,
                            Some(&selected_neural_snapshot),
                        );
                    }
                }
            }
            if bpm != frame.prev_bpm {
                app.push_all_delay_bpm();
                editor
                    .runtime_mut()
                    .set_reactive("SEQ", "bpm", Value::Number(bpm as f64));
                frame.prev_bpm = bpm;
                needs_reactive_cycle = true;
            }
            if transport_visible && cpu_load_bits != frame.prev_cpu_load_bits {
                needs_reactive_cycle |= editor
                    .runtime_mut()
                    .set_reactive(
                        "SEQ",
                        "cpu-load-pct",
                        Value::Number(f32::from_bits(cpu_load_bits) as f64),
                    )
                    .effects_dirty;
                frame.prev_cpu_load_bits = cpu_load_bits;
            }
            if !transport_visible && cpu_load_bits != frame.prev_cpu_load_bits {
                frame.prev_cpu_load_bits = cpu_load_bits;
            }
            let master_rec_on = master_recording.load(Ordering::Acquire);
            app.ui.master_recording = master_rec_on;
            if transport_visible && master_rec_on != frame.prev_master_recording {
                needs_reactive_cycle |= editor
                    .runtime_mut()
                    .set_reactive("SEQ", "master-recording", Value::Bool(master_rec_on))
                    .effects_dirty;
                frame.prev_master_recording = master_rec_on;
            }
            if !transport_visible && master_rec_on != frame.prev_master_recording {
                frame.prev_master_recording = master_rec_on;
            }
            if master_meter_visible && meters.cached_peak_l_level != frame.prev_peak_l_level {
                needs_reactive_cycle |= editor
                    .runtime_mut()
                    .set_reactive("SEQ", "master-peak-l", Value::Number(meters.cached_peak_l_level))
                    .effects_dirty;
                frame.prev_peak_l_level = meters.cached_peak_l_level;
            }
            if !master_meter_visible && meters.cached_peak_l_level != frame.prev_peak_l_level {
                frame.prev_peak_l_level = meters.cached_peak_l_level;
            }
            if master_meter_visible && meters.cached_peak_r_level != frame.prev_peak_r_level {
                needs_reactive_cycle |= editor
                    .runtime_mut()
                    .set_reactive("SEQ", "master-peak-r", Value::Number(meters.cached_peak_r_level))
                    .effects_dirty;
                frame.prev_peak_r_level = meters.cached_peak_r_level;
            }
            if !master_meter_visible && meters.cached_peak_r_level != frame.prev_peak_r_level {
                frame.prev_peak_r_level = meters.cached_peak_r_level;
            }
            if meters.cached_track_peak_levels != frame.prev_track_peak_levels {
                if track_meter_visible {
                    needs_reactive_cycle |= sync_track_peak_field_delta(
                        editor.runtime_mut(),
                        &frame.prev_track_peak_levels,
                        &meters.cached_track_peak_levels,
                    );
                }
                frame.prev_track_peak_levels = meters.cached_track_peak_levels.clone();
            }
            if meters.cached_rack_slot_peak_levels != frame.prev_rack_slot_peak_levels {
                if track_meter_visible {
                    needs_reactive_cycle |= sync_rack_slot_peak_field_delta(
                        editor.runtime_mut(),
                        &frame.prev_rack_slot_peak_levels,
                        &meters.cached_rack_slot_peak_levels,
                    );
                }
                frame.prev_rack_slot_peak_levels = meters.cached_rack_slot_peak_levels.clone();
            }
            if meters.cached_bus_peak_levels != frame.prev_bus_peak_levels {
                if mixer_visible {
                    needs_reactive_cycle |= sync_bus_peak_field_delta(
                        editor.runtime_mut(),
                        &frame.prev_bus_peak_levels,
                        &meters.cached_bus_peak_levels,
                    );
                }
                frame.prev_bus_peak_levels = meters.cached_bus_peak_levels.clone();
            }
            if meters.last_neural_visualization_poll_at.elapsed() >= NEURAL_VISUALIZATION_POLL_INTERVAL {
                meters.last_neural_visualization_poll_at = Instant::now();
                needs_reactive_cycle |=
                    sync_neural_visualization_fields(editor.runtime_mut(), &state);
            }
            if meters.cached_modulator_phases != frame.prev_modulator_phases {
                if fx_visible {
                    needs_reactive_cycle |= sync_modulator_phase_field_delta(
                        editor.runtime_mut(),
                        &frame.prev_modulator_phases,
                        &meters.cached_modulator_phases,
                    );
                }
                frame.prev_modulator_phases = meters.cached_modulator_phases.clone();
            }
            if meters.cached_modulator_levels != frame.prev_modulator_levels {
                if fx_visible {
                    needs_reactive_cycle |= sync_modulator_level_field_delta(
                        editor.runtime_mut(),
                        &frame.prev_modulator_levels,
                        &meters.cached_modulator_levels,
                    );
                }
                frame.prev_modulator_levels = meters.cached_modulator_levels.clone();
            }
            if bus_playheads != frame.prev_bus_playheads {
                if metal_visible {
                    editor.runtime_mut().set_reactive(
                        "SEQ",
                        "bus-playheads",
                        build_bus_playheads_value(&app),
                    );
                    needs_reactive_cycle = true;
                }
                frame.prev_bus_playheads = bus_playheads;
            }
            if sequencer_visible {
                let previous_track_playheads = frame.prev_track_playheads.clone();
                if sync_track_playhead_field_delta(
                    editor.runtime_mut(),
                    &state,
                    &app,
                    &mut frame.prev_track_playheads,
                ) {
                    needs_reactive_cycle = true;
                }
                if previous_track_playheads != frame.prev_track_playheads {
                    let auto_follow_now = auto_follow_enabled(&auto_follow_override_until);
                    let selection_empty = selected_steps.lock().unwrap().is_empty();
                    let selected = selected_steps.lock().unwrap();
                    let rt = editor.runtime_mut();
                    for mut viewport in expanded_step_projection.all_viewports() {
                        if viewport.track >= app.tracks.len() {
                            continue;
                        }
                        let active_step = track_active_playhead_step(&state, viewport.track);
                        let active_page = active_step / PAGE_SIZE;
                        if playing && auto_follow_now && selection_empty {
                            if viewport.page != active_page {
                                viewport.page = active_page;
                                viewport.cursor_step = active_step;
                                expanded_step_projection.set_viewport(viewport);
                                needs_reactive_cycle |= sync_expanded_step_viewport(
                                    rt, &state, &app, &selected, ct, viewport,
                                );
                                continue;
                            }
                        }
                        needs_reactive_cycle |=
                            sync_expanded_step_viewport_playhead(rt, &state, viewport);
                    }
                }
            } else {
                frame.prev_track_playheads = track_playheads_snapshot(&state, &app);
            }
            if current_track_playhead_visible
                && (!frame.prev_current_track_playhead_visible || playhead != frame.prev_playhead)
                && !app.tracks.is_empty()
            {
                if frame.prev_current_track_playhead_visible {
                    needs_reactive_cycle |= sync_playhead_field_delta(
                        editor.runtime_mut(),
                        frame.prev_playhead as usize,
                        playhead as usize,
                        state.pattern.track_params[ct].get_num_steps(),
                    );
                } else {
                    needs_reactive_cycle |= sync_playhead_fields(
                        editor.runtime_mut(),
                        playhead as usize,
                        state.pattern.track_params[ct].get_num_steps(),
                    );
                }
                frame.prev_playhead = playhead;
            }
            if !current_track_playhead_visible && frame.prev_playhead != playhead {
                frame.prev_playhead = playhead;
            }
            if (fx_visible || step_visible)
                && current_track_playhead_changed
                && !app.tracks.is_empty()
            {
                let rt = editor.runtime_mut();
                sync_track_params_with_neural_selection(
                    rt,
                    &app,
                    &state,
                    ct,
                    &selected_steps,
                    Some(&selected_neural_snapshot),
                );
                if gesture.preview_plock_variant.as_ref().is_some_and(|(track, _)| {
                    *track != ct || !selected_steps.lock().unwrap().is_empty()
                }) {
                    gesture.preview_plock_variant = None;
                }
                let preview_dirty = sync_track_plock_variant_preview(
                    rt,
                    &app,
                    &state,
                    ct,
                    &selected_steps,
                    gesture.preview_plock_variant.as_ref(),
                );
                refresh_visible_step_after_cycle |= preview_dirty;
                if fx_visible {
                    sync_fx_param_binding_fields_with_neural_selection(
                        rt,
                        &app,
                        &state,
                        ct,
                        &selected_steps,
                        Some(&selected_neural_snapshot),
                    );
                }
                needs_reactive_cycle = true;
            }
            frame.prev_current_track_playhead_visible = current_track_playhead_visible;
            let mut profile_pattern_reactive_cycle = false;
            let mut refresh_visible_sequencer_after_cycle = false;
            let mut refresh_visible_mixer_after_cycle = false;
            let mut refresh_visible_samples_after_cycle = false;
            let typed_invalidations = ui_invalidations.drain();
            if apply_ui_invalidations(
                typed_invalidations,
                UiInvalidationApplyCtx {
                    app: &mut app,
                    editor: &mut editor,
                    state: &state,
                    track_collapsed: &track_collapsed,
                    bus_state: &bus_state,
                    current_track_idx: ct,
                    selected_steps: &selected_steps,
                    selected_neural_neurons: &selected_neural_snapshot,
                    piano_roll_selection: &piano_roll_selection,
                    accumulator_names: &accumulator_names,
                    cached_track_peak_levels: &meters.cached_track_peak_levels,
                    cached_bus_peak_levels: &meters.cached_bus_peak_levels,
                    record_armed: &record_armed,
                    active_delete_target: &active_delete_target,
                    active_delete_target_version: &active_delete_target_version,
                    expanded_step_projection: &expanded_step_projection,
                    fx_visible,
                    sequencer_visible,
                    mixer_visible,
                },
            ) {
                needs_reactive_cycle = true;
            }
            if epoch != frame.prev_pattern_epoch && !app.tracks.is_empty() {
                let profile_switch = pattern_switch_profile_enabled();
                let profile_total_started = Instant::now();
                let sync_names_pattern_elapsed;
                let mut sync_playhead_elapsed = Duration::ZERO;
                let sync_current_steps_elapsed;
                let sync_sequencer_elapsed;
                let sync_expanded_elapsed;
                let sync_piano_elapsed;
                let sync_step_params_elapsed;
                let sync_mixer_elapsed;
                let sync_track_params_elapsed;
                let sync_fx_bindings_elapsed;
                let sync_plocks_sidebar_elapsed;
                let old_pattern_epoch = frame.prev_pattern_epoch;
                let rt = editor.runtime_mut();
                let started = Instant::now();
                sync_shared_track_collapsed(&track_collapsed, &app);
                sync_track_name_state(rt, &mut track_names, &app);
                sync_pattern_state(rt, &state);
                let selected_neural_snapshot = selected_neural_neurons.lock().unwrap().clone();
                sync_selected_neural_neuron_bindings(rt, &state, &selected_neural_snapshot);
                sync_names_pattern_elapsed = started.elapsed();
                if current_track_playhead_visible {
                    let started = Instant::now();
                    sync_playhead_fields(
                        rt,
                        playhead as usize,
                        state.pattern.track_params[ct].get_num_steps(),
                    );
                    sync_playhead_elapsed = started.elapsed();
                }
                let started = Instant::now();
                rt.set_reactive("SEQ", "steps", build_steps_value(&state, ct));
                sync_current_steps_elapsed = started.elapsed();
                let started = Instant::now();
                sync_all_track_sequencer_state(rt, &state, &app, ct, &selected_steps);
                sync_sequencer_elapsed = started.elapsed();
                let started = Instant::now();
                if sequencer_visible {
                    let _ = sync_all_expanded_step_viewports(
                        rt,
                        &state,
                        &app,
                        &selected_steps,
                        ct,
                        &expanded_step_projection,
                    );
                }
                sync_expanded_elapsed = started.elapsed();
                let started = Instant::now();
                sync_piano_roll_state(rt, &state, ct, &piano_roll_selection);
                sync_piano_elapsed = started.elapsed();
                let started = Instant::now();
                sync_step_param_lists(rt, &state, ct);
                sync_step_params_elapsed = started.elapsed();
                let started = Instant::now();
                sync_track_mixer_state(rt, &app, &state);
                sync_bus_mixer_state(rt, &app);
                if track_meter_visible {
                    sync_track_peak_fields(rt, &meters.cached_track_peak_levels);
                }
                if mixer_visible {
                    sync_bus_peak_fields(rt, &meters.cached_bus_peak_levels);
                }
                sync_mixer_elapsed = started.elapsed();
                *accumulator_names.lock().unwrap() = build_accumulator_names(&app);
                let started = Instant::now();
                sync_track_params_with_neural_selection(
                    rt,
                    &app,
                    &state,
                    ct,
                    &selected_steps,
                    Some(&selected_neural_snapshot),
                );
                if gesture.preview_plock_variant.as_ref().is_some_and(|(track, _)| {
                    *track != ct || !selected_steps.lock().unwrap().is_empty()
                }) {
                    gesture.preview_plock_variant = None;
                }
                let preview_dirty = sync_track_plock_variant_preview(
                    rt,
                    &app,
                    &state,
                    ct,
                    &selected_steps,
                    gesture.preview_plock_variant.as_ref(),
                );
                refresh_visible_step_after_cycle |= preview_dirty;
                sync_track_params_elapsed = started.elapsed();
                let started = Instant::now();
                sync_fx_param_binding_fields_with_neural_selection(
                    rt,
                    &app,
                    &state,
                    ct,
                    &selected_steps,
                    Some(&selected_neural_snapshot),
                );
                sync_fx_bindings_elapsed = started.elapsed();
                frame.prev_selected_neural_neurons = selected_neural_snapshot;
                let started = Instant::now();
                rt.set_reactive(
                    "SEQ",
                    "step-has-plocks",
                    build_step_has_plocks(&state, ct, &app.graph.effect_descriptors),
                );
                sync_sidebar_browser(rt, &app, ct);
                sync_plocks_sidebar_elapsed = started.elapsed();
                if profile_switch {
                    eprintln!(
                        "[pattern-switch-profile][epoch-sync] total={:.2}ms epoch {}->{} names_pattern={:.2}ms playhead={:.2}ms current_steps={:.2}ms sequencer_bindings={:.2}ms expanded_step_viewports={:.2}ms piano={:.2}ms step_params={:.2}ms mixer={:.2}ms track_params={:.2}ms fx_bindings={:.2}ms plocks_sidebar={:.2}ms",
                        duration_ms(profile_total_started.elapsed()),
                        old_pattern_epoch,
                        epoch,
                        duration_ms(sync_names_pattern_elapsed),
                        duration_ms(sync_playhead_elapsed),
                        duration_ms(sync_current_steps_elapsed),
                        duration_ms(sync_sequencer_elapsed),
                        duration_ms(sync_expanded_elapsed),
                        duration_ms(sync_piano_elapsed),
                        duration_ms(sync_step_params_elapsed),
                        duration_ms(sync_mixer_elapsed),
                        duration_ms(sync_track_params_elapsed),
                        duration_ms(sync_fx_bindings_elapsed),
                        duration_ms(sync_plocks_sidebar_elapsed),
                    );
                }
                frame.prev_pattern_epoch = epoch;
                frame.prev_track_button_states = track_button_state_snapshot(&state);
                needs_reactive_cycle = true;
                refresh_visible_mixer_after_cycle |= mixer_visible;
                profile_pattern_reactive_cycle = profile_switch;
            }
            let ui_ep = ui_epoch.load(Ordering::Relaxed);
            if ui_ep != frame.prev_ui_epoch {
                if std::env::var_os("ESEQLISP_TRACE_UI").is_some() {
                    eprintln!(
                        "[ui-trace][metal_seq] ui_epoch {}->{} visible metal={} mixer={} sequencer={} fx={} ct={}",
                        frame.prev_ui_epoch,
                        ui_ep,
                        metal_visible,
                        mixer_visible,
                        sequencer_visible,
                        fx_visible,
                        ct
                    );
                }
                pull_shared_bus_state(&mut app, &bus_state);
                let track_button_states = track_button_state_snapshot(&state);
                let track_buttons_changed = track_button_states != frame.prev_track_button_states;
                if std::env::var_os("ESEQLISP_TRACE_UI").is_some() {
                    eprintln!(
                        "[ui-trace][metal_seq] track_buttons_changed={} prev_buttons={} next_buttons={}",
                        track_buttons_changed,
                        frame.prev_track_button_states.len(),
                        track_button_states.len()
                    );
                }
                let rt = editor.runtime_mut();
                sync_macro_state(rt, &app);
                if app.tracks.is_empty() {
                    sync_track_topology_state(
                        rt,
                        &app,
                        &state,
                        &mut track_names,
                        ct,
                        &selected_steps,
                        &piano_roll_selection,
                        &accumulator_names,
                        &record_armed,
                        &meters.cached_track_peak_levels,
                    );
                    sync_bus_peak_fields(rt, &meters.cached_bus_peak_levels);
                } else {
                    sync_shared_track_collapsed(&track_collapsed, &app);
                    sync_track_name_state(rt, &mut track_names, &app);
                    rt.set_reactive("SEQ", "steps", build_steps_value(&state, ct));
                    sync_step_param_lists(rt, &state, ct);
                    if metal_visible || sequencer_visible {
                        sync_all_track_sequencer_state(rt, &state, &app, ct, &selected_steps);
                    }
                    if sequencer_visible {
                        let _ = sync_all_expanded_step_viewports(
                            rt,
                            &state,
                            &app,
                            &selected_steps,
                            ct,
                            &expanded_step_projection,
                        );
                    }
                    sync_track_mixer_state(rt, &app, &state);
                    sync_bus_mixer_state(rt, &app);
                    sync_track_peak_fields(rt, &meters.cached_track_peak_levels);
                    sync_bus_peak_fields(rt, &meters.cached_bus_peak_levels);
                    *accumulator_names.lock().unwrap() = build_accumulator_names(&app);
                    sync_track_params_with_neural_selection(
                        rt,
                        &app,
                        &state,
                        ct,
                        &selected_steps,
                        Some(&selected_neural_snapshot),
                    );
                    if gesture.preview_plock_variant.as_ref().is_some_and(|(track, _)| {
                        *track != ct || !selected_steps.lock().unwrap().is_empty()
                    }) {
                        gesture.preview_plock_variant = None;
                    }
                    let preview_dirty = sync_track_plock_variant_preview(
                        rt,
                        &app,
                        &state,
                        ct,
                        &selected_steps,
                        gesture.preview_plock_variant.as_ref(),
                    );
                    refresh_visible_step_after_cycle |= preview_dirty;
                    sync_fx_param_binding_fields_with_neural_selection(
                        rt,
                        &app,
                        &state,
                        ct,
                        &selected_steps,
                        Some(&selected_neural_snapshot),
                    );
                    rt.set_reactive(
                        "SEQ",
                        "selected-steps",
                        build_selection_value(&selected_steps),
                    );
                    sync_piano_roll_state(rt, &state, ct, &piano_roll_selection);
                    rt.set_reactive(
                        "SEQ",
                        "step-has-plocks",
                        build_step_has_plocks(&state, ct, &app.graph.effect_descriptors),
                    );
                }
                // Sync recording state
                let rec_on = recording.load(Ordering::Relaxed);
                let master_rec_on = master_recording.load(Ordering::Acquire);
                rt.set_reactive("SEQ", "recording", Value::Bool(rec_on));
                rt.set_reactive("SEQ", "master-recording", Value::Bool(master_rec_on));
                rt.set_reactive(
                    "SEQ",
                    "delete-target-version",
                    Value::Number(active_delete_target_version.load(Ordering::Relaxed) as f64),
                );
                sync_mixer_delete_target_binding_fields(
                    rt,
                    app.tracks.len(),
                    &state,
                    active_delete_target.lock().unwrap().as_ref(),
                );
                let armed = record_armed.lock().unwrap();
                let record_armed_changed = armed.len() != app.graph.record_armed.len()
                    || armed
                        .iter()
                        .enumerate()
                        .any(|(i, armed)| app.graph.record_armed.get(i) != Some(armed));
                rt.set_reactive("SEQ", "record-armed", build_record_armed_value(&armed));
                // Sync to app for TUI recording logic
                app.ui.recording = rec_on;
                app.ui.master_recording = master_rec_on;
                frame.prev_master_recording = master_rec_on;
                for (i, a) in armed.iter().enumerate() {
                    if i < app.graph.record_armed.len() {
                        app.graph.record_armed[i] = *a;
                    }
                }
                refresh_visible_sequencer_after_cycle = sequencer_visible;
                refresh_visible_mixer_after_cycle |=
                    mixer_visible && (record_armed_changed || track_buttons_changed);
                if std::env::var_os("ESEQLISP_TRACE_UI").is_some() {
                    eprintln!(
                        "[ui-trace][metal_seq] refresh_after_cycle sequencer={} mixer={} record_armed_changed={} track_buttons_changed={}",
                        refresh_visible_sequencer_after_cycle,
                        refresh_visible_mixer_after_cycle,
                        record_armed_changed,
                        track_buttons_changed
                    );
                }
                frame.prev_track_button_states = track_button_states;
                frame.prev_ui_epoch = ui_ep;
                needs_reactive_cycle = true;
            }
            let fx_ep = fx_epoch.load(Ordering::Relaxed);
            if fx_visible && fx_ep != frame.prev_fx_epoch {
                let rt = editor.runtime_mut();
                rt.set_reactive(
                    "SEQ",
                    "effects",
                    if app.tracks.is_empty() {
                        Value::List(vec![])
                    } else {
                        build_effects_value(
                            &state,
                            ct,
                            &app.graph.effect_descriptors,
                            &selected_steps,
                        )
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
                rt.set_reactive(
                    "SEQ",
                    "step-has-plocks",
                    if app.tracks.is_empty() {
                        Value::List(vec![])
                    } else {
                        build_step_has_plocks(&state, ct, &app.graph.effect_descriptors)
                    },
                );
                rt.set_reactive(
                    "SEQ",
                    "bus-effects",
                    build_bus_effects_value_for_selection(&app, Some(&selected_steps)),
                );
                frame.prev_fx_epoch = fx_ep;
                needs_reactive_cycle = true;
            }
            if transport_visible && transport_playhead != frame.prev_transport_playhead {
                needs_reactive_cycle |= editor
                    .runtime_mut()
                    .set_reactive(
                        "SEQ",
                        "transport-playhead",
                        Value::Number(transport_playhead as f64),
                    )
                    .effects_dirty;
                frame.prev_transport_playhead = transport_playhead;
            }
            if !transport_visible && transport_playhead != frame.prev_transport_playhead {
                frame.prev_transport_playhead = transport_playhead;
            }
            {
                let ct = current_track.load(Ordering::Relaxed);
                let analysis_key = if app.is_sampler_track(ct) {
                    let buffer_id = app.graph.track_buffer_ids.get(ct).copied().unwrap_or(-1);
                    let entry = app.sample_analysis.cache().get(buffer_id);
                    let (status, bpm_bits, onset_count) = match entry.as_deref() {
                        Some(sequencer::analysis::AnalysisEntry::Pending) => (1, 0, 0),
                        Some(sequencer::analysis::AnalysisEntry::Ready(result)) => {
                            (2, result.bpm.to_bits(), result.onsets_frames.len())
                        }
                        Some(sequencer::analysis::AnalysisEntry::Failed(_)) => (3, 0, 0),
                        None => (0, 0, 0),
                    };
                    Some((ct, buffer_id, status, bpm_bits, onset_count))
                } else {
                    None
                };
                if analysis_key != frame.prev_sampler_analysis_key {
                    if let Some((ct, _, _, _, _)) = analysis_key {
                        app.publish_sampler_analysis_runtime(ct);
                        editor.runtime_mut().set_reactive(
                            "SEQ",
                            "instrument-panel",
                            build_instrument_panel_value(&app, ct, &selected_steps),
                        );
                        needs_reactive_cycle = true;
                    }
                    frame.prev_sampler_analysis_key = analysis_key;
                }
            }
            // Update sampler playhead for waveform display
            {
                let ct = current_track.load(Ordering::Relaxed);
                if app.is_sampler_track(ct) {
                    let ph = read_sampler_playhead_seconds(&app, ct);
                    if ph > 0.0 {
                        editor.runtime_mut().set_reactive(
                            "SEQ",
                            "sampler-playhead",
                            Value::Number(ph),
                        );
                        needs_reactive_cycle = true;
                    }
                }
            }
            let auto_follow = auto_follow_enabled(&auto_follow_override_until);
            if auto_follow != frame.prev_auto_follow {
                editor
                    .runtime_mut()
                    .set_reactive("SEQ", "auto-follow", Value::Bool(auto_follow));
                frame.prev_auto_follow = auto_follow;
                needs_reactive_cycle = true;
            }
            let editor_macro_action = sessions.instrument_edit_session
                .as_ref()
                .and_then(active_instrument_editor_macro_action)
                .or_else(|| {
                    sessions.effect_edit_session
                        .as_ref()
                        .and_then(active_effect_editor_macro_action)
                });
            let editor_macro_action = editor_macro_action_strings(editor_macro_action.as_ref());
            if editor_macro_action != frame.prev_editor_macro_action {
                let rt = editor.runtime_mut();
                rt.set_reactive(
                    "SEQ",
                    "editor-active-macro-name",
                    Value::String(editor_macro_action.0.clone()),
                );
                rt.set_reactive(
                    "SEQ",
                    "editor-active-macro-action",
                    Value::String(editor_macro_action.1.clone()),
                );
                frame.prev_editor_macro_action = editor_macro_action;
                refresh_visible_samples_after_cycle = true;
                needs_reactive_cycle = true;
            }

            if needs_reactive_cycle {
                let profile_cycle = profile_pattern_reactive_cycle;
                let cycle_total_started = Instant::now();
                let started = Instant::now();
                editor.runtime_mut().run_reactive_cycle();
                let reactive_elapsed = started.elapsed();
                let started = Instant::now();
                editor.refresh_runtime_side_effects();
                let side_effects_elapsed = started.elapsed();
                let mut refresh_seq_elapsed = Duration::ZERO;
                let mut refresh_mixer_elapsed = Duration::ZERO;
                let mut refresh_samples_elapsed = Duration::ZERO;
                if refresh_visible_sequencer_after_cycle {
                    let started = Instant::now();
                    editor.refresh_visible_layouts_for_buffer_named("*sequencer*");
                    refresh_seq_elapsed = started.elapsed();
                }
                if refresh_visible_mixer_after_cycle {
                    let started = Instant::now();
                    editor.refresh_visible_layouts_for_buffer_named("*mixer*");
                    refresh_mixer_elapsed = started.elapsed();
                }
                if refresh_visible_samples_after_cycle {
                    let started = Instant::now();
                    editor.refresh_visible_layouts_for_buffer_named("*samples*");
                    refresh_samples_elapsed = started.elapsed();
                }
                if refresh_visible_step_after_cycle {
                    editor.refresh_visible_layouts_for_buffer_named("*step*");
                }
                editor.mark_needs_redraw();
                if profile_cycle {
                    eprintln!(
                        "[pattern-switch-profile][reactive-cycle] total={:.2}ms reactive={:.2}ms side_effects={:.2}ms refresh_seq={:.2}ms refresh_mixer={:.2}ms refresh_samples={:.2}ms refresh_seq_flag={} refresh_mixer_flag={} refresh_samples_flag={}",
                        duration_ms(cycle_total_started.elapsed()),
                        duration_ms(reactive_elapsed),
                        duration_ms(side_effects_elapsed),
                        duration_ms(refresh_seq_elapsed),
                        duration_ms(refresh_mixer_elapsed),
                        duration_ms(refresh_samples_elapsed),
                        refresh_visible_sequencer_after_cycle,
                        refresh_visible_mixer_after_cycle,
                        refresh_visible_samples_after_cycle,
                    );
                }
            }
        }
        ui_loop_stats.note_sync(reactive_sync_started.elapsed());

        // Keep selection animation live only during playback; when paused, edits/events
        // still request redraws explicitly, but idle should stay cheap.
        if playing_now && !selected_steps.lock().unwrap().is_empty() {
            editor.mark_needs_redraw();
        }

        stub_animation_cache.update_size(viewport_size);

        // Render
        if last_render_at.elapsed() >= frame_interval {
            if stub_animation_active && !editor.needs_redraw() && !sdf_animation_active {
                if let Some(tiled_frame) = stub_animation_cache.frame() {
                    let render_started = Instant::now();
                    let render_status = backend
                        .render_tiled(tiled_frame)
                        .map_err(|_| "render failed")?;
                    ui_loop_stats.note_frame(Duration::ZERO, render_started.elapsed());
                    if render_status == eseqlisp::metal_backend::TiledRenderStatus::Presented {
                        last_render_at = Instant::now();
                        continue;
                    }
                    last_render_at = Instant::now();
                }
            }
        }

        if editor.needs_redraw() && last_render_at.elapsed() >= frame_interval {
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
                    if backend.agent_instrument_stub_animation_visible() {
                        stub_animation_cache.store(viewport_size, tiled_frame);
                    } else {
                        stub_animation_cache.reset();
                    }
                    last_render_at = Instant::now();
                }
                eseqlisp::metal_backend::TiledRenderStatus::NotPresented => {
                    eseqlisp::frame::requeue_unpresented_tiled_frame(&mut editor, &tiled_frame);
                    last_render_at = Instant::now();
                }
            }
        }

        if editor.should_quit() {
            break;
        }
    }

    let _ = backend.teardown();
    drop(stream);
    unsafe {
        sequencer::audiograph::clear_os_workgroup();
        sequencer::audiograph::engine_stop_workers();
        sequencer::audiograph::destroy_live_graph(lg_ptr.0);
    }

    Ok(())
}
